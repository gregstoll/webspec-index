//! Findings store (dedupe by signature), manifest, summary and the `report` command (§9).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::check::{RawFinding, SpecResult, Tally};
use crate::index::{Census, Untestable};
use crate::invariants::Evidence;

/// Region HTML beyond this size is not stored; such findings cannot be replayed or promoted.
const MAX_REGION_HTML: usize = 256 * 1024;
const MORE_ANCHORS: usize = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub run_id: String,
    pub harness_rev: String,
    pub dirty: bool,
    pub index_version: String,
    pub structure_version: String,
    pub db: String,
    pub invariants: Vec<String>,
    /// spec -> (base_url, provider, snapshot sha)
    pub snapshots: BTreeMap<String, (String, String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindingRecord {
    pub id: String,
    pub invariant: String,
    pub report_only: bool,
    pub signature: String,
    pub run_id: String,
    pub spec: String,
    pub base_url: String,
    pub provider: String,
    pub snapshot: String,
    pub index_version: String,
    pub harness_rev: String,
    pub dirty: bool,
    pub anchor: String,
    pub section_type: String,
    pub shape: String,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
    pub evidence: Vec<Evidence>,
    pub region_html: Option<String>,
    pub region_len: usize,
    pub count: usize,
    pub more_anchors: Vec<String>,
    pub replay: String,
}

pub fn finding_id(invariant: &str, signature: &str) -> String {
    let h = crate::index::sha256_hex(signature.as_bytes());
    format!("{invariant}-{}", &h[..6])
}

#[derive(Default)]
pub struct Store {
    pub findings: BTreeMap<String, FindingRecord>,
}

impl Store {
    pub fn add(&mut self, raw: &RawFinding, manifest: &Manifest, dir: &Path) {
        let (base_url, provider, snapshot) = manifest
            .snapshots
            .get(&raw.spec)
            .cloned()
            .unwrap_or_default();
        let region_len = raw.region_html.len();
        let spec_anchor = format!("{}#{}", raw.spec, raw.anchor);
        let make = || {
            let id = finding_id(raw.invariant, &raw.signature);
            FindingRecord {
                replay: format!(
                    "cargo run --release --example spec_fuzz -- replay {} {id}",
                    dir.display()
                ),
                id,
                invariant: raw.invariant.to_string(),
                report_only: raw.report_only,
                signature: raw.signature.clone(),
                run_id: manifest.run_id.clone(),
                spec: raw.spec.clone(),
                base_url: base_url.clone(),
                provider: provider.clone(),
                snapshot: snapshot.clone(),
                index_version: manifest.index_version.clone(),
                harness_rev: manifest.harness_rev.clone(),
                dirty: manifest.dirty,
                anchor: raw.anchor.clone(),
                section_type: raw.section_type.clone(),
                shape: raw.shape.clone(),
                expected: raw.expected.clone(),
                actual: raw.actual.clone(),
                evidence: raw.evidence.clone(),
                region_html: (region_len <= MAX_REGION_HTML && region_len > 0)
                    .then(|| raw.region_html.clone()),
                region_len,
                count: 1,
                more_anchors: Vec::new(),
            }
        };
        match self.findings.get_mut(&raw.signature) {
            None => {
                self.findings.insert(raw.signature.clone(), make());
            }
            Some(rec) => {
                rec.count += 1;
                let replace = region_len > 0 && region_len < rec.region_len;
                let demoted = if replace {
                    let old = format!("{}#{}", rec.spec, rec.anchor);
                    let (count, more) = (rec.count, std::mem::take(&mut rec.more_anchors));
                    *rec = make();
                    rec.count = count;
                    rec.more_anchors = more;
                    old
                } else {
                    spec_anchor
                };
                if rec.more_anchors.len() < MORE_ANCHORS && !rec.more_anchors.contains(&demoted) {
                    rec.more_anchors.push(demoted);
                }
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SpecSummary {
    pub sections: usize,
    pub unlocated: usize,
    pub findings: usize,
    pub skipped: Option<String>,
    pub parse_ms: u128,
    pub structure_ms: u128,
    pub check_ms: u128,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub run_id: String,
    pub elapsed_s: f64,
    pub totals: BTreeMap<String, Tally>,
    pub distinct: BTreeMap<String, usize>,
    pub specs: BTreeMap<String, SpecSummary>,
    pub untestable: Vec<UntestableRow>,
    pub findings: Vec<FindingLine>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UntestableRow {
    pub name: String,
    pub reason: String,
}

impl From<&Untestable> for UntestableRow {
    fn from(u: &Untestable) -> Self {
        UntestableRow {
            name: u.name.clone(),
            reason: u.reason.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FindingLine {
    pub id: String,
    pub invariant: String,
    pub report_only: bool,
    pub signature: String,
    pub count: usize,
    pub representative: String,
}

pub fn build(
    results: &[SpecResult],
    census: &Census,
    manifest: &Manifest,
    dir: &Path,
    elapsed_s: f64,
) -> (Store, Summary) {
    let mut store = Store::default();
    let mut summary = Summary {
        run_id: manifest.run_id.clone(),
        elapsed_s,
        untestable: census.untestable.iter().map(UntestableRow::from).collect(),
        ..Default::default()
    };
    for r in results {
        for f in &r.findings {
            store.add(f, manifest, dir);
        }
        for (inv, t) in &r.tally {
            let e = summary.totals.entry(inv.to_string()).or_default();
            e.checked += t.checked;
            e.failed += t.failed;
            e.not_applicable += t.not_applicable;
        }
        summary.specs.insert(
            r.spec.clone(),
            SpecSummary {
                sections: r.sections,
                unlocated: r.unlocated.len(),
                findings: r.findings.len(),
                skipped: r.skipped.clone(),
                parse_ms: r.parse_ms,
                structure_ms: r.structure_ms,
                check_ms: r.check_ms,
            },
        );
    }
    for rec in store.findings.values() {
        *summary.distinct.entry(rec.invariant.clone()).or_default() += 1;
    }
    let mut lines: Vec<FindingLine> = store
        .findings
        .values()
        .map(|r| FindingLine {
            id: r.id.clone(),
            invariant: r.invariant.clone(),
            report_only: r.report_only,
            signature: r.signature.clone(),
            count: r.count,
            representative: format!("{}#{}", r.spec, r.anchor),
        })
        .collect();
    lines.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.signature.cmp(&b.signature))
    });
    summary.findings = lines;
    (store, summary)
}

pub fn write(dir: &Path, manifest: &Manifest, store: &Store, summary: &Summary) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(manifest)?,
    )?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join("findings.jsonl"))?);
    for rec in store.findings.values() {
        writeln!(f, "{}", serde_json::to_string(rec)?)?;
    }
    f.flush()?;
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(summary)?,
    )?;
    Ok(())
}

pub fn load_findings(dir: &Path) -> Result<Vec<FindingRecord>> {
    let text = std::fs::read_to_string(dir.join("findings.jsonl"))
        .with_context(|| format!("reading {}/findings.jsonl", dir.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| Ok(serde_json::from_str(l)?))
        .collect()
}

pub fn load_finding(dir: &Path, id: &str) -> Result<FindingRecord> {
    load_findings(dir)?
        .into_iter()
        .find(|f| f.id == id)
        .with_context(|| format!("no finding {id} in {}", dir.display()))
}

pub fn print_report(dir: &Path) -> Result<()> {
    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json"))?)?;
    let summary: Summary =
        serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json"))?)?;
    let findings = load_findings(dir)?;
    let by_id: BTreeMap<&str, &FindingRecord> =
        findings.iter().map(|f| (f.id.as_str(), f)).collect();
    println!(
        "run {}  harness {}{}  index {}  structure v{}  {:.1}s",
        manifest.run_id,
        manifest.harness_rev,
        if manifest.dirty { "+dirty" } else { "" },
        manifest.index_version,
        manifest.structure_version,
        summary.elapsed_s
    );
    let skipped: Vec<_> = summary
        .specs
        .iter()
        .filter(|(_, s)| s.skipped.is_some())
        .collect();
    println!(
        "specs checked: {}  skipped: {}  untestable: {}",
        summary.specs.len() - skipped.len(),
        skipped.len(),
        summary.untestable.len()
    );
    for (name, s) in &skipped {
        println!("  skipped {name}: {}", s.skipped.as_deref().unwrap_or(""));
    }
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for u in &summary.untestable {
        *reasons
            .entry(u.reason.split(':').next().unwrap_or(&u.reason))
            .or_default() += 1;
    }
    for (r, n) in reasons {
        println!("  untestable ({r}): {n}");
    }
    println!(
        "\n{:<4} {:>9} {:>8} {:>9} {:>9}",
        "inv", "checked", "failed", "n/a", "distinct"
    );
    for inv in crate::invariants::all() {
        let t = summary.totals.get(inv.id()).copied().unwrap_or_default();
        println!(
            "{:<4} {:>9} {:>8} {:>9} {:>9}{}",
            inv.id(),
            t.checked,
            t.failed,
            t.not_applicable,
            summary.distinct.get(inv.id()).copied().unwrap_or(0),
            if inv.report_only() {
                "  (report-only)"
            } else {
                ""
            }
        );
    }
    for report_only in [false, true] {
        println!(
            "\n{}",
            if report_only {
                "report-only findings"
            } else {
                "findings (by occurrence count)"
            }
        );
        for line in summary
            .findings
            .iter()
            .filter(|l| l.report_only == report_only)
        {
            let item = by_id
                .get(line.id.as_str())
                .and_then(|f| f.evidence.first())
                .map(|e| crate::oracle::excerpt(&e.item, 90))
                .unwrap_or_default();
            println!(
                "{:<10} {:>6}  {:<70}  {}  e.g. {}",
                line.id, line.count, line.signature, line.representative, item
            );
        }
    }
    Ok(())
}
