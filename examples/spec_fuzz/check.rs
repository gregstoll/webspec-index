//! Per-spec driver: parse once, run every selected invariant over every section.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use webspec_index::parse::steps::StructuralSpec;

use crate::index::Stored;
use crate::invariants::{self, Ctx, DbView, Evidence, Invariant, Outcome, SectionCtx, SpecCtx};
use crate::oracle::SourceDoc;

pub struct SpecInput<'a> {
    pub spec: &'a str,
    pub base_url: &'a str,
    pub sha: &'a str,
    pub html: &'a str,
    pub db: Option<DbView>,
    pub stored: Option<Stored>,
    /// G4 held (or there is no index to compare against): S2/S3 may run.
    pub with_structure: bool,
}

/// One failing (invariant, section, evidence group).
#[derive(Clone, Debug)]
pub struct RawFinding {
    pub invariant: &'static str,
    pub report_only: bool,
    pub signature: String,
    pub spec: String,
    pub anchor: String,
    pub section_type: String,
    pub shape: String,
    pub evidence: Vec<Evidence>,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
    pub region_html: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Tally {
    pub checked: usize,
    pub failed: usize,
    pub not_applicable: usize,
}

#[derive(Debug, Default)]
pub struct SpecResult {
    pub spec: String,
    pub sections: usize,
    pub unlocated: Vec<String>,
    pub tally: BTreeMap<&'static str, Tally>,
    pub findings: Vec<RawFinding>,
    pub skipped: Option<String>,
    pub parse_ms: u128,
    pub structure_ms: u128,
    pub check_ms: u128,
}

pub fn signature(invariant: &str, shape_key: &str, e: &Evidence) -> String {
    format!("{invariant}|{shape_key}|{}:{}", e.kind, e.construct)
}

/// Split one failure into findings, one per (kind, construct) group.
fn record(
    out: &mut Vec<RawFinding>,
    inv: &dyn Invariant,
    s: Option<&SectionCtx>,
    spec: &str,
    failure: invariants::Failure,
    region_html: &mut Option<String>,
) {
    let shape_key = s.map(|s| s.shape_key()).unwrap_or_else(|| "spec".into());
    let mut groups: BTreeMap<String, Vec<Evidence>> = BTreeMap::new();
    for e in failure.items {
        groups
            .entry(signature(inv.id(), &shape_key, &e))
            .or_default()
            .push(e);
    }
    for (sig, mut items) in groups {
        items.truncate(5);
        let html = match s {
            Some(s) if matches!(inv.id(), "M0" | "M2" | "M4") => s.fixture_html_for(inv.id()),
            Some(s) => region_html.get_or_insert_with(|| s.region_html()).clone(),
            None => String::new(),
        };
        out.push(RawFinding {
            invariant: inv.id(),
            report_only: inv.report_only(),
            signature: sig,
            spec: spec.to_string(),
            anchor: s.map(|s| s.anchor().to_string()).unwrap_or_default(),
            section_type: s
                .map(|s| s.ty().as_str().to_string())
                .unwrap_or_else(|| "spec".into()),
            shape: s
                .map(|s| s.region.shape.as_str().to_string())
                .unwrap_or_else(|| "spec".into()),
            evidence: items,
            expected: failure.expected.clone(),
            actual: failure.actual.clone(),
            region_html: html,
        });
    }
}

pub fn build_ctx(input: SpecInput<'_>, result: &mut SpecResult) -> anyhow::Result<SpecCtx> {
    let t = Instant::now();
    let parsed = webspec_index::parse::parse_spec(input.html, input.spec, input.base_url)?;
    result.parse_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let structure: StructuralSpec = webspec_index::parse::steps::extract_step_structure(
        input.html,
        input.spec,
        input.base_url,
        input.sha,
    );
    result.structure_ms = t.elapsed().as_millis();
    let src = SourceDoc::new(input.html, input.spec, input.base_url);
    let structure = (input.with_structure || input.stored.is_none()).then_some(structure);
    Ok(SpecCtx::new(src, parsed, structure, input.db, input.stored))
}

pub fn check_spec(input: SpecInput<'_>, selected: &[&'static dyn Invariant]) -> SpecResult {
    let mut result = SpecResult {
        spec: input.spec.to_string(),
        ..Default::default()
    };
    let mut ctx = match build_ctx(input, &mut result) {
        Ok(c) => c,
        Err(e) => {
            result.skipped = Some(format!("parse failed: {e}"));
            return result;
        }
    };
    let t = Instant::now();
    run_checks(&mut ctx, selected, &mut result);
    result.check_ms = t.elapsed().as_millis();
    result
}

pub fn run_checks(ctx: &mut SpecCtx, selected: &[&'static dyn Invariant], result: &mut SpecResult) {
    let spec = ctx.spec().to_string();
    for inv in selected.iter().filter(|i| i.spec_level()) {
        let tally = result.tally.entry(inv.id()).or_default();
        match inv.check(Ctx::Spec(ctx)) {
            Outcome::Pass => tally.checked += 1,
            Outcome::NotApplicable => tally.not_applicable += 1,
            Outcome::Fail(f) => {
                tally.checked += 1;
                tally.failed += 1;
                // A stale structure only invalidates the IR checks; stale sections or refs
                // invalidate everything.
                let structure_only =
                    inv.id() == "D1" && f.items.iter().all(|e| e.construct == "structure");
                record(&mut result.findings, *inv, None, &spec, f, &mut None);
                if structure_only {
                    ctx.structure = None;
                } else if inv.id() == "D1" {
                    result.skipped = Some(format!(
                        "stale index for {spec}: run 'webspec-index reparse --spec {spec}' with this build"
                    ));
                    return;
                }
            }
        }
    }
    let mut sections: Vec<_> = ctx.unique_sections().collect();
    sections.sort_by(|a, b| a.anchor.cmp(&b.anchor));
    result.sections = sections.len();
    for section in sections {
        let Some(sctx) = SectionCtx::new(ctx, section) else {
            result.unlocated.push(section.anchor.clone());
            continue;
        };
        let mut region_html = None;
        for inv in selected.iter().filter(|i| !i.spec_level()) {
            let tally = result.tally.entry(inv.id()).or_default();
            match inv.check(Ctx::Section(&sctx)) {
                Outcome::Pass => tally.checked += 1,
                Outcome::NotApplicable => tally.not_applicable += 1,
                Outcome::Fail(f) => {
                    tally.checked += 1;
                    tally.failed += 1;
                    record(
                        &mut result.findings,
                        *inv,
                        Some(&sctx),
                        &spec,
                        f,
                        &mut region_html,
                    );
                }
            }
        }
    }
}

/// Run a closure on one section of a standalone document (fixtures, replay, self-tests).
pub fn with_section<R>(
    html: &str,
    spec: &str,
    base_url: &str,
    anchor: &str,
    f: impl FnOnce(&SectionCtx) -> R,
) -> anyhow::Result<R> {
    let mut result = SpecResult::default();
    let ctx = build_ctx(
        SpecInput {
            spec,
            base_url,
            sha: "hash:fixture",
            html,
            db: None,
            stored: None,
            with_structure: true,
        },
        &mut result,
    )?;
    let section = ctx
        .section(anchor)
        .ok_or_else(|| anyhow::anyhow!("{spec}#{anchor} is not a section of this document"))?;
    let sctx = SectionCtx::new(&ctx, section)
        .ok_or_else(|| anyhow::anyhow!("the oracle cannot locate #{anchor}"))?;
    Ok(f(&sctx))
}
