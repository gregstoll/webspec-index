//! Invariant registry, contexts and outcomes (design §6, §13 extension point).

mod content;
pub mod effects;
mod freshness;
pub mod query;
pub mod slice;
mod structure;

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::Connection;
use scraper::ElementRef;
use serde::Serialize;
use webspec_index::model::{ParsedSection, ParsedSpec, SectionType};
use webspec_index::parse::steps::{StructuralAlgorithm, StructuralSpec};
use webspec_index::spec_registry::SpecRegistry;
use webspec_index::state::slice::SliceIndex;

use crate::index::Stored;
use crate::oracle::{self, Md, Region, SourceDoc, SrcText, N};

#[cfg(test)]
pub use effects::{check_e1, check_e2, check_e3};
#[cfg(test)]
pub use freshness::d1_diff;
pub use query::callout_links;
pub use structure::{scope_targets, step_counts};

/// Reference-bearing link hrefs inside a section's step body (S3's source side).
pub fn structure_body_links(s: &SectionCtx) -> Vec<String> {
    structure::body_links(s)
        .into_iter()
        .map(|(h, _)| h)
        .collect()
}

/// One evidence item: a missing link, a missing word, a shape string.
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq)]
pub struct Evidence {
    /// What went wrong for this item, e.g. `missing`, `extra`, `md-fewer-steps`.
    pub kind: String,
    pub item: String,
    /// Structural context of the item in the source (§9), or a shape string.
    pub construct: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_excerpt: Option<String>,
}

impl Evidence {
    pub fn new(kind: &str, item: impl Into<String>, construct: impl Into<String>) -> Self {
        Evidence {
            kind: kind.to_string(),
            item: item.into(),
            construct: construct.into(),
            source_excerpt: None,
            rendered_excerpt: None,
        }
    }
    pub fn source(mut self, s: Option<String>) -> Self {
        self.source_excerpt = s;
        self
    }
    pub fn rendered(mut self, s: Option<String>) -> Self {
        self.rendered_excerpt = s;
        self
    }
}

#[derive(Debug, Default)]
pub struct Failure {
    pub items: Vec<Evidence>,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
}

#[derive(Debug)]
pub enum Outcome {
    Pass,
    Fail(Failure),
    NotApplicable,
}

impl Outcome {
    pub fn fail(
        items: Vec<Evidence>,
        expected: serde_json::Value,
        actual: serde_json::Value,
    ) -> Self {
        Outcome::Fail(Failure {
            items,
            expected,
            actual,
        })
    }
    pub fn check(ok: bool, f: impl FnOnce() -> Failure) -> Self {
        if ok {
            Outcome::Pass
        } else {
            Outcome::Fail(f())
        }
    }
}

pub enum Ctx<'c, 'a> {
    Section(&'c SectionCtx<'a>),
    Spec(&'c SpecCtx),
}

pub trait Invariant: Sync {
    fn id(&self) -> &'static str;
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome;
    fn report_only(&self) -> bool {
        false
    }
    fn spec_level(&self) -> bool {
        false
    }
}

pub fn all() -> Vec<&'static dyn Invariant> {
    vec![
        &freshness::D1,
        &effects::E1,
        &effects::E2,
        &effects::E3,
        &content::C1,
        &content::C2,
        &content::C3,
        &structure::R1,
        &structure::R2,
        &structure::S1,
        &structure::S2,
        &structure::S3,
        &query::M0,
        &query::M1,
        &query::M2,
        &query::M4,
        &query::Q1,
        &query::N1,
        &query::A1,
        &structure::T1,
        &content::O1,
        &slice::L1,
        &slice::L2,
        &slice::L3,
        &slice::L4,
        &slice::L5,
    ]
}

pub fn by_id(id: &str) -> Option<&'static dyn Invariant> {
    all().into_iter().find(|i| i.id().eq_ignore_ascii_case(id))
}

/// Read-only index access for the query-layer invariants.
pub struct DbView {
    pub conn: Connection,
    pub snapshot_id: i64,
}

/// Everything shared by the sections of one spec.
pub struct SpecCtx {
    pub src: SourceDoc,
    pub parsed: ParsedSpec,
    /// Section index by anchor, first occurrence wins (`INSERT OR IGNORE`).
    pub first: HashMap<String, usize>,
    pub structure: Option<StructuralSpec>,
    pub ir_by_anchor: HashMap<String, Vec<usize>>,
    pub refs_by_from: HashMap<String, BTreeSet<(String, String)>>,
    /// Document-order ranges each scope anchor owns: from each element carrying the anchor's id to
    /// the next element carrying any scope anchor's id.
    pub scope_ranges: HashMap<String, Vec<(usize, usize)>>,
    pub registry: SpecRegistry,
    pub db: Option<DbView>,
    pub stored: Option<Stored>,
    /// Lazily built per-spec slice indexes, keyed by anchor. Populated when no `DbView` is present.
    pub(crate) slice_indexes: OnceCell<BTreeMap<String, SliceIndex>>,
    /// Cross-spec `SPEC#anchor` existence, cached for A1.
    pub exists_cache: std::cell::RefCell<HashMap<(String, String), bool>>,
}

impl SpecCtx {
    pub fn new(
        src: SourceDoc,
        parsed: ParsedSpec,
        structure: Option<StructuralSpec>,
        db: Option<DbView>,
        stored: Option<Stored>,
    ) -> Self {
        let mut first = HashMap::new();
        for (i, s) in parsed.sections.iter().enumerate() {
            first.entry(s.anchor.clone()).or_insert(i);
        }
        let mut ir_by_anchor: HashMap<String, Vec<usize>> = HashMap::new();
        if let Some(st) = &structure {
            for (i, a) in st.algorithms.iter().enumerate() {
                ir_by_anchor
                    .entry(a.source.section_anchor.clone())
                    .or_default()
                    .push(i);
            }
        }
        let mut refs_by_from: HashMap<String, BTreeSet<(String, String)>> = HashMap::new();
        for r in &parsed.references {
            refs_by_from
                .entry(r.from_anchor.clone())
                .or_default()
                .insert((r.to_spec.clone(), r.to_anchor.clone()));
        }
        let scope_ids: HashSet<&str> = parsed
            .sections
            .iter()
            .filter(|s| {
                matches!(
                    s.section_type,
                    SectionType::Heading | SectionType::Algorithm
                )
            })
            .map(|s| s.anchor.as_str())
            .collect();
        let mut starts: Vec<(usize, String)> = Vec::new();
        for pos in 0..src.node_count() {
            let n = src.node_at(pos);
            if let Some(id) = n.value().as_element().and_then(|e| e.attr("id")) {
                if scope_ids.contains(id) {
                    starts.push((pos, id.to_string()));
                }
            }
        }
        let mut scope_ranges: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (i, (pos, id)) in starts.iter().enumerate() {
            let end = starts
                .get(i + 1)
                .map(|(p, _)| *p)
                .unwrap_or(src.node_count());
            scope_ranges
                .entry(id.clone())
                .or_default()
                .push((*pos, end));
        }
        SpecCtx {
            src,
            parsed,
            first,
            structure,
            ir_by_anchor,
            refs_by_from,
            scope_ranges,
            registry: SpecRegistry::new(),
            db,
            stored,
            slice_indexes: OnceCell::new(),
            exists_cache: Default::default(),
        }
    }

    pub fn spec(&self) -> &str {
        &self.src.spec
    }

    pub fn section(&self, anchor: &str) -> Option<&ParsedSection> {
        self.first.get(anchor).map(|i| &self.parsed.sections[*i])
    }

    /// Sections in first-wins order, one per anchor.
    pub fn unique_sections(&self) -> impl Iterator<Item = &ParsedSection> {
        self.parsed
            .sections
            .iter()
            .enumerate()
            .filter(|(i, s)| self.first.get(&s.anchor) == Some(i))
            .map(|(_, s)| s)
    }

    pub fn ir_algorithms(&self, anchor: &str) -> Vec<&StructuralAlgorithm> {
        let Some(st) = &self.structure else {
            return Vec::new();
        };
        self.ir_by_anchor
            .get(anchor)
            .map(|v| v.iter().map(|i| &st.algorithms[*i]).collect())
            .unwrap_or_default()
    }

    /// Resolve an href the way reference extraction does.
    pub fn resolve(&self, href: &str) -> Option<(String, String)> {
        if let Some(a) = href.strip_prefix('#') {
            Some((self.spec().to_string(), a.to_string()))
        } else if href.starts_with("http://") || href.starts_with("https://") {
            self.registry.resolve_url(href)
        } else {
            None
        }
    }

    pub fn section_url(&self, anchor: &str) -> String {
        let base = &self.src.base_url;
        if base.ends_with(".html") {
            format!("{base}#{anchor}")
        } else {
            format!("{base}/#{anchor}")
        }
    }

    /// Whether `SPEC#anchor` names an indexed section. Intra-spec answers come from the parse;
    /// others need the index.
    pub fn exists(&self, spec: &str, anchor: &str) -> Option<bool> {
        if spec == self.spec() {
            return Some(self.first.contains_key(anchor));
        }
        let db = self.db.as_ref()?;
        let key = (spec.to_string(), anchor.to_string());
        if let Some(v) = self.exists_cache.borrow().get(&key) {
            return Some(*v);
        }
        let v = webspec_index::db::queries::get_snapshot(&db.conn, spec)
            .ok()
            .flatten()
            .and_then(|sid| {
                webspec_index::db::queries::get_section(&db.conn, sid, anchor)
                    .ok()
                    .flatten()
            })
            .is_some();
        self.exists_cache.borrow_mut().insert(key, v);
        Some(v)
    }
}

/// One section under test, with lazily computed views.
pub struct SectionCtx<'a> {
    pub spec: &'a SpecCtx,
    pub section: &'a ParsedSection,
    pub el: ElementRef<'a>,
    pub region: Region<'a>,
    region_ids: HashSet<ego_tree::NodeId>,
    src_text: OnceCell<SrcText<'a>>,
    md: OnceCell<Md>,
    query: OnceCell<Option<webspec_index::model::QueryResult>>,
}

impl<'a> SectionCtx<'a> {
    pub fn new(spec: &'a SpecCtx, section: &'a ParsedSection) -> Option<Self> {
        let el = spec.src.locate(&section.anchor)?;
        let region = oracle::region(&spec.src, el, section.section_type);
        let mut region_ids: HashSet<_> = region.roots.iter().map(|n| n.id()).collect();
        region_ids.extend(region.fixture_roots.iter().map(|n| n.id()));
        Some(SectionCtx {
            spec,
            section,
            el,
            region,
            region_ids,
            src_text: OnceCell::new(),
            md: OnceCell::new(),
            query: OnceCell::new(),
        })
    }

    pub fn anchor(&self) -> &str {
        &self.section.anchor
    }

    pub fn ty(&self) -> SectionType {
        self.section.section_type
    }

    pub fn content(&self) -> &str {
        self.section.content_text.as_deref().unwrap_or("")
    }

    pub fn shape_key(&self) -> String {
        format!("{}/{}", self.ty().as_str(), self.region.shape.as_str())
    }

    pub fn src_text(&self) -> &SrcText<'a> {
        self.src_text
            .get_or_init(|| oracle::walk(&self.spec.src, &self.region.roots))
    }

    pub fn md(&self) -> &Md {
        self.md.get_or_init(|| oracle::md_info(self.content()))
    }

    /// Construct path of `n`. Inside an algorithm div, content after the div's first list is marked
    /// `div.algorithm[after-list]`: whatever follows the steps is one kind of place.
    pub fn construct(&self, n: N) -> String {
        let path = oracle::construct_path(n, &self.region_ids);
        if self.region.shape != oracle::Shape::AlgoDiv {
            return path;
        }
        let root = self.region.roots[0];
        let Some(top) = n
            .ancestors()
            .chain(std::iter::once(n))
            .find(|a| a.parent() == Some(root))
        else {
            return path;
        };
        let first_list = root
            .children()
            .position(|c| matches!(oracle::elname(&c), Some("ol" | "ul" | "dl")));
        let top_index = root.children().position(|c| c.id() == top.id());
        match (first_list, top_index) {
            (Some(l), Some(t)) if t > l => {
                path.replacen("div.algorithm", "div.algorithm[after-list]", 1)
            }
            _ => path,
        }
    }

    /// Construct path cut at the innermost block: where a word was lost, not how it was styled.
    pub fn block_construct(&self, n: N) -> String {
        let path = self.construct(n);
        let parts: Vec<&str> = path.split('>').collect();
        let keep = parts
            .iter()
            .rposition(|p| !oracle::INLINE_LABELS.contains(p))
            .map(|i| i + 1)
            .unwrap_or(parts.len());
        parts[..keep].join(">")
    }

    pub fn query_result(&self) -> Option<&webspec_index::model::QueryResult> {
        self.query
            .get_or_init(|| {
                let db = self.spec.db.as_ref()?;
                webspec_index::query_section_from_conn(&db.conn, self.spec.spec(), self.anchor())
                    .ok()
                    .flatten()
            })
            .as_ref()
    }

    pub fn region_html(&self) -> String {
        oracle::fixture_html(&self.region.fixture_roots, &self.region.excluded)
    }

    /// The document a fixture for `invariant` needs. Checks on rendered content (M0, M2, M4) need
    /// what the product renders for a heading today, subsections included; the rest need the
    /// section's own region.
    pub fn fixture_html_for(&self, invariant: &str) -> String {
        if self.region.shape.is_heading() && matches!(invariant, "M0" | "M2" | "M4") {
            let mut roots = vec![self.region.fixture_roots[0]];
            roots.extend(oracle::heading_extent(self.el));
            return oracle::fixture_html(&roots, &Default::default());
        }
        self.region_html()
    }
}
