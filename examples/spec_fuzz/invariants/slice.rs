//! Slice invariants L1–L3 (spec §14.3): mention coverage, conservation and context closure.

use std::collections::{BTreeMap, HashSet};

use scraper::ElementRef;
use serde_json::json;
use webspec_index::db::state::load_slice_index;
use webspec_index::state::extract::{extract_state, StateInputs};
use webspec_index::state::model::StateCatalog;
use webspec_index::state::slice::select::{slice, KeptStep, Slice, StepRole, ViewRequest};
use webspec_index::state::slice::{build_slice_indexes, SliceIndex};

use super::{Ctx, Evidence, Invariant, Outcome, SectionCtx, SpecCtx};
use crate::oracle::N;

/// One numbered step in an algorithm body, with independently extracted variable mentions.
#[derive(Debug)]
pub struct SourceStep {
    pub path: String,
    pub own_vars: Vec<String>,
    #[allow(dead_code)]
    pub own_text: String,
}

/// Walk the algorithm body OLs and extract every numbered `li` with its variables and text.
///
/// Steps are numbered in document order: top-level steps share a counter across all body OLs;
/// nested steps within a `li` reset the counter for their OL.
pub fn source_steps(s: &SectionCtx) -> Vec<SourceStep> {
    let ols = super::structure::body_ols(s);
    let mut result = Vec::new();
    let mut top_counter = 0usize;
    for ol in ols {
        walk_ol(ol, &[], &mut top_counter, &mut result);
    }
    result
}

fn walk_ol(ol: N, prefix: &[usize], counter: &mut usize, out: &mut Vec<SourceStep>) {
    for child in ol.children() {
        if crate::oracle::elname(&child) != Some("li") {
            continue;
        }
        *counter += 1;
        let mut my_path = prefix.to_vec();
        my_path.push(*counter);
        let path_str = my_path
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".");

        let (own_vars, own_text) = extract_li_content(child);
        out.push(SourceStep {
            path: path_str.clone(),
            own_vars,
            own_text,
        });

        let mut nested_counter = 0usize;
        for grandchild in child.children() {
            if crate::oracle::elname(&grandchild) == Some("ol") {
                walk_ol(grandchild, &my_path, &mut nested_counter, out);
            }
        }
    }
}

fn is_excluded_boundary(e: &ElementRef) -> bool {
    let name = e.value().name();
    if matches!(name, "ol" | "ul" | "dl" | "li") {
        return true;
    }
    if name == "aside" {
        return true;
    }
    crate::oracle::has_class(e, "note")
        || crate::oracle::has_class(e, "example")
        || crate::oracle::has_class(e, "warning")
        || crate::oracle::has_class(e, "advisement")
}

fn is_named_argument_label(var_elem: &ElementRef) -> bool {
    let var_text: String = var_elem.text().collect();

    if let Some(parent) = var_elem.parent().and_then(ElementRef::wrap) {
        if parent.value().name() == "a" {
            let parent_text: String = parent.text().collect();
            if parent_text == var_text {
                return true;
            }
        }
    }

    let element_children: Vec<_> = var_elem.children().filter_map(ElementRef::wrap).collect();
    if element_children.len() == 1 && element_children[0].value().name() == "a" {
        let child_text: String = element_children[0].text().collect();
        if child_text == var_text {
            return true;
        }
    }

    false
}

fn walk_for_content(n: N, vars: &mut Vec<String>, text_parts: &mut Vec<String>) {
    for child in n.children() {
        if let Some(e) = ElementRef::wrap(child) {
            if is_excluded_boundary(&e) {
                continue;
            }
            if e.value().name() == "var" {
                let t: String = e.text().collect();
                if !is_named_argument_label(&e) {
                    vars.push(t.clone());
                }
                text_parts.push(t);
                continue;
            }
            walk_for_content(child, vars, text_parts);
        } else if let Some(t) = child.value().as_text() {
            text_parts.push(t.to_string());
        }
    }
}

fn extract_li_content(li: N) -> (Vec<String>, String) {
    let mut vars = Vec::new();
    let mut text_parts = Vec::new();
    walk_for_content(li, &mut vars, &mut text_parts);
    let own_text = text_parts.concat();
    (vars, own_text)
}

fn section_ctx<'c, 'a>(ctx: &Ctx<'c, 'a>) -> Option<&'c SectionCtx<'a>> {
    match ctx {
        Ctx::Section(s) => Some(s),
        Ctx::Spec(_) => None,
    }
}

// ── Pure cores ───────────────────────────────────────────────────────────────────────────────────

/// For every variable `v` in the index: finds source steps with `v` in `own_vars` that the
/// `involving:[v]` slice does not keep as `match` or `inherited`. Returns `missing:{path}` strings.
pub fn uncovered(index: &SliceIndex, source: &[SourceStep]) -> Vec<String> {
    let mut result = Vec::new();
    for var_name in &index.vars {
        let view_req = ViewRequest {
            involving: vec![var_name.clone()],
            ..Default::default()
        };
        let Ok(s) = slice(index, &view_req) else {
            continue;
        };
        let kept: HashSet<&str> = s
            .steps
            .iter()
            .filter(|k| matches!(k.role, StepRole::Match | StepRole::Inherited))
            .map(|k| k.path.as_str())
            .collect();
        for src_step in source {
            if src_step.own_vars.iter().any(|v| v == var_name)
                && !kept.contains(src_step.path.as_str())
            {
                let item = format!("missing:{}", src_step.path);
                if !result.contains(&item) {
                    result.push(item);
                }
            }
        }
    }
    result
}

/// Checks that kept steps plus omitted run step counts equal `source_count`.
/// Returns `Some("count:{kept}+{omitted}!={src}")` on failure, `None` on success.
pub fn conservation_gap(s: &Slice, source_count: usize) -> Option<String> {
    let kept = s.steps.len();
    let omitted: usize = s.omitted.iter().map(|r| r.steps as usize).sum();
    if kept + omitted == source_count {
        None
    } else {
        Some(format!("count:{kept}+{omitted}!={source_count}"))
    }
}

/// Returns the paths of kept steps whose parent prefix is not also kept, and `run-parent:P` for
/// omitted runs whose parent `P` is not kept.
pub fn open_prefixes(s: &Slice) -> Vec<String> {
    let kept_paths: HashSet<&str> = s.steps.iter().map(|k| k.path.as_str()).collect();
    let mut gaps: Vec<String> = Vec::new();

    for step in &s.steps {
        let parts: Vec<&str> = step.path.split('.').collect();
        let has_missing_prefix = (1..parts.len()).any(|d| {
            let prefix = parts[..d].join(".");
            !kept_paths.contains(prefix.as_str())
        });
        if has_missing_prefix && !gaps.contains(&step.path) {
            gaps.push(step.path.clone());
        }
    }

    for run in &s.omitted {
        if let Some(parent) = &run.parent {
            if !kept_paths.contains(parent.as_str()) {
                let item = format!("run-parent:{parent}");
                if !gaps.contains(&item) {
                    gaps.push(item);
                }
            }
        }
    }

    gaps
}

// ── Invariant wrappers ───────────────────────────────────────────────────────────────────────────

pub struct L1;
impl Invariant for L1 {
    fn id(&self) -> &'static str {
        "L1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section_ctx(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(index) = s.spec.slice_index(s.anchor()) else {
            return Outcome::NotApplicable;
        };
        let source = source_steps(s);
        let missing_paths = uncovered(&index, &source);
        if missing_paths.is_empty() {
            return Outcome::Pass;
        }
        let mut items: Vec<Evidence> = Vec::new();
        for var_name in &index.vars {
            let view_req = ViewRequest {
                involving: vec![var_name.clone()],
                ..Default::default()
            };
            let Ok(sl) = slice(&index, &view_req) else {
                continue;
            };
            let kept: HashSet<&str> = sl
                .steps
                .iter()
                .filter(|k: &&KeptStep| matches!(k.role, StepRole::Match | StepRole::Inherited))
                .map(|k| k.path.as_str())
                .collect();
            for src_step in &source {
                if src_step.own_vars.iter().any(|v| v == var_name)
                    && !kept.contains(src_step.path.as_str())
                {
                    items.push(Evidence::new(
                        "missing",
                        format!("missing:{}", src_step.path),
                        format!("var {var_name}"),
                    ));
                }
            }
        }
        Outcome::fail(
            items,
            json!({"index_vars": index.vars.len()}),
            json!({"uncovered": missing_paths}),
        )
    }
}

pub struct L2;
impl Invariant for L2 {
    fn id(&self) -> &'static str {
        "L2"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section_ctx(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(index) = s.spec.slice_index(s.anchor()) else {
            return Outcome::NotApplicable;
        };
        let source_count = super::structure::step_counts(s).0;
        let mut items: Vec<Evidence> = Vec::new();
        for var_name in &index.vars {
            let view_req = ViewRequest {
                involving: vec![var_name.clone()],
                ..Default::default()
            };
            let Ok(sl) = slice(&index, &view_req) else {
                continue;
            };
            if let Some(gap) = conservation_gap(&sl, source_count) {
                items.push(Evidence::new(
                    "conservation",
                    gap,
                    format!("var {var_name}"),
                ));
            }
        }
        if items.is_empty() {
            Outcome::Pass
        } else {
            Outcome::fail(items, json!({"source_steps": source_count}), json!({}))
        }
    }
}

pub struct L3;
impl Invariant for L3 {
    fn id(&self) -> &'static str {
        "L3"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section_ctx(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(index) = s.spec.slice_index(s.anchor()) else {
            return Outcome::NotApplicable;
        };
        let mut items: Vec<Evidence> = Vec::new();
        for var_name in &index.vars {
            let view_req = ViewRequest {
                involving: vec![var_name.clone()],
                ..Default::default()
            };
            let Ok(sl) = slice(&index, &view_req) else {
                continue;
            };
            for gap in open_prefixes(&sl) {
                items.push(Evidence::new("open-prefix", gap, format!("var {var_name}")));
            }
        }
        if items.is_empty() {
            Outcome::Pass
        } else {
            Outcome::fail(items, json!({}), json!({}))
        }
    }
}

// ── SpecCtx slice index ──────────────────────────────────────────────────────────────────────────

impl SpecCtx {
    pub fn slice_index(&self, anchor: &str) -> Option<SliceIndex> {
        if let Some(db) = &self.db {
            return load_slice_index(&db.conn, db.snapshot_id, anchor)
                .ok()
                .flatten();
        }
        let map = self.slice_indexes.get_or_init(|| {
            let Some(structure) = &self.structure else {
                return BTreeMap::new();
            };
            let state = extract_state(&StateInputs {
                document: &self.src.doc,
                spec: &self.src.spec,
                base_url: &self.src.base_url,
                snapshot_sha: "hash:fuzz",
                structure,
                sections: &self.parsed.sections,
                idl_definitions: &self.parsed.idl_definitions,
                catalog: &StateCatalog::default(),
                body_nodes: None,
            });
            let indexes = build_slice_indexes(structure, &state);
            indexes.into_iter().map(|i| (i.anchor.clone(), i)).collect()
        });
        map.get(anchor).cloned()
    }
}
