//! D1: the stored index equals a fresh in-process parse of the cached HTML (§5.2).

use std::collections::BTreeMap;

use serde_json::json;
use webspec_index::model::ParsedSection;

use super::{Ctx, Evidence, Failure, Invariant, Outcome, SpecCtx};
use crate::index::{RefRow, Stored};

pub struct D1;
impl Invariant for D1 {
    fn id(&self) -> &'static str {
        "D1"
    }
    fn spec_level(&self) -> bool {
        true
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Ctx::Spec(spec) = ctx else {
            return Outcome::NotApplicable;
        };
        let Some(stored) = &spec.stored else {
            return Outcome::NotApplicable;
        };
        let items = d1_diff(spec, stored);
        Outcome::check(items.is_empty(), || Failure {
            expected: json!({"sections": spec.first.len(), "refs": spec.parsed.references.len()}),
            actual: json!({"sections": stored.sections.len(), "refs": stored.refs.len()}),
            items,
        })
    }
}

/// JSON path and both values at the first point where `fresh` and `stored` differ.
fn first_difference(
    fresh: &serde_json::Value,
    stored: &serde_json::Value,
    path: String,
) -> Option<String> {
    use serde_json::Value;
    match (fresh, stored) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                match b.get(k) {
                    Some(vb) => {
                        if let Some(d) = first_difference(va, vb, format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: only in fresh")),
                }
            }
            b.keys()
                .find(|k| !a.contains_key(*k))
                .map(|k| format!("{path}.{k}: only in stored"))
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                if let Some(d) = first_difference(va, vb, format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            (a.len() != b.len())
                .then(|| format!("{path}: length fresh={} stored={}", a.len(), b.len()))
        }
        (a, b) if a != b => Some(format!(
            "{path}: fresh={} stored={}",
            crate::oracle::excerpt(&a.to_string(), 120),
            crate::oracle::excerpt(&b.to_string(), 120)
        )),
        _ => None,
    }
}

fn fields(s: &ParsedSection) -> [(&'static str, String); 8] {
    [
        ("title", format!("{:?}", s.title)),
        ("content", format!("{:?}", s.content_text)),
        ("type", s.section_type.as_str().to_string()),
        ("parent", format!("{:?}", s.parent_anchor)),
        ("prev", format!("{:?}", s.prev_anchor)),
        ("next", format!("{:?}", s.next_anchor)),
        ("depth", format!("{:?}", s.depth)),
        ("number", format!("{:?}", s.number)),
    ]
}

/// Differences between the stored rows and the fresh parse, one evidence item per differing
/// field kind with up to five example anchors.
pub fn d1_diff(spec: &SpecCtx, stored: &Stored) -> Vec<Evidence> {
    let mut by_field: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let fresh: Vec<&ParsedSection> = spec.unique_sections().collect();
    let stored_by_anchor: BTreeMap<&str, &ParsedSection> = stored
        .sections
        .iter()
        .map(|s| (s.anchor.as_str(), s))
        .collect();
    for f in &fresh {
        match stored_by_anchor.get(f.anchor.as_str()) {
            None => by_field
                .entry("section-missing".into())
                .or_default()
                .push(f.anchor.clone()),
            Some(st) => {
                for ((name, a), (_, b)) in fields(f).iter().zip(fields(st).iter()) {
                    if a != b {
                        by_field
                            .entry(format!("section-{name}"))
                            .or_default()
                            .push(f.anchor.clone());
                    }
                }
            }
        }
    }
    let fresh_anchors: std::collections::HashSet<&str> =
        fresh.iter().map(|s| s.anchor.as_str()).collect();
    for st in &stored.sections {
        if !fresh_anchors.contains(st.anchor.as_str()) {
            by_field
                .entry("section-extra".into())
                .or_default()
                .push(st.anchor.clone());
        }
    }
    let mut fresh_refs: Vec<RefRow> = spec
        .parsed
        .references
        .iter()
        .map(RefRow::from_parsed)
        .collect();
    let mut stored_refs = stored.refs.clone();
    fresh_refs.sort();
    stored_refs.sort();
    if fresh_refs != stored_refs {
        let a: std::collections::BTreeSet<_> = fresh_refs.iter().collect();
        let b: std::collections::BTreeSet<_> = stored_refs.iter().collect();
        let mut ex: Vec<String> = a
            .symmetric_difference(&b)
            .map(|r| r.from_anchor.clone())
            .collect();
        ex.dedup();
        by_field
            .entry("refs".into())
            .or_default()
            .extend(ex.into_iter().chain(std::iter::once(format!(
                "fresh={} stored={}",
                fresh_refs.len(),
                stored_refs.len()
            ))));
    }
    if let (Some(fresh), Some(json)) = (&spec.structure, &stored.structure_json) {
        let a = serde_json::to_value(fresh).unwrap_or_default();
        let b: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        if a != b {
            let at = first_difference(&a, &b, String::new()).unwrap_or_default();
            by_field.entry("structure".into()).or_default().push(at);
        }
    }
    by_field
        .into_iter()
        .map(|(field, anchors)| {
            let n = anchors.len();
            let mut ex: Vec<String> = anchors.into_iter().take(5).collect();
            if n > 5 {
                ex.push(format!("(+{} more)", n - 5));
            }
            Evidence::new("stale", ex.join(" "), field)
        })
        .collect()
}
