//! References (R1, R2), algorithm steps (S1, S2), IR links (S3), classification (T1, report-only).

use std::collections::BTreeSet;

use scraper::ElementRef;
use serde_json::json;
use webspec_index::model::SectionType;

use super::{Ctx, Evidence, Failure, Invariant, Outcome, SectionCtx};
use crate::oracle::{self, Shape, N};

const MAX_ITEMS: usize = 40;

fn section<'c, 'a>(ctx: &Ctx<'c, 'a>) -> Option<&'c SectionCtx<'a>> {
    match ctx {
        Ctx::Section(s) => Some(s),
        Ctx::Spec(_) => None,
    }
}

fn is_scope_section(s: &SectionCtx) -> bool {
    matches!(s.ty(), SectionType::Heading | SectionType::Algorithm)
}

fn in_ranges(pos: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|(start, end)| pos > *start && pos < *end)
}

fn outgoing(s: &SectionCtx) -> BTreeSet<(String, String)> {
    s.spec
        .refs_by_from
        .get(s.anchor())
        .cloned()
        .unwrap_or_default()
}

fn fmt_target(t: &(String, String)) -> String {
    format!("{}#{}", t.0, t.1)
}

/// Resolvable reference links of the region that lie inside the section's reference scope.
fn region_scope_links<'a>(s: &SectionCtx<'a>) -> Option<Vec<((String, String), N<'a>)>> {
    if !is_scope_section(s) || !s.region.shape.has_region() {
        return None;
    }
    let ranges = s.spec.scope_ranges.get(s.anchor())?;
    Some(
        s.src_text()
            .links
            .iter()
            .filter(|l| l.cat.is_ref_link() && in_ranges(s.spec.src.pos(&l.node), ranges))
            .filter_map(|l| Some((s.spec.resolve(&l.href)?, l.node)))
            .collect(),
    )
}

/// Every resolvable reference link in the section's scope, by an independent document-order walk.
fn scope_links(s: &SectionCtx) -> Option<BTreeSet<(String, String)>> {
    if !is_scope_section(s) {
        return None;
    }
    let ranges = s.spec.scope_ranges.get(s.anchor())?;
    let src = &s.spec.src;
    let mut out = BTreeSet::new();
    for (start, end) in ranges {
        for pos in *start..*end {
            let Some(e) = ElementRef::wrap(src.node_at(pos)) else {
                continue;
            };
            if e.value().name() != "a"
                || oracle::has_class(&e, "self-link")
                || e.value().attr("data-link-type") == Some("biblio")
            {
                continue;
            }
            if let Some(t) = e.value().attr("href").and_then(|h| s.spec.resolve(h)) {
                out.insert(t);
            }
        }
    }
    Some(out)
}

/// R1's (region links in scope) or R2's (all scope links) targets as `SPEC#anchor`.
pub fn scope_targets(s: &SectionCtx, region_only: bool) -> Vec<String> {
    let set: BTreeSet<(String, String)> = if region_only {
        region_scope_links(s)
            .unwrap_or_default()
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    } else {
        scope_links(s).unwrap_or_default()
    };
    set.iter().map(fmt_target).collect()
}

pub struct R1;
impl Invariant for R1 {
    fn id(&self) -> &'static str {
        "R1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(links) = region_scope_links(s) else {
            return Outcome::NotApplicable;
        };
        let refs = outgoing(s);
        let mut items = Vec::new();
        let mut seen = BTreeSet::new();
        for (t, node) in links {
            if !refs.contains(&t) && seen.insert(t.clone()) && items.len() < MAX_ITEMS {
                items.push(Evidence::new(
                    "not-a-ref",
                    fmt_target(&t),
                    s.construct(node),
                ));
            }
        }
        if items.is_empty() {
            return Outcome::Pass;
        }
        Outcome::fail(
            items,
            json!({"missing": seen.iter().map(fmt_target).collect::<Vec<_>>()}),
            json!({"refs": refs.len()}),
        )
    }
}

pub struct R2;
impl Invariant for R2 {
    fn id(&self) -> &'static str {
        "R2"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(scope_links) = scope_links(s) else {
            return Outcome::NotApplicable;
        };
        let refs = outgoing(s);
        if scope_links == refs {
            return Outcome::Pass;
        }
        let mut items: Vec<Evidence> = scope_links
            .difference(&refs)
            .take(MAX_ITEMS)
            .map(|t| Evidence::new("scope-link-not-ref", fmt_target(t), "scope"))
            .collect();
        items.extend(
            refs.difference(&scope_links)
                .take(MAX_ITEMS)
                .map(|t| Evidence::new("ref-not-in-scope", fmt_target(t), "scope")),
        );
        Outcome::fail(
            items,
            json!({"scope_links": scope_links.len()}),
            json!({"refs": refs.len()}),
        )
    }
}

/// Source step count and depth of an algorithm's step body (S1/S2's expected side).
pub fn step_counts(s: &SectionCtx) -> (usize, usize) {
    oracle::steps_of(&body_ols(s))
}

/// Source step body: top-level `ol`s of an algorithm region.
fn body_ols<'a>(s: &SectionCtx<'a>) -> Vec<N<'a>> {
    if s.ty() != SectionType::Algorithm || !s.region.shape.has_region() {
        return Vec::new();
    }
    // The body must be steps: the sibling list is an `ol`, the algorithm div has an `ol` child,
    // or an `emu-alg` holds one. A `dl`/`ul` body is a switch or a property list.
    let has_ol_child = |n: &N| n.children().any(|c| oracle::elname(&c) == Some("ol"));
    let steps = match s.region.shape {
        Shape::AlgoSibling => oracle::elname(&s.region.roots[1]) == Some("ol"),
        Shape::AlgoDiv => has_ol_child(&s.region.roots[0]),
        Shape::EmuClause => s
            .region
            .roots
            .iter()
            .any(|r| oracle::elname(r) == Some("emu-alg") && has_ol_child(r)),
        _ => false,
    };
    if !steps {
        return Vec::new();
    }
    oracle::body_ols(&s.region.roots)
}

fn body_shape(s: &SectionCtx, ols: &[N]) -> String {
    let n = ols.len();
    let bucket = if n >= 3 {
        "3+".to_string()
    } else {
        n.to_string()
    };
    if s.region.shape == Shape::EmuClause {
        let algs = s
            .region
            .roots
            .iter()
            .filter(|r| oracle::elname(r) == Some("emu-alg"))
            .count();
        let a = if algs >= 3 {
            "3+".to_string()
        } else {
            algs.to_string()
        };
        format!("emu-alg={a}")
    } else {
        format!("ols={bucket}")
    }
}

fn step_kind(prefix: &str, src: (usize, usize), got: (usize, usize)) -> String {
    if got.0 < src.0 {
        format!("{prefix}-fewer-steps")
    } else if got.0 > src.0 {
        format!("{prefix}-more-steps")
    } else {
        format!("{prefix}-depth")
    }
}

pub struct S1;
impl Invariant for S1 {
    fn id(&self) -> &'static str {
        "S1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let ols = body_ols(s);
        if ols.is_empty() {
            return Outcome::NotApplicable;
        }
        let src = oracle::steps_of(&ols);
        let md = s.md();
        let got = (md.ol_items, md.ol_depth);
        Outcome::check(src == got, || Failure {
            items: vec![Evidence::new(
                &step_kind("md", src, got),
                s.anchor(),
                body_shape(s, &ols),
            )],
            expected: json!({"steps": src.0, "depth": src.1}),
            actual: json!({"steps": got.0, "depth": got.1}),
        })
    }
}

pub struct S2;
impl Invariant for S2 {
    fn id(&self) -> &'static str {
        "S2"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.spec.structure.is_none() {
            return Outcome::NotApplicable;
        }
        let ols = body_ols(s);
        if ols.is_empty() {
            return Outcome::NotApplicable;
        }
        let src = oracle::steps_of(&ols);
        let algs = s.spec.ir_algorithms(s.anchor());
        if algs.is_empty() {
            return Outcome::fail(
                vec![Evidence::new(
                    "no-ir-algorithm",
                    s.anchor(),
                    body_shape(s, &ols),
                )],
                json!({"steps": src.0, "depth": src.1}),
                json!({"algorithms": 0}),
            );
        }
        let count: usize = algs.iter().map(|a| a.steps.len()).sum();
        let depth = algs
            .iter()
            .flat_map(|a| a.steps.iter().map(|x| x.path.len()))
            .max()
            .unwrap_or(0);
        let got = (count, depth);
        Outcome::check(src == got, || Failure {
            items: vec![Evidence::new(
                &step_kind("ir", src, got),
                s.anchor(),
                body_shape(s, &ols),
            )],
            expected: json!({"steps": src.0, "depth": src.1}),
            actual: json!({"steps": got.0, "depth": got.1, "algorithms": algs.len()}),
        })
    }
}

/// IR link hrefs of the section's algorithms, absolutized. `aoid:` hrefs are the IR's own aliases
/// for ecmarkup `emu-xref` targets, recorded next to the element's real link.
pub fn ir_links(s: &SectionCtx) -> Vec<String> {
    s.spec
        .ir_algorithms(s.anchor())
        .iter()
        .flat_map(|a| {
            let v = serde_json::to_value(a).unwrap_or_default();
            oracle::ir_link_hrefs(&v)
        })
        .filter(|h| !h.starts_with("aoid:"))
        .map(|h| s.spec.src.abs(&h))
        .collect()
}

/// Links inside the step body, bibliography references included: they are links in the step
/// text, and the IR keeps them. Deleted text (`<del>`) is not part of the steps.
pub fn body_links<'a>(s: &SectionCtx<'a>) -> Vec<(String, N<'a>)> {
    let ols = body_ols(s);
    oracle::walk(&s.spec.src, &ols)
        .links
        .into_iter()
        .filter(|l| !matches!(l.cat, oracle::Cat::SelfLink | oracle::Cat::InDel))
        .map(|l| (l.abs, l.node))
        .collect()
}

pub struct S3;
impl Invariant for S3 {
    fn id(&self) -> &'static str {
        "S3"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.spec.structure.is_none()
            || body_ols(s).is_empty()
            || s.spec.ir_algorithms(s.anchor()).is_empty()
        {
            return Outcome::NotApplicable;
        }
        let src = body_links(s);
        let src_hrefs: Vec<String> = src.iter().map(|(h, _)| h.clone()).collect();
        let ir = ir_links(s);
        let miss = oracle::missing(&src_hrefs, &ir);
        let extra = oracle::missing(&ir, &src_hrefs);
        if miss.is_empty() && extra.is_empty() {
            return Outcome::Pass;
        }
        let mut items = Vec::new();
        let mut used = std::collections::HashSet::new();
        for url in miss.iter().take(MAX_ITEMS) {
            let found = src
                .iter()
                .enumerate()
                .rev()
                .find(|(i, (h, _))| h == url && !used.contains(i));
            let construct = match found {
                Some((i, (_, n))) => {
                    used.insert(i);
                    s.construct(*n)
                }
                None => "unknown".into(),
            };
            let excerpt =
                found.map(|(_, (_, n))| oracle::outer_html_excerpt(n.parent().unwrap_or(*n), 300));
            items.push(Evidence::new("missing", url.clone(), construct).source(excerpt));
        }
        for url in extra.iter().take(MAX_ITEMS) {
            items.push(Evidence::new("extra", url.clone(), "ir"));
        }
        Outcome::fail(
            items,
            json!({"missing": miss, "links": src_hrefs.len()}),
            json!({"extra": extra, "links": ir.len()}),
        )
    }
}

pub struct T1;
impl Invariant for T1 {
    fn id(&self) -> &'static str {
        "T1"
    }
    fn report_only(&self) -> bool {
        true
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.ty() != SectionType::Algorithm || s.el.value().name() != "dfn" {
            return Outcome::NotApplicable;
        }
        if oracle::is_algorithm_subject(s.el) {
            return Outcome::Pass;
        }
        let in_div =
            s.el.ancestors()
                .filter_map(ElementRef::wrap)
                .any(|e| oracle::is_algo_div(&e));
        let reason = if in_div {
            "algo-div-non-subject".to_string()
        } else {
            match oracle::sibling_list(s.el) {
                Some((_, list)) => {
                    let e = ElementRef::wrap(list).expect("element");
                    let class = ["domintro", "props", "switch"]
                        .iter()
                        .find(|c| oracle::has_class(&e, c))
                        .map(|c| format!(".{c}"))
                        .unwrap_or_default();
                    format!("list-sibling:{}{class}", e.value().name())
                }
                None => "no-body".into(),
            }
        };
        Outcome::fail(
            vec![Evidence::new("not-an-algorithm", s.anchor(), reason)
                .source(Some(oracle::excerpt(&s.region_html(), 400)))],
            json!({"section_type": "definition"}),
            json!({"section_type": "algorithm"}),
        )
    }
}
