//! `promote`: turn a finding (or any indexed section) into a ratchet fixture (§10).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::check::{self, SpecInput, SpecResult};
use crate::invariants::{self, Ctx, Invariant, Outcome, SectionCtx};
use crate::oracle::{self, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Fixed,
    KnownBug,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub spec: String,
    pub base_url: String,
    pub anchor: String,
    pub invariant: String,
    pub status: Status,
    pub bug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub expected: serde_json::Value,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fuzz")
}

const PROMOTABLE: [&str; 16] = [
    "C1", "C2", "C3", "S1", "S2", "S3", "M0", "M1", "M2", "M4", "R1", "R2", "N1", "T1", "A1", "D1",
];

/// Oracle-computed values the regression test compares the product against.
pub fn expected_values(inv: &dyn Invariant, s: &SectionCtx) -> serde_json::Value {
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    match inv.id() {
        "C1" => json!({
            "links": sorted(s.src_text().expected_links()),
            "one_sided": s.region.shape.is_heading(),
        }),
        "C2" => json!({"words": sorted(oracle::words(&s.src_text().text))}),
        "C3" => json!({"nonempty": true}),
        "S1" | "S2" => {
            let (steps, depth) = crate::invariants::step_counts(s);
            json!({"steps": steps, "depth": depth})
        }
        "M1" => json!({"url": s.spec.section_url(s.anchor())}),
        "M4" => json!({"callout_links": sorted(crate::invariants::callout_links(s))}),
        "R1" | "R2" => {
            json!({"targets": sorted(crate::invariants::scope_targets(s, inv.id() == "R1"))})
        }
        "S3" => json!({
            "links": sorted(crate::invariants::structure_body_links(s)),
        }),
        "T1" => json!({"not_section_type": "algorithm"}),
        "A1" => {
            let anchors = match inv.check(Ctx::Section(s)) {
                Outcome::Fail(f) => f.items.into_iter().map(|e| e.item).collect(),
                _ => Vec::new(),
            };
            json!({"anchors": anchors})
        }
        "D1" => json!({"content": s.section.content_text}),
        _ => json!({}),
    }
}

fn verdict(inv: &dyn Invariant, html: &str, f: &Fixture) -> Result<Option<Vec<String>>> {
    check::with_section(html, &f.spec, &f.base_url, &f.anchor, |s| {
        match inv.check(Ctx::Section(s)) {
            Outcome::Fail(fail) => Some(
                fail.items
                    .iter()
                    .map(|e| check::signature(inv.id(), &s.shape_key(), e))
                    .collect(),
            ),
            _ => None,
        }
    })
}

/// Drop region descendants one at a time while the finding still reproduces.
fn reduce(inv: &dyn Invariant, html: &str, f: &Fixture, signature: &str) -> Result<String> {
    let mut result = SpecResult::default();
    let ctx = check::build_ctx(
        SpecInput {
            spec: &f.spec,
            base_url: &f.base_url,
            sha: "hash:fixture",
            html,
            db: None,
            stored: None,
            with_structure: true,
        },
        &mut result,
    )?;
    let section = ctx
        .section(&f.anchor)
        .context("anchor is not a section of the region")?;
    let s = SectionCtx::new(&ctx, section).context("oracle cannot locate the anchor")?;
    // Every ancestor of the anchor stays. So does the anchor's own content where the anchor only
    // names the region (a dfn, a heading) rather than being it.
    let anchor_is_region = matches!(
        s.region.shape,
        Shape::Element | Shape::EmuClause | Shape::RfcSection
    );
    let keep: HashSet<_> = if anchor_is_region {
        s.el.ancestors().chain([*s.el]).map(|a| a.id()).collect()
    } else {
        s.el.ancestors()
            .chain(s.el.descendants())
            .map(|a| a.id())
            .collect()
    };
    let (roots, mut excluded) =
        if s.region.shape.is_heading() && matches!(inv.id(), "M0" | "M2" | "M4") {
            let mut roots = vec![s.region.fixture_roots[0]];
            roots.extend(oracle::heading_extent(s.el));
            (roots, HashSet::new())
        } else {
            (s.region.fixture_roots.clone(), s.region.excluded.clone())
        };
    let mut candidates = Vec::new();
    let mut frontier: Vec<_> = if roots.len() > 1 {
        roots.clone()
    } else {
        roots.iter().flat_map(|r| r.children()).collect()
    };
    for _ in 0..4 {
        let mut next = Vec::new();
        for n in frontier {
            if !n.value().is_element() {
                continue;
            }
            if !keep.contains(&n.id()) {
                candidates.push(n);
            }
            next.extend(n.children());
        }
        frontier = next;
    }
    let mut attempts = 0;
    for c in candidates {
        if attempts >= 600 {
            break;
        }
        if c.ancestors().any(|a| excluded.contains(&a.id())) {
            continue;
        }
        attempts += 1;
        excluded.insert(c.id());
        let trial = oracle::fixture_html(&roots, &excluded);
        let still = verdict(inv, &trial, f)
            .ok()
            .flatten()
            .is_some_and(|sigs| sigs.iter().any(|x| x == signature));
        if !still {
            excluded.remove(&c.id());
        }
    }
    Ok(oracle::fixture_html(&roots, &excluded))
}

pub struct Request {
    pub name: String,
    pub status: Status,
    pub bug: String,
    pub reduce: bool,
}

/// Write `<name>.html` and `<name>.json` after checking the verdict matches the status.
pub fn promote_html(
    req: &Request,
    invariant: &str,
    spec: &str,
    base_url: &str,
    anchor: &str,
    html: &str,
    signature: Option<&str>,
) -> Result<PathBuf> {
    let inv =
        invariants::by_id(invariant).with_context(|| format!("unknown invariant {invariant}"))?;
    if !PROMOTABLE.contains(&inv.id()) {
        bail!(
            "{} needs the index and cannot be a standalone fixture",
            inv.id()
        );
    }
    let mut fixture = Fixture {
        name: req.name.clone(),
        spec: spec.to_string(),
        base_url: base_url.to_string(),
        anchor: anchor.to_string(),
        invariant: inv.id().to_string(),
        status: req.status,
        bug: req.bug.clone(),
        signature: signature.map(str::to_string),
        expected: json!({}),
    };
    // A section promoted without a finding reduces towards its first failing signature.
    let derived;
    let signature = match signature {
        Some(s) => Some(s),
        None if req.status == Status::KnownBug => {
            derived = verdict(inv, html, &fixture)?.and_then(|sigs| sigs.into_iter().next());
            derived.as_deref()
        }
        None => None,
    };
    fixture.signature = signature.map(str::to_string);
    let html = match (req.status, signature) {
        (Status::KnownBug, Some(sig)) if req.reduce => reduce(inv, html, &fixture, sig)?,
        _ => html.to_string(),
    };
    let v = verdict(inv, &html, &fixture)?;
    match (req.status, &v) {
        (Status::KnownBug, None) => bail!(
            "{} passes on the fixture; nothing to record as known_bug",
            inv.id()
        ),
        (Status::KnownBug, Some(sigs)) => {
            if let Some(sig) = signature {
                if !sigs.iter().any(|x| x == sig) {
                    bail!("fixture fails with {sigs:?}, not the finding's signature {sig}");
                }
            }
        }
        (Status::Fixed, Some(sigs)) => bail!(
            "{} fails on the fixture ({sigs:?}); it is not fixed",
            inv.id()
        ),
        (Status::Fixed, None) => {}
    }
    fixture.expected =
        check::with_section(&html, spec, base_url, anchor, |s| expected_values(inv, s))?;
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir)?;
    let json_path = dir.join(format!("{}.json", req.name));
    std::fs::write(dir.join(format!("{}.html", req.name)), &html)?;
    std::fs::write(&json_path, serde_json::to_string_pretty(&fixture)? + "\n")?;
    Ok(json_path)
}
