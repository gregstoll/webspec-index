//! Ratchet over findings promoted by the invariant fuzz harness (`examples/spec_fuzz`).
//!
//! Each `tests/fixtures/fuzz/<name>.json` names a section of `<name>.html`, the invariant it
//! exercises, the values the harness's oracle expects, and a status:
//!
//! - `fixed`: the invariant must hold. A failure is a regression.
//! - `known_bug`: the invariant must still fail. A pass means the bug was fixed: flip the fixture to
//!   `fixed` in the same change.
//!
//! The checks here use only the product and pulldown-cmark, never harness code, so fixtures stay
//! valid while the oracle evolves.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;
use webspec_index::content_filter::{transform_content, LinksMode};
use webspec_index::model::{ParsedSection, ParsedSpec};
use webspec_index::spec_registry::SpecRegistry;
use webspec_index::state::slice::render::{render_view, Rendering};
use webspec_index::state::slice::select::{slice as slice_fn, StepRole, ViewRequest};
use webspec_index::state::testing::slice_indexes_html;

#[derive(serde::Deserialize)]
struct Fixture {
    spec: String,
    base_url: String,
    anchor: String,
    invariant: String,
    status: String,
    bug: String,
    expected: Value,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fuzz")
}

// ── Markdown scan ───────────────────────────────────────────────────────────────────────────────

const NOTE_LABELS: [&str; 4] = ["Note:", "Example:", "Warning:", "Issue:"];

#[derive(Default)]
struct Md {
    links: Vec<String>,
    note_links: Vec<String>,
    text: String,
    ol_items: usize,
    ol_depth: usize,
}

fn scan(md: &str) -> Md {
    let mut out = Md::default();
    let mut lists: Vec<bool> = Vec::new();
    let mut notes: Vec<bool> = Vec::new();
    let mut first_text = false;
    for ev in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => {
                notes.push(false);
                first_text = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                notes.pop();
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                if notes.iter().any(|n| *n) {
                    out.note_links.push(dest_url.to_string());
                }
                out.links.push(dest_url.to_string());
            }
            Event::Start(Tag::List(start)) => lists.push(start.is_some()),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                out.text.push(' ');
                if lists.last() == Some(&true) {
                    out.ol_items += 1;
                    out.ol_depth = out.ol_depth.max(lists.iter().filter(|o| **o).count());
                }
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                if !notes.is_empty() && first_text {
                    first_text = false;
                    if NOTE_LABELS.contains(&t.trim()) {
                        *notes.last_mut().unwrap() = true;
                        continue;
                    }
                }
                out.text.push_str(&t);
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::CodeBlock,
            )
            | Event::Start(Tag::Paragraph | Tag::TableCell)
            | Event::SoftBreak
            | Event::HardBreak => out.text.push(' '),
            _ => {}
        }
    }
    out
}

/// Ordered-item paths outside blockquotes; each component is the list's start number plus the
/// item's index in that list.
fn rendered_paths(md: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut quote_depth = 0usize;
    let mut lists: Vec<(Option<u64>, u64)> = Vec::new();
    let mut open: Vec<Option<usize>> = Vec::new();
    for ev in Parser::new_ext(md, Options::all()) {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote_depth -= 1,
            Event::Start(Tag::List(start)) => lists.push((start, 0)),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let Some((Some(start), seen)) = lists.last_mut() else {
                    open.push(None);
                    continue;
                };
                let number = *start + *seen;
                *seen += 1;
                if quote_depth > 0 {
                    open.push(None);
                    continue;
                }
                let path = match open.iter().rev().find_map(|o| *o) {
                    Some(parent) => format!("{}.{number}", paths[parent]),
                    None => number.to_string(),
                };
                open.push(Some(paths.len()));
                paths.push(path);
            }
            Event::End(TagEnd::Item) => {
                open.pop();
            }
            _ => {}
        }
    }
    paths
}

/// `^\s*- \[steps? .* omitted`
fn is_marker_line(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("- [step") else {
        return false;
    };
    let rest = rest.strip_prefix('s').unwrap_or(rest);
    rest.strip_prefix(' ')
        .is_some_and(|tail| tail.contains(" omitted"))
}

fn words(t: &str) -> Vec<String> {
    t.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Multiset difference `want - have`.
fn missing(want: &[String], have: &[String]) -> Vec<String> {
    let mut count: HashMap<&str, i64> = HashMap::new();
    for h in have {
        *count.entry(h).or_default() += 1;
    }
    let mut out = Vec::new();
    for w in want {
        let c = count.entry(w).or_default();
        if *c > 0 {
            *c -= 1;
        } else {
            out.push(w.clone());
        }
    }
    out
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn usize_at(v: &Value, key: &str) -> usize {
    v[key].as_u64().unwrap_or(0) as usize
}

// ── Product views ───────────────────────────────────────────────────────────────────────────────

struct Product {
    parsed: ParsedSpec,
    first: HashMap<String, usize>,
}

impl Product {
    fn new(f: &Fixture, html: &str) -> Self {
        let parsed =
            webspec_index::parse::parse_spec(html, &f.spec, &f.base_url).expect("parse_spec");
        let mut first = HashMap::new();
        for (i, s) in parsed.sections.iter().enumerate() {
            first.entry(s.anchor.clone()).or_insert(i);
        }
        Product { parsed, first }
    }

    fn section(&self, anchor: &str) -> Option<&ParsedSection> {
        self.first.get(anchor).map(|i| &self.parsed.sections[*i])
    }
}

fn absolutize(href: &str, base: &str) -> String {
    if href.starts_with('#') {
        format!("{base}{href}")
    } else {
        href.to_string()
    }
}

/// The IR algorithms of `anchor`, serialized, so the check survives IR type changes.
fn ir_algorithms(f: &Fixture, html: &str) -> Vec<Value> {
    let st = webspec_index::parse::steps::extract_step_structure(
        html,
        &f.spec,
        &f.base_url,
        "hash:fixture",
    );
    let v = serde_json::to_value(st).expect("structure serializes");
    v["algorithms"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["source"]["section_anchor"].as_str() == Some(f.anchor.as_str()))
        .cloned()
        .collect()
}

/// Link spans anywhere in the algorithms, deduplicated by id, without the IR's `aoid:` aliases.
fn ir_links(algs: &[Value], base: &str) -> Vec<String> {
    fn rec(v: &Value, seen: &mut HashSet<String>, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let (Some(id), Some(href), Some(_)) = (
                    m.get("id").and_then(Value::as_str),
                    m.get("href").and_then(Value::as_str),
                    m.get("span"),
                ) {
                    if seen.insert(id.to_string()) {
                        out.push(href.to_string());
                    }
                }
                m.values().for_each(|x| rec(x, seen, out));
            }
            Value::Array(a) => a.iter().for_each(|x| rec(x, seen, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for a in algs {
        rec(a, &mut HashSet::new(), &mut out);
    }
    out.into_iter()
        .filter(|h| !h.starts_with("aoid:"))
        .map(|h| absolutize(&h, base))
        .collect()
}

const MODES: [(LinksMode, bool); 6] = [
    (LinksMode::Full, false),
    (LinksMode::Full, true),
    (LinksMode::Short, false),
    (LinksMode::Short, true),
    (LinksMode::None, false),
    (LinksMode::None, true),
];

fn section_url(f: &Fixture) -> String {
    if f.base_url.ends_with(".html") {
        format!("{}#{}", f.base_url, f.anchor)
    } else {
        format!("{}/#{}", f.base_url, f.anchor)
    }
}

// ── Verdict ─────────────────────────────────────────────────────────────────────────────────────

/// `Ok` when the fixture's invariant holds on the product, `Err(reason)` when it fails.
fn evaluate(f: &Fixture, html: &str) -> Result<(), String> {
    let product = Product::new(f, html);
    let section = product
        .section(&f.anchor)
        .ok_or_else(|| format!("#{} is not a section", f.anchor))?;
    let content = section.content_text.clone().unwrap_or_default();
    let md = scan(&content);
    let e = &f.expected;
    let registry = SpecRegistry::new();
    let fail = |why: String| -> Result<(), String> { Err(why) };
    match f.invariant.as_str() {
        "C1" => {
            let want = strings(e, "links");
            let lost = missing(&want, &md.links);
            let extra = if e["one_sided"].as_bool() == Some(true) {
                Vec::new()
            } else {
                missing(&md.links, &want)
            };
            if lost.is_empty() && extra.is_empty() {
                Ok(())
            } else {
                fail(format!("links missing {lost:?}, extra {extra:?}"))
            }
        }
        "C2" => {
            let lost = missing(&strings(e, "words"), &words(&md.text));
            if lost.is_empty() {
                Ok(())
            } else {
                fail(format!("words missing {lost:?}"))
            }
        }
        "C3" => {
            if content.trim().is_empty() {
                fail("empty content".into())
            } else {
                Ok(())
            }
        }
        "S1" => {
            let want = (usize_at(e, "steps"), usize_at(e, "depth"));
            if (md.ol_items, md.ol_depth) == want {
                Ok(())
            } else {
                fail(format!(
                    "markdown steps {:?}, expected {want:?}",
                    (md.ol_items, md.ol_depth)
                ))
            }
        }
        "S2" => {
            let algs = ir_algorithms(f, html);
            if algs.is_empty() {
                return fail("no IR algorithm".into());
            }
            let steps: usize = algs
                .iter()
                .map(|a| a["steps"].as_array().map_or(0, Vec::len))
                .sum();
            let depth = algs
                .iter()
                .flat_map(|a| a["steps"].as_array().cloned().unwrap_or_default())
                .map(|s| s["path"].as_array().map_or(0, Vec::len))
                .max()
                .unwrap_or(0);
            let want = (usize_at(e, "steps"), usize_at(e, "depth"));
            if (steps, depth) == want {
                Ok(())
            } else {
                fail(format!("IR steps {:?}, expected {want:?}", (steps, depth)))
            }
        }
        "S3" => {
            let want = strings(e, "links");
            let got = ir_links(&ir_algorithms(f, html), &f.base_url);
            let (lost, extra) = (missing(&want, &got), missing(&got, &want));
            if lost.is_empty() && extra.is_empty() {
                Ok(())
            } else {
                fail(format!("IR links missing {lost:?}, extra {extra:?}"))
            }
        }
        "M0" => {
            let panics: Vec<String> = MODES
                .iter()
                .filter(|(m, n)| {
                    catch_unwind(AssertUnwindSafe(|| {
                        transform_content(&content, *m, *n, &registry)
                    }))
                    .is_err()
                })
                .map(|(m, n)| format!("{m:?} no_notes={n}"))
                .collect();
            if panics.is_empty() {
                Ok(())
            } else {
                fail(format!("transform_content panics in {panics:?}"))
            }
        }
        "M1" => {
            let want = (f.spec.clone(), f.anchor.clone());
            for input in [section_url(f), format!("{}#{}", f.spec, f.anchor)] {
                match webspec_index::parse_spec_anchor(&input) {
                    Ok((s, a, _)) if (s.clone(), a.clone()) == want => {}
                    Ok((s, a, _)) => return fail(format!("{input} parses as {s}#{a}")),
                    Err(err) => return fail(format!("{input}: {err}")),
                }
            }
            Ok(())
        }
        "M2" => {
            let short = scan(&transform_content(
                &content,
                LinksMode::Short,
                false,
                &registry,
            ));
            if short.links.len() != md.links.len() {
                return fail(format!(
                    "short mode has {} links, full {}",
                    short.links.len(),
                    md.links.len()
                ));
            }
            if words(&short.text) != words(&md.text) {
                return fail("short mode changes the text".into());
            }
            for (full, sh) in md.links.iter().zip(&short.links) {
                let want = registry
                    .resolve_url(full)
                    .map(|(s, a)| format!("{s}#{a}"))
                    .unwrap_or_else(|| full.clone());
                if &want != sh {
                    return fail(format!("{full} rendered as {sh}, expected {want}"));
                }
            }
            Ok(())
        }
        "M4" => {
            let nn = scan(&transform_content(
                &content,
                LinksMode::Full,
                true,
                &registry,
            ));
            let want = missing(&md.links, &md.note_links);
            let (lost, kept) = (missing(&want, &nn.links), missing(&nn.links, &want));
            let stray = missing(&md.note_links, &strings(e, "callout_links"));
            if lost.is_empty() && kept.is_empty() && stray.is_empty() {
                Ok(())
            } else {
                fail(format!("--no-notes lost {lost:?}, kept {kept:?}; note links outside callouts {stray:?}"))
            }
        }
        "R1" | "R2" => {
            let refs: HashSet<String> = product
                .parsed
                .references
                .iter()
                .filter(|r| r.from_anchor == f.anchor)
                .map(|r| format!("{}#{}", r.to_spec, r.to_anchor))
                .collect();
            let want: HashSet<String> = strings(e, "targets").into_iter().collect();
            let ok = if f.invariant == "R1" {
                want.is_subset(&refs)
            } else {
                want == refs
            };
            if ok {
                Ok(())
            } else {
                fail(format!("refs {refs:?}, expected {want:?}"))
            }
        }
        "N1" => {
            if let Some(next) = &section.next_anchor {
                let back = product.section(next).and_then(|s| s.prev_anchor.clone());
                if back.as_deref() != Some(f.anchor.as_str()) {
                    return fail(format!("next {next} has prev {back:?}"));
                }
            }
            if let Some(p) = &section.parent_anchor {
                if product.section(p).is_none() {
                    return fail(format!("parent {p} is not a section"));
                }
            }
            Ok(())
        }
        "T1" => {
            let ty = section.section_type.as_str();
            if Some(ty) == e["not_section_type"].as_str() {
                fail(format!("typed {ty}"))
            } else {
                Ok(())
            }
        }
        "A1" => {
            let unindexed: Vec<String> = strings(e, "anchors")
                .into_iter()
                .filter(|a| {
                    let anchor = a.split_once('#').map_or(a.as_str(), |(_, x)| x);
                    product.section(anchor).is_none()
                })
                .collect();
            if unindexed.is_empty() {
                Ok(())
            } else {
                fail(format!("not sections: {unindexed:?}"))
            }
        }
        "D1" => {
            // A fresh parse must reproduce what the index stored: the recorded rendering, and the
            // same structure on every extraction.
            if e.get("content").is_some()
                && section.content_text.as_deref() != e["content"].as_str()
            {
                return fail(format!(
                    "content differs from the recorded rendering:\n{content}"
                ));
            }
            if e["structure_deterministic"].as_bool() == Some(true) {
                let extract = || {
                    serde_json::to_string(&webspec_index::parse::steps::extract_step_structure(
                        html,
                        &f.spec,
                        &f.base_url,
                        "hash:fixture",
                    ))
                    .expect("structure serializes")
                };
                let first = extract();
                if (0..3).any(|_| extract() != first) {
                    return fail(
                        "extract_step_structure differs between runs on the same input".into(),
                    );
                }
            }
            Ok(())
        }
        "L1" => {
            let indexes = slice_indexes_html(html, &f.spec);
            let index = indexes
                .iter()
                .find(|i| i.anchor == f.anchor)
                .ok_or_else(|| format!("no slice index for #{}", f.anchor))?;
            let var_name = e["variable"].as_str().ok_or("expected.variable missing")?;
            let want_paths: HashSet<String> = strings(e, "paths").into_iter().collect();
            let view_req = ViewRequest {
                involving: vec![var_name.to_string()],
                ..Default::default()
            };
            let s = slice_fn(index, &view_req).map_err(|e| e.to_string())?;
            let got_paths: HashSet<String> = s
                .steps
                .iter()
                .filter(|k| matches!(k.role, StepRole::Match | StepRole::Inherited))
                .map(|k| k.path.clone())
                .collect();
            if got_paths != want_paths {
                return fail(format!(
                    "L1 {var_name}: paths {got_paths:?}, want {want_paths:?}"
                ));
            }
            Ok(())
        }
        "L2" => {
            let indexes = slice_indexes_html(html, &f.spec);
            let index = indexes
                .iter()
                .find(|i| i.anchor == f.anchor)
                .ok_or_else(|| format!("no slice index for #{}", f.anchor))?;
            let total = index.steps.len();
            for var_name in &index.vars {
                let view_req = ViewRequest {
                    involving: vec![var_name.clone()],
                    ..Default::default()
                };
                let s = slice_fn(index, &view_req).map_err(|e| e.to_string())?;
                let kept = s.steps.len();
                let omitted: usize = s.omitted.iter().map(|r| r.steps as usize).sum();
                if kept + omitted != total {
                    return fail(format!("L2 {var_name}: count:{kept}+{omitted}!={total}"));
                }
            }
            Ok(())
        }
        "L3" => {
            let indexes = slice_indexes_html(html, &f.spec);
            let index = indexes
                .iter()
                .find(|i| i.anchor == f.anchor)
                .ok_or_else(|| format!("no slice index for #{}", f.anchor))?;
            for var_name in &index.vars {
                let view_req = ViewRequest {
                    involving: vec![var_name.clone()],
                    ..Default::default()
                };
                let s = slice_fn(index, &view_req).map_err(|e| e.to_string())?;
                let kept_set: HashSet<&str> = s.steps.iter().map(|k| k.path.as_str()).collect();
                for step in &s.steps {
                    let parts: Vec<&str> = step.path.split('.').collect();
                    for depth in 1..parts.len() {
                        let prefix = parts[..depth].join(".");
                        if !kept_set.contains(prefix.as_str()) {
                            return fail(format!(
                                "L3 {var_name}: prefix {prefix} missing for kept {}",
                                step.path
                            ));
                        }
                    }
                }
                for run in &s.omitted {
                    if let Some(parent) = &run.parent {
                        if !kept_set.contains(parent.as_str()) {
                            return fail(format!("L3 {var_name}: run parent {parent} not kept"));
                        }
                    }
                }
            }
            Ok(())
        }
        "L4" => {
            let indexes = slice_indexes_html(html, &f.spec);
            let index = indexes
                .iter()
                .find(|i| i.anchor == f.anchor)
                .ok_or_else(|| format!("no slice index for #{}", f.anchor))?;
            for var_name in &index.vars {
                let view_req = ViewRequest {
                    involving: vec![var_name.clone()],
                    ..Default::default()
                };
                let s = slice_fn(index, &view_req).map_err(|e| e.to_string())?;
                let view = render_view(&content, index, &s);
                if view.rendering == Rendering::Unaligned {
                    return Ok(());
                }
                let markers = view.content.lines().filter(|l| is_marker_line(l)).count();
                if markers != s.omitted.len() {
                    return fail(format!(
                        "L4 {var_name}: markers:{markers}!={}",
                        s.omitted.len()
                    ));
                }
                let rendered = rendered_paths(&view.content);
                let kept: Vec<&str> = s.steps.iter().map(|k| k.path.as_str()).collect();
                if rendered != kept {
                    return fail(format!(
                        "L4 {var_name}: paths:{}!={}",
                        rendered.join(","),
                        kept.join(",")
                    ));
                }
            }
            Ok(())
        }
        "L5" => {
            let indexes = slice_indexes_html(html, &f.spec);
            let index = indexes
                .iter()
                .find(|i| i.anchor == f.anchor)
                .ok_or_else(|| format!("no slice index for #{}", f.anchor))?;
            for l in e["lets"].as_array().ok_or("expected.lets missing")? {
                let (step, var, from) = (
                    l["step"].as_str().unwrap_or_default(),
                    l["var"].as_str().unwrap_or_default(),
                    l["from"].as_str().unwrap_or_default(),
                );
                let view_req = ViewRequest {
                    involving: vec![from.to_string()],
                    ..Default::default()
                };
                let s = slice_fn(index, &view_req).map_err(|e| e.to_string())?;
                let kept: HashSet<&str> = s.steps.iter().map(|k| k.path.as_str()).collect();
                if let Some(path) = strings(l, "uses")
                    .into_iter()
                    .find(|p| !kept.contains(p.as_str()))
                {
                    return fail(format!("let:{step}:{var}<-{from}:missing:{path}"));
                }
            }
            Ok(())
        }
        other => fail(format!("no check for invariant {other}")),
    }
}

#[test]
fn fuzz_regressions() {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("tests/fixtures/fuzz")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fuzz fixtures");
    let mut problems = Vec::new();
    for path in &paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let f: Fixture = serde_json::from_str(&std::fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{name}.json: {e}"));
        let html = std::fs::read_to_string(path.with_extension("html"))
            .unwrap_or_else(|e| panic!("{name}.html: {e}"));
        if f.bug.trim().is_empty() {
            problems.push(format!("{name}: no bug description"));
        }
        let verdict = evaluate(&f, &html);
        match (f.status.as_str(), verdict) {
            ("fixed", Ok(())) | ("known_bug", Err(_)) => {}
            ("fixed", Err(why)) => problems.push(format!("{name}: REGRESSED ({}): {why}", f.bug)),
            ("known_bug", Ok(())) => problems.push(format!(
                "{name}: no longer reproduces ({}): flip its status to \"fixed\"",
                f.bug
            )),
            (other, _) => problems.push(format!("{name}: unknown status {other}")),
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
