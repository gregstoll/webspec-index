//! Query layer: transform modes (M0, M2, M4), round trips (M1), refs command (Q1), navigation
//! (N1), anchor coverage (A1, report-only).

use std::panic::{catch_unwind, AssertUnwindSafe};

use scraper::ElementRef;
use serde_json::json;
use webspec_index::content_filter::{transform_content, LinksMode};
use webspec_index::model::SectionType;

use super::{Ctx, Evidence, Failure, Invariant, Outcome, SectionCtx};
use crate::oracle::{self, Cat};

const MAX_ITEMS: usize = 40;

fn section<'c, 'a>(ctx: &Ctx<'c, 'a>) -> Option<&'c SectionCtx<'a>> {
    match ctx {
        Ctx::Section(s) => Some(s),
        Ctx::Spec(_) => None,
    }
}

pub const MODES: [(LinksMode, bool); 6] = [
    (LinksMode::Full, false),
    (LinksMode::Full, true),
    (LinksMode::Short, false),
    (LinksMode::Short, true),
    (LinksMode::None, false),
    (LinksMode::None, true),
];

pub fn mode_name(m: LinksMode, no_notes: bool) -> String {
    let l = match m {
        LinksMode::Full => "full",
        LinksMode::Short => "short",
        LinksMode::None => "none",
    };
    if no_notes {
        format!("{l}+no-notes")
    } else {
        l.to_string()
    }
}

pub fn transform(s: &SectionCtx, mode: LinksMode, no_notes: bool) -> Option<String> {
    catch_unwind(AssertUnwindSafe(|| {
        transform_content(s.content(), mode, no_notes, &s.spec.registry)
    }))
    .ok()
}

pub struct M0;
impl Invariant for M0 {
    fn id(&self) -> &'static str {
        "M0"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.content().is_empty() {
            return Outcome::NotApplicable;
        }
        let modes = panicking_modes(|m, n| transform_content(s.content(), m, n, &s.spec.registry));
        if modes.is_empty() {
            return Outcome::Pass;
        }
        let items = modes
            .iter()
            .map(|m| Evidence::new("panic", s.anchor(), m.clone()))
            .collect();
        Outcome::fail(
            items,
            json!({"panic": false}),
            json!({"panicking_modes": modes}),
        )
    }
}

/// Mode combinations in which `f` panics.
pub fn panicking_modes(f: impl Fn(LinksMode, bool) -> String) -> Vec<String> {
    MODES
        .iter()
        .filter(|(m, n)| catch_unwind(AssertUnwindSafe(|| f(*m, *n))).is_err())
        .map(|(m, n)| mode_name(*m, *n))
        .collect()
}

/// M2 on rendered strings: short mode keeps link count and words, and rewrites exactly the
/// registry-known destinations.
pub fn m2_items(
    full: &str,
    short: &str,
    registry: &webspec_index::spec_registry::SpecRegistry,
) -> Vec<Evidence> {
    let full = oracle::md_info(full);
    let ms = oracle::md_info(short);
    let mut items = Vec::new();
    if ms.links.len() != full.links.len() {
        items.push(Evidence::new(
            "link-count",
            format!("full={} short={}", full.links.len(), ms.links.len()),
            "count",
        ));
    }
    if oracle::words(&ms.text) != oracle::words(&full.text) {
        items.push(Evidence::new("words", "short-mode text differs", "text"));
    }
    for (f, sh) in full.links.iter().zip(ms.links.iter()) {
        let expect = registry
            .resolve_url(f)
            .map(|(a, b)| format!("{a}#{b}"))
            .unwrap_or_else(|| f.clone());
        if &expect != sh && items.len() < MAX_ITEMS {
            items.push(
                Evidence::new("rewrite", format!("{f} -> {sh}"), "link").rendered(Some(expect)),
            );
        }
    }
    items
}

/// M4a on rendered strings: `--no-notes` removes exactly the links of labelled note blockquotes.
pub fn m4a_items(full: &str, no_notes: &str) -> Vec<Evidence> {
    let full = oracle::md_info(full);
    let mn = oracle::md_info(no_notes);
    let expected = oracle::missing(&full.links, &full.note_links);
    let mut items = Vec::new();
    for u in oracle::missing(&expected, &mn.links).iter().take(MAX_ITEMS) {
        items.push(
            Evidence::new("no-notes-lost", u.clone(), "M4a")
                .rendered(oracle::rendered_excerpt(no_notes, u).map(|x| oracle::excerpt(&x, 300))),
        );
    }
    for u in oracle::missing(&mn.links, &expected).iter().take(MAX_ITEMS) {
        items.push(Evidence::new("no-notes-kept", u.clone(), "M4a"));
    }
    items
}

/// Q1 on ref lists: the refs command returns exactly the library's outgoing targets.
pub fn q1_items(lib: &[(String, String)], refs: &[(String, String)]) -> Vec<Evidence> {
    let mut want = lib.to_vec();
    let mut got: Vec<(String, String)> = Vec::new();
    for t in refs {
        if !got.contains(t) {
            got.push(t.clone());
        }
    }
    want.sort();
    got.sort();
    if want == got {
        Vec::new()
    } else {
        vec![Evidence::new(
            "refs-mismatch",
            format!("lib={} refs={}", want.len(), got.len()),
            "refs",
        )]
    }
}

pub struct M1;
impl Invariant for M1 {
    fn id(&self) -> &'static str {
        "M1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let spec = s.spec.spec();
        let want = (spec.to_string(), s.anchor().to_string());
        let url = s.spec.section_url(s.anchor());
        let short = format!("{spec}#{}", s.anchor());
        let mut items = Vec::new();
        let mut parsed = Vec::new();
        for (kind, input) in [("url", &url), ("short", &short)] {
            match webspec_index::parse_spec_anchor(input) {
                Ok((sp, an, _)) => {
                    if (sp.clone(), an.clone()) != want {
                        items.push(
                            Evidence::new(&format!("{kind}-roundtrip"), input.clone(), kind)
                                .rendered(Some(format!("{sp}#{an}"))),
                        );
                    }
                    parsed.push((sp, an));
                }
                Err(e) => items.push(
                    Evidence::new(&format!("{kind}-roundtrip"), input.clone(), kind)
                        .rendered(Some(e.to_string())),
                ),
            }
        }
        if let (Some(db), [a, b]) = (&s.spec.db, parsed.as_slice()) {
            let q = |p: &(String, String)| {
                webspec_index::query_section_from_conn(&db.conn, &p.0, &p.1)
                    .map(|r| serde_json::to_string(&r).unwrap_or_default())
                    .map_err(|e| e.to_string())
            };
            let (qa, qb) = (q(a), q(b));
            if qa != qb || matches!(qa, Ok(ref x) if x == "null") {
                let show = |r: &Result<String, String>| oracle::excerpt(&format!("{r:?}"), 200);
                items.push(
                    Evidence::new("query-identity", short.clone(), "query")
                        .rendered(Some(format!("url: {} | short: {}", show(&qa), show(&qb)))),
                );
            }
        }
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({"spec": spec, "anchor": s.anchor()}),
            actual: json!({"url": url}),
        })
    }
}

pub struct M2;
impl Invariant for M2 {
    fn id(&self) -> &'static str {
        "M2"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.content().is_empty() {
            return Outcome::NotApplicable;
        }
        let Some(short) = transform(s, LinksMode::Short, false) else {
            return Outcome::NotApplicable;
        };
        let items = m2_items(s.content(), &short, &s.spec.registry);
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({"links": s.md().links.len()}),
            actual: json!({"short": oracle::excerpt(&short, 2000)}),
        })
    }
}

pub struct M4;
impl Invariant for M4 {
    fn id(&self) -> &'static str {
        "M4"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.content().is_empty() || s.ty() == SectionType::Idl {
            return Outcome::NotApplicable;
        }
        let Some(nn) = transform(s, LinksMode::Full, true) else {
            return Outcome::NotApplicable;
        };
        let full = s.md();
        let mut items = m4a_items(s.content(), &nn);
        if s.region.shape.has_region() && !full.note_links.is_empty() {
            let extent;
            let links = if s.region.shape.is_heading() {
                extent = oracle::walk(&s.spec.src, &oracle::heading_extent(s.el));
                &extent.links
            } else {
                &s.src_text().links
            };
            let src_note = callout_links(s);
            for u in oracle::missing(&full.note_links, &src_note)
                .iter()
                .take(MAX_ITEMS)
            {
                let construct = match links.iter().find(|l| &l.abs == u) {
                    Some(l) => s.construct(l.node),
                    None => "not-in-region".into(),
                };
                items.push(
                    Evidence::new(
                        "note-link-not-from-callout",
                        u.clone(),
                        format!("M4b:{construct}"),
                    )
                    .rendered(
                        oracle::rendered_excerpt(s.content(), u).map(|x| oracle::excerpt(&x, 300)),
                    ),
                );
            }
        }
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({"links": full.links.len(), "note_links": full.note_links.len()}),
            actual: json!({"no_notes": oracle::excerpt(&nn, 2000)}),
        })
    }
}

/// Links inside source callouts, where labelled note blockquotes may take links from. Heading
/// content also renders its subsections, so their callouts count too.
pub fn callout_links(s: &SectionCtx) -> Vec<String> {
    let links = if s.region.shape.is_heading() {
        oracle::walk(&s.spec.src, &oracle::heading_extent(s.el)).links
    } else {
        oracle::walk(&s.spec.src, &s.region.roots).links
    };
    links
        .into_iter()
        .filter(|l| l.cat == Cat::InNote)
        .map(|l| l.abs)
        .collect()
}

pub struct Q1;
impl Invariant for Q1 {
    fn id(&self) -> &'static str {
        "Q1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let Some(db) = &s.spec.db else {
            return Outcome::NotApplicable;
        };
        let Ok(lib) =
            webspec_index::db::queries::get_outgoing_refs(&db.conn, db.snapshot_id, s.anchor())
        else {
            return Outcome::NotApplicable;
        };
        if lib.len() < webspec_index::format::REFS_LIST_THRESHOLD {
            return Outcome::NotApplicable;
        }
        let spec = s.spec.spec();
        let id = format!("{spec}#{}", s.anchor());
        let mut items = Vec::new();
        match webspec_index::find_references_from_conn(
            &db.conn,
            Some((spec.to_string(), s.anchor().to_string())),
            &id,
            "outgoing",
            lib.len() as u32,
            None,
        ) {
            Ok(r) => {
                let got: Vec<(String, String)> = r
                    .matches
                    .iter()
                    .flat_map(|m| m.outgoing.iter().flatten())
                    .map(|e| (e.spec.clone(), e.anchor.clone()))
                    .collect();
                items.extend(q1_items(&lib, &got));
            }
            Err(e) => items.push(Evidence::new("refs-error", e.to_string(), "refs")),
        }
        if let Some(q) = s.query_result() {
            let cmd =
                webspec_index::format::refs_command(&id, "outgoing", q.outgoing_refs.len(), None);
            if q.outgoing_refs.len() != lib.len()
                || !webspec_index::format::query(q, None).contains(&cmd)
            {
                items.push(Evidence::new("query-command", cmd, "format"));
            }
        }
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({"outgoing": lib.len()}),
            actual: json!({}),
        })
    }
}

pub struct N1;
impl Invariant for N1 {
    fn id(&self) -> &'static str {
        "N1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let mut items = Vec::new();
        if let Some(n) = &s.section.next_anchor {
            let back = s.spec.section(n).and_then(|x| x.prev_anchor.as_deref());
            if back != Some(s.anchor()) {
                items.push(
                    Evidence::new("next-prev", format!("{} -> {n}", s.anchor()), "next")
                        .rendered(Some(format!("{n}.prev = {back:?}"))),
                );
            }
        }
        if let Some(p) = &s.section.parent_anchor {
            if s.spec.section(p).is_none() {
                items.push(Evidence::new(
                    "parent-missing",
                    format!("{} -> {p}", s.anchor()),
                    "parent",
                ));
            }
        }
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({}),
            actual: json!({"next": s.section.next_anchor, "parent": s.section.parent_anchor}),
        })
    }
}

/// Why an anchor is not a section, from the element carrying it.
fn anchor_marker(s: &SectionCtx, spec: &str, anchor: &str) -> String {
    if spec != s.spec.spec() {
        return match s.spec.db.as_ref().map(|db| {
            webspec_index::db::queries::get_snapshot(&db.conn, spec)
                .ok()
                .flatten()
                .is_some()
        }) {
            Some(true) => "cross-spec:anchor-missing".into(),
            Some(false) => "cross-spec:spec-not-indexed".into(),
            None => "cross-spec".into(),
        };
    }
    let Some(e) = s.spec.src.id_element(anchor) else {
        return "no-id".into();
    };
    dfn_marker(&e).unwrap_or_else(|| e.value().name().to_string())
}

fn dfn_marker(e: &ElementRef) -> Option<String> {
    if e.value().name() != "dfn" {
        return None;
    }
    let param = e
        .children()
        .filter_map(ElementRef::wrap)
        .any(|c| c.value().name() == "var")
        || e.value().attr("data-dfn-type") == Some("argument");
    let anc = |names: &[&str]| {
        e.ancestors()
            .filter_map(ElementRef::wrap)
            .any(|a| names.contains(&a.value().name()))
    };
    Some(
        if param {
            "dfn-parameter"
        } else if anc(&["emu-clause", "emu-annex"]) {
            "dfn-in-emu-clause"
        } else if anc(&["ol"]) {
            "dfn-in-steps"
        } else {
            "dfn-other"
        }
        .to_string(),
    )
}

pub struct A1;
impl Invariant for A1 {
    fn id(&self) -> &'static str {
        "A1"
    }
    fn report_only(&self) -> bool {
        true
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        let spec = s.spec.spec().to_string();
        let mut items = Vec::new();
        let mut push = |kind: &str, target: String, marker: String| {
            if items.len() < MAX_ITEMS {
                items.push(Evidence::new(kind, target, marker));
            }
        };
        for r in &s.region.roots {
            for e in r.descendants().filter_map(ElementRef::wrap) {
                if e.value().name() != "dfn" {
                    continue;
                }
                let Some(id) = e.value().attr("id") else {
                    continue;
                };
                if id != s.anchor() && !s.spec.first.contains_key(id) {
                    push(
                        "dfn-in-region",
                        format!("{spec}#{id}"),
                        dfn_marker(&e).unwrap_or_default(),
                    );
                }
            }
        }
        if let Some(refs) = s.spec.refs_by_from.get(s.anchor()) {
            for (ts, ta) in refs {
                if ts == &spec && !s.spec.first.contains_key(ta) {
                    push(
                        "intra-ref-target",
                        format!("{ts}#{ta}"),
                        anchor_marker(s, ts, ta),
                    );
                }
            }
        }
        if !s.content().is_empty() {
            if let Some(short) = transform(s, LinksMode::Short, false) {
                let mut seen = std::collections::HashSet::new();
                for l in oracle::md_info(&short).links {
                    if l.contains("://") || !seen.insert(l.clone()) {
                        continue;
                    }
                    let Some((sp, an)) = l.split_once('#') else {
                        continue;
                    };
                    if s.spec.exists(sp, an) == Some(false) {
                        push("short-link", l.clone(), anchor_marker(s, sp, an));
                    }
                }
            }
        }
        Outcome::check(items.is_empty(), || Failure {
            items,
            expected: json!({}),
            actual: json!({}),
        })
    }
}
