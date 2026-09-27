//! Rule-independent extraction of algorithm, step, body, and inline structure.
//!
//! This module deliberately stops before effect classification. It records the
//! lexical facts needed by later analyses without deciding whether a linked
//! operation invokes a body inline, schedules it, or merely represents an
//! uncertain call.

use regex::Regex;
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use super::algorithms::step_number;

/// Version of the serialized structural parse format.
pub const STRUCTURE_VERSION: &str = "10";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralSpec {
    pub version: String,
    pub spec: String,
    pub snapshot_sha: String,
    /// Actual fragment targets, including prose definitions without a step body.
    #[serde(default)]
    pub anchors: Vec<String>,
    pub algorithms: Vec<StructuralAlgorithm>,
    pub issues: Vec<StructuralIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralAlgorithm {
    pub source: SourceIdentity,
    pub title: Option<String>,
    pub root_body_id: String,
    pub bodies: Vec<StructuralBody>,
    pub steps: Vec<StructuralStep>,
    pub segments: Vec<StructuralSegment>,
    pub notes: Vec<StructuralNote>,
    pub branches: Vec<StructuralBranch>,
    pub operation_sites: Vec<OperationSite>,
    pub body_definitions: Vec<BodyDefinitionSite>,
    pub body_invocations: Vec<BodyInvocationSite>,
    pub body_arguments: Vec<BodyArgumentSite>,
    pub continuations: Vec<ContinuationSite>,
    pub issues: Vec<StructuralIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub spec: String,
    pub snapshot_sha: String,
    pub section_anchor: String,
    pub node_id: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralBody {
    pub source: SourceIdentity,
    pub kind: BodyKind,
    pub name: Option<String>,
    pub parent_body_id: Option<String>,
    pub defined_at_step_id: Option<String>,
    pub items: Vec<BodyItem>,
    pub range: Option<BodyRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    Algorithm,
    Named,
    Anonymous,
    Remainder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum BodyItem {
    Step(String),
    Segment(String),
    Note(String),
    Branch(String),
    Body(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyRange {
    pub start_step_id: String,
    pub end_step_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralStep {
    pub source: SourceIdentity,
    pub body_id: String,
    pub parent_step_id: Option<String>,
    pub path: Vec<u32>,
    pub ordinal: u32,
    pub items: Vec<StepItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum StepItem {
    Segment(String),
    ChildStep(String),
    Note(String),
    Branch(String),
    Body(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralSegment {
    pub source: SourceIdentity,
    pub owner_step_id: Option<String>,
    pub owner_body_id: String,
    pub ordinal: u32,
    /// Canonical matching text. All spans below are UTF-8 byte offsets here.
    pub text: String,
    pub tokens: Vec<InlineToken>,
    pub links: Vec<LinkSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineToken {
    pub kind: InlineTokenKind,
    pub span: TextSpan,
    pub source_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InlineTokenKind {
    Text,
    Variable,
    Code,
    Literal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSpan {
    pub id: String,
    pub span: TextSpan,
    pub visible_text: String,
    pub href: String,
    pub target: Option<AnchorTarget>,
    pub generator_id: Option<String>,
    pub link_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AnchorTarget {
    pub spec: String,
    pub anchor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralNote {
    pub source: SourceIdentity,
    pub kind: NoteKind,
    pub text: String,
    pub links: Vec<LinkSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Note,
    Example,
    Warning,
    Advisement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralBranch {
    pub source: SourceIdentity,
    pub parent_step_id: String,
    pub label: String,
    /// Canonical `<dt>` text in segment encoding; the label spans below index
    /// it. Empty for `<ul>` items, whose text is in their segments.
    #[serde(default)]
    pub label_text: String,
    #[serde(default)]
    pub label_tokens: Vec<InlineToken>,
    #[serde(default)]
    pub label_links: Vec<LinkSpan>,
    pub items: Vec<StepItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSite {
    pub source: SourceIdentity,
    pub step_id: String,
    pub body_id: String,
    pub segment_id: String,
    pub span: TextSpan,
    pub link_id: String,
    pub target: Option<AnchorTarget>,
    pub role: ReferenceRole,
    pub actual_body_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRole {
    /// Executable segment with invocation semantics left to later analysis.
    Operation,
    /// Syntax that is clearly a non-executing mention or type reference.
    Mention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyDefinitionSite {
    pub source: SourceIdentity,
    pub step_id: String,
    pub scope_body_id: String,
    pub name: String,
    pub body_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyInvocationSite {
    pub source: SourceIdentity,
    pub step_id: String,
    pub scope_body_id: String,
    pub verb: BodyInvocationVerb,
    pub name: String,
    pub span: TextSpan,
    pub candidate_body_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyInvocationVerb {
    Run,
    Perform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyArgumentSite {
    pub source: SourceIdentity,
    pub operation_site_id: String,
    pub step_id: String,
    pub body_id: String,
    pub binding: BodyArgumentBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyArgumentBinding {
    Named,
    Anonymous,
    Remainder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationSite {
    pub source: SourceIdentity,
    pub step_id: String,
    pub enclosing_body_id: String,
    pub segment_id: String,
    pub span: TextSpan,
    pub syntax: ContinuationSyntax,
    pub remainder_body_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationSyntax {
    Inline,
    Queued,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralIssue {
    pub code: String,
    pub message: String,
    pub site_id: Option<String>,
}

struct ExtractContext<'a> {
    spec: &'a str,
    base_url: &'a str,
    snapshot_sha: &'a str,
    anchor: &'a str,
    owner: ElementRef<'a>,
}

struct AlgorithmBuilder<'a> {
    ctx: ExtractContext<'a>,
    algorithm: StructuralAlgorithm,
    body_bindings: BTreeMap<(String, String), Vec<String>>,
    pending_continuations: Vec<usize>,
}

/// Extract reusable algorithm structure from one source snapshot.
///
/// The caller supplies the snapshot digest because persistence owns snapshot
/// identity. This function performs no database access and no effect analysis.
pub fn extract_step_structure(
    html: &str,
    spec: &str,
    base_url: &str,
    snapshot_sha: &str,
) -> StructuralSpec {
    let document = Html::parse_document(html);
    extract_step_structure_from_document(&document, spec, base_url, snapshot_sha)
}

/// Extract reusable algorithm structure from an already parsed snapshot.
///
/// The caller supplies the snapshot digest because persistence owns snapshot
/// identity. This function performs no database access and no effect analysis.
pub fn extract_step_structure_from_document(
    document: &Html,
    spec: &str,
    base_url: &str,
    snapshot_sha: &str,
) -> StructuralSpec {
    let candidates = find_algorithm_candidates(document);
    let mut algorithms = Vec::new();
    let mut issues = Vec::new();

    for candidate in candidates {
        let anchor = candidate.anchor.clone();
        let ctx = ExtractContext {
            spec,
            base_url,
            snapshot_sha,
            anchor: &anchor,
            owner: candidate.owner,
        };
        match extract_algorithm(candidate, ctx) {
            Some(algorithm) => algorithms.push(algorithm),
            None => issues.push(StructuralIssue {
                code: "unsupported_structure".to_string(),
                message: format!("algorithm {} has no supported ordered step body", anchor),
                site_id: None,
            }),
        }
    }

    resolve_aoid_links(document, spec, &mut algorithms);

    StructuralSpec {
        version: STRUCTURE_VERSION.to_string(),
        spec: spec.to_string(),
        snapshot_sha: snapshot_sha.to_string(),
        anchors: document
            .select(&Selector::parse("[id]").unwrap())
            .filter_map(|element| element.value().id().map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        algorithms,
        issues,
    }
}

/// Identity of an inline rendering outside structural algorithms. `anchor`
/// scopes generated link ids exactly as a structural algorithm anchor does.
pub(crate) struct InlineContext<'a> {
    pub spec: &'a str,
    pub base_url: &'a str,
    pub snapshot_sha: &'a str,
    pub anchor: &'a str,
}

/// Canonical text, tokens and links of one block, in segment encoding. Nested
/// lists and callouts are skipped: they are their own blocks.
pub(crate) fn canonical_inline(
    element: &ElementRef<'_>,
    ctx: &InlineContext<'_>,
) -> (String, Vec<InlineToken>, Vec<LinkSpan>) {
    let (text, tokens, links, _) = canonical_inline_marked(element, ctx, None);
    (text, tokens, links)
}

/// Positions in a marked inline rendering.
pub(crate) struct InlineMarks {
    /// Span of the rendered text of the element `mark` (the defining dfn), if it was rendered.
    pub mark: Option<TextSpan>,
    /// One entry per `Variable` token, in token order: the `<var>` element's node id.
    pub vars: Vec<(TextSpan, ego_tree::NodeId)>,
}

/// `canonical_inline`, also reporting where the element `mark` and every
/// `<var>` landed in the text.
pub(crate) fn canonical_inline_marked(
    element: &ElementRef<'_>,
    ctx: &InlineContext<'_>,
    mark: Option<ego_tree::NodeId>,
) -> (String, Vec<InlineToken>, Vec<LinkSpan>, InlineMarks) {
    let ctx = ExtractContext {
        spec: ctx.spec,
        base_url: ctx.base_url,
        snapshot_sha: ctx.snapshot_sha,
        anchor: ctx.anchor,
        owner: *element,
    };
    let mut builder = CanonicalBuilder::new(&ctx);
    builder.skip_nested_blocks = true;
    builder.mark = mark;
    for child in element.children() {
        builder.walk(child);
    }
    // The builder never emits leading whitespace, so trimming shifts no span.
    debug_assert_eq!(builder.text.trim_start().len(), builder.text.len());
    (
        builder.text.trim().to_string(),
        builder.tokens,
        builder.links,
        InlineMarks {
            mark: builder.mark_span,
            vars: builder.var_nodes,
        },
    )
}

/// Resolve a link `href` to its canonical anchor target.
pub(crate) fn resolve_href(href: &str, spec: &str, base_url: &str) -> Option<AnchorTarget> {
    resolve_target(href, spec, base_url)
}

/// Node ids of every `<ol>`/`emu-alg` that is a structural algorithm body.
pub(crate) fn structural_body_nodes(document: &Html) -> HashSet<ego_tree::NodeId> {
    find_algorithm_candidates(document)
        .into_iter()
        .flat_map(|candidate| {
            let body = candidate.body.map(|body| match body {
                CandidateBody::List(list) => list.id(),
                CandidateBody::SourceEmuAlg(emu_alg) => emu_alg.id(),
            });
            body.into_iter()
                .chain(candidate.extra_bodies.iter().map(|list| list.id()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn resolve_aoid_links(document: &Html, spec: &str, algorithms: &mut [StructuralAlgorithm]) {
    // Source-form Ecmarkup uses aoid references without href fragments. Resolve
    // only names with one target in this snapshot; ambiguous names stay unknown.
    let mut anchors: HashMap<String, Option<String>> = HashMap::new();
    let selector = Selector::parse("[aoid][id]").expect("valid selector");
    for element in document.select(&selector) {
        let (Some(aoid), Some(id)) = (element.value().attr("aoid"), element.value().id()) else {
            continue;
        };
        anchors
            .entry(aoid.to_string())
            .and_modify(|known| {
                if known.as_deref() != Some(id) {
                    *known = None;
                }
            })
            .or_insert_with(|| Some(id.to_string()));
    }
    for algorithm in algorithms {
        let mut resolved = HashMap::new();
        for segment in &mut algorithm.segments {
            for link in &mut segment.links {
                let Some(aoid) = link.href.strip_prefix("aoid:") else {
                    continue;
                };
                let Some(Some(anchor)) = anchors.get(aoid) else {
                    continue;
                };
                let target = AnchorTarget {
                    spec: spec.to_string(),
                    anchor: anchor.clone(),
                };
                link.target = Some(target.clone());
                resolved.insert((segment.source.node_id.clone(), link.id.clone()), target);
            }
        }
        for operation in &mut algorithm.operation_sites {
            if let Some(target) =
                resolved.get(&(operation.segment_id.clone(), operation.link_id.clone()))
            {
                operation.target = Some(target.clone());
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CandidateBody<'a> {
    List(ElementRef<'a>),
    SourceEmuAlg(ElementRef<'a>),
}

struct AlgorithmCandidate<'a> {
    anchor: String,
    title: Option<String>,
    owner: ElementRef<'a>,
    body: Option<CandidateBody<'a>>,
    /// Step lists of dfn-less algorithm divs that point back to this anchor
    /// ("The <a>x</a> setter steps are:"), each an extra entry body.
    extra_bodies: Vec<ElementRef<'a>>,
    order: usize,
}

fn find_algorithm_candidates<'a>(document: &'a Html) -> Vec<AlgorithmCandidate<'a>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let positions: HashMap<ego_tree::NodeId, usize> = document
        .root_element()
        .descendants()
        .enumerate()
        .map(|(index, node)| (node.id(), index))
        .collect();
    let position =
        |element: &ElementRef<'_>| positions.get(&element.id()).copied().unwrap_or(usize::MAX);

    let emu_selector = Selector::parse("emu-clause[id], emu-annex[id]").expect("valid selector");
    for clause in document.select(&emu_selector) {
        let Some(emu_alg) = direct_child_named(&clause, "emu-alg") else {
            continue;
        };
        let anchor = clause.value().attr("id").unwrap().to_string();
        let title = direct_child_named(&clause, "h1")
            .map(|heading| normalize_plain_text(&heading.text().collect::<String>()));
        let body = direct_child_named(&emu_alg, "ol")
            .map(CandidateBody::List)
            .or(Some(CandidateBody::SourceEmuAlg(emu_alg)));
        seen.insert(anchor.clone());
        result.push(AlgorithmCandidate {
            anchor,
            title,
            owner: clause,
            body,
            extra_bodies: Vec::new(),
            order: position(&clause),
        });
    }

    let mut continued = Vec::new();
    let div_selector =
        Selector::parse("div.algorithm, div[data-algorithm]").expect("valid algorithm selector");
    for container in document.select(&div_selector) {
        if has_algorithm_ancestor(&container) {
            continue;
        }
        let Some(first) = first_definition_outside_list(&container) else {
            let intro = container.children().find_map(ElementRef::wrap);
            if let (Some(anchor), Some(list)) = (
                intro.and_then(|p| super::sections::continued_anchor(&p)),
                first_outer_list(&container),
            ) {
                continued.push((anchor.to_string(), container, list));
            }
            continue;
        };
        let pairs = algorithm_pairs(&container);
        if pairs.is_empty() {
            let anchor = first.value().attr("id").unwrap().to_string();
            if !seen.insert(anchor.clone()) {
                continue;
            }
            let body = first_outer_list(&container).map(CandidateBody::List);
            result.push(AlgorithmCandidate {
                anchor,
                title: nonempty_text(&first),
                owner: container,
                body,
                extra_bodies: Vec::new(),
                order: position(&container),
            });
            continue;
        }
        for (dfn, intro, list) in pairs {
            let anchor = dfn.value().attr("id").unwrap().to_string();
            if !seen.insert(anchor.clone()) {
                continue;
            }
            // The container's own algorithm keeps the container as owner, so
            // its node ids match those of single-algorithm containers.
            let owner = if dfn.id() == first.id() {
                container
            } else {
                intro
            };
            result.push(AlgorithmCandidate {
                anchor,
                title: nonempty_text(&dfn),
                owner,
                body: Some(CandidateBody::List(list)),
                extra_bodies: Vec::new(),
                order: position(&owner),
            });
        }
    }

    let dfn_selector = Selector::parse("dfn[id]").expect("valid dfn selector");
    for dfn in document.select(&dfn_selector) {
        let anchor = dfn.value().attr("id").unwrap().to_string();
        if seen.contains(&anchor) || has_algorithm_ancestor(&dfn) || inside_emu_clause(&dfn) {
            continue;
        }
        let Some(owner) = dfn.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        let Some(block) = following_algorithm_block(&owner) else {
            continue;
        };
        let body = (block.value().name() == "ol").then_some(CandidateBody::List(block));
        seen.insert(anchor.clone());
        result.push(AlgorithmCandidate {
            anchor,
            title: nonempty_text(&dfn),
            owner,
            body,
            extra_bodies: Vec::new(),
            order: position(&owner),
        });
    }

    let mut continued_titles: HashMap<&str, Option<String>> = HashMap::new();
    if !continued.is_empty() {
        let wanted: HashSet<&str> = continued.iter().map(|(a, _, _)| a.as_str()).collect();
        for dfn in document.select(&dfn_selector) {
            if let Some(id) = dfn.value().id().filter(|id| wanted.contains(id)) {
                continued_titles
                    .entry(id)
                    .or_insert_with(|| nonempty_text(&dfn));
            }
        }
    }
    for (anchor, container, list) in continued {
        match result
            .iter_mut()
            .find(|candidate| candidate.anchor == anchor)
        {
            Some(candidate) if candidate.body.is_some() => candidate.extra_bodies.push(list),
            Some(candidate) => {
                candidate.owner = container;
                candidate.body = Some(CandidateBody::List(list));
                candidate.order = position(&container);
            }
            None => {
                let Some(title) = continued_titles.get(anchor.as_str()).cloned() else {
                    continue;
                };
                result.push(AlgorithmCandidate {
                    anchor,
                    title,
                    owner: container,
                    body: Some(CandidateBody::List(list)),
                    extra_bodies: Vec::new(),
                    order: position(&container),
                });
            }
        }
    }

    result.sort_by_key(|candidate| candidate.order);
    result
}

fn extract_algorithm(
    candidate: AlgorithmCandidate<'_>,
    ctx: ExtractContext<'_>,
) -> Option<StructuralAlgorithm> {
    let body = candidate.body?;
    let owner_path = algorithm_path(&candidate.owner, &candidate.owner);
    let algorithm_source = source_identity(&ctx, "algorithm", &owner_path, None);
    let root_body_source = source_identity(&ctx, "body", &owner_path, Some("root"));
    let root_body_id = root_body_source.node_id.clone();
    let mut builder = AlgorithmBuilder {
        ctx,
        algorithm: StructuralAlgorithm {
            source: algorithm_source,
            title: candidate.title,
            root_body_id: root_body_id.clone(),
            bodies: vec![StructuralBody {
                source: root_body_source,
                kind: BodyKind::Algorithm,
                name: None,
                parent_body_id: None,
                defined_at_step_id: None,
                items: Vec::new(),
                range: None,
            }],
            steps: Vec::new(),
            segments: Vec::new(),
            notes: Vec::new(),
            branches: Vec::new(),
            operation_sites: Vec::new(),
            body_definitions: Vec::new(),
            body_invocations: Vec::new(),
            body_arguments: Vec::new(),
            continuations: Vec::new(),
            issues: Vec::new(),
        },
        body_bindings: BTreeMap::new(),
        pending_continuations: Vec::new(),
    };

    match body {
        CandidateBody::List(list) => {
            builder.parse_list(&list, &root_body_id, None, &[]);
        }
        CandidateBody::SourceEmuAlg(emu_alg) => {
            builder.parse_source_emu_alg(&emu_alg, &root_body_id);
        }
    }
    for list in &candidate.extra_bodies {
        let source = source_identity(&builder.ctx, "body", &builder.path(list), Some("entry"));
        let body_id = source.node_id.clone();
        builder.algorithm.bodies.push(StructuralBody {
            source,
            kind: BodyKind::Algorithm,
            name: None,
            parent_body_id: Some(root_body_id.clone()),
            defined_at_step_id: None,
            items: Vec::new(),
            range: None,
        });
        builder
            .body_mut(&root_body_id)
            .items
            .push(BodyItem::Body(body_id.clone()));
        builder.parse_list(list, &body_id, None, &[]);
    }
    builder.finish_remainders();
    Some(builder.algorithm)
}

impl AlgorithmBuilder<'_> {
    fn path(&self, element: &ElementRef<'_>) -> String {
        algorithm_path(&self.ctx.owner, element)
    }

    fn parse_list(
        &mut self,
        list: &ElementRef<'_>,
        body_id: &str,
        parent_step_id: Option<&str>,
        path_prefix: &[u32],
    ) -> Vec<String> {
        let mut ids = Vec::new();
        for (position, item) in direct_children_named(list, "li").into_iter().enumerate() {
            let number = step_number(&item).unwrap_or(position + 1) as u32;
            let mut path = path_prefix.to_vec();
            path.push(number);
            let step_path = self.path(&item);
            let source = source_identity(&self.ctx, "step", &step_path, None);
            let step_id = source.node_id.clone();
            self.algorithm.steps.push(StructuralStep {
                source,
                body_id: body_id.to_string(),
                parent_step_id: parent_step_id.map(str::to_string),
                path: path.clone(),
                ordinal: (position + 1) as u32,
                items: Vec::new(),
            });
            if let Some(parent) = parent_step_id {
                self.step_mut(parent)
                    .items
                    .push(StepItem::ChildStep(step_id.clone()));
            } else {
                self.body_mut(body_id)
                    .items
                    .push(BodyItem::Step(step_id.clone()));
            }
            self.parse_step_children(&item, &step_id, body_id, &path);
            ids.push(step_id);
        }
        ids
    }

    fn parse_step_children(
        &mut self,
        item: &ElementRef<'_>,
        step_id: &str,
        body_id: &str,
        path: &[u32],
    ) {
        let mut inline_group = Vec::new();
        for child in item.children() {
            let child_element = ElementRef::wrap(child);
            if let Some(element) = child_element {
                let name = element.value().name();
                if name == "ol" || name == "ul" || name == "dl" || note_kind(&element).is_some() {
                    self.flush_inline_group(&mut inline_group, step_id, body_id);
                    match name {
                        "ol" => self.parse_nested_list(&element, step_id, body_id, path),
                        "ul" => self.parse_ul_branches(&element, step_id, body_id, path),
                        "dl" => self.parse_dl_branches(&element, step_id, body_id, path),
                        _ => self.record_note(&element, step_id, body_id),
                    }
                    continue;
                }
                if is_block_element(name) {
                    self.flush_inline_group(&mut inline_group, step_id, body_id);
                    let nodes: Vec<_> = element.children().collect();
                    self.record_segment_nodes(&nodes, &self.path(&element), step_id, body_id);
                    continue;
                }
            }
            inline_group.push(child);
        }
        self.flush_inline_group(&mut inline_group, step_id, body_id);
    }

    fn flush_inline_group(
        &mut self,
        nodes: &mut Vec<ego_tree::NodeRef<'_, Node>>,
        step_id: &str,
        body_id: &str,
    ) {
        if nodes.is_empty() {
            return;
        }
        let path = nodes
            .iter()
            .find_map(|node| ElementRef::wrap(*node).map(|element| self.path(&element)))
            .unwrap_or_else(|| format!("{step_id}/text"));
        self.record_segment_nodes(nodes, &path, step_id, body_id);
        nodes.clear();
    }

    fn record_segment_nodes(
        &mut self,
        nodes: &[ego_tree::NodeRef<'_, Node>],
        path: &str,
        step_id: &str,
        body_id: &str,
    ) {
        let ordinal = self
            .algorithm
            .segments
            .iter()
            .filter(|segment| segment.owner_step_id.as_deref() == Some(step_id))
            .count() as u32
            + 1;
        let source = source_identity(
            &self.ctx,
            "segment",
            path,
            Some(&format!("{step_id}:{ordinal}")),
        );
        let segment = canonical_segment(nodes, source, Some(step_id), body_id, ordinal, &self.ctx);
        if segment.text.is_empty() {
            return;
        }
        self.record_segment(segment, step_id, body_id);
    }

    fn record_segment(&mut self, segment: StructuralSegment, step_id: &str, body_id: &str) {
        if let Some((name, before, mut body_segment)) = split_inline_named_definition(&segment) {
            let body_path = format!("{}/named-body", segment.source.node_id);
            let body_source = source_identity(&self.ctx, "body", &body_path, Some(&name));
            let named_body_id = body_source.node_id.clone();
            body_segment.owner_body_id = named_body_id.clone();
            self.algorithm.bodies.push(StructuralBody {
                source: body_source,
                kind: BodyKind::Named,
                name: Some(name.clone()),
                parent_body_id: Some(body_id.to_string()),
                defined_at_step_id: Some(step_id.to_string()),
                items: vec![BodyItem::Segment(body_segment.source.node_id.clone())],
                range: None,
            });
            self.algorithm.body_definitions.push(BodyDefinitionSite {
                source: source_identity(
                    &self.ctx,
                    "body-definition",
                    &segment.source.node_id,
                    Some(&name),
                ),
                step_id: step_id.to_string(),
                scope_body_id: body_id.to_string(),
                name: name.clone(),
                body_id: named_body_id.clone(),
            });
            self.body_bindings
                .entry((body_id.to_string(), name.to_ascii_lowercase()))
                .or_default()
                .push(named_body_id.clone());

            self.algorithm.segments.push(before.clone());
            self.step_mut(step_id)
                .items
                .push(StepItem::Segment(before.source.node_id.clone()));
            self.record_operations(&before, step_id, body_id);
            self.algorithm.segments.push(body_segment.clone());
            self.record_operations(&body_segment, step_id, &named_body_id);
            self.record_body_invocations(&body_segment, step_id, &named_body_id);
            self.record_named_body_arguments(&body_segment, step_id, &named_body_id);
            self.record_continuation(&body_segment, step_id, &named_body_id);
            self.step_mut(step_id)
                .items
                .push(StepItem::Body(named_body_id));
            return;
        }

        if let Some((before, mut after, initiating_link_id)) = split_inline_body(&segment) {
            let anonymous_path = format!("{}/inline-body", segment.source.node_id);
            let body_source = source_identity(&self.ctx, "body", &anonymous_path, None);
            let anonymous_body_id = body_source.node_id.clone();
            after.owner_body_id = anonymous_body_id.clone();
            self.algorithm.bodies.push(StructuralBody {
                source: body_source,
                kind: BodyKind::Anonymous,
                name: None,
                parent_body_id: Some(body_id.to_string()),
                defined_at_step_id: Some(step_id.to_string()),
                items: Vec::new(),
                range: None,
            });
            let before_id = before.source.node_id.clone();
            let after_id = after.source.node_id.clone();
            self.algorithm.segments.push(before.clone());
            self.step_mut(step_id)
                .items
                .push(StepItem::Segment(before_id));
            self.record_operations(&before, step_id, body_id);
            self.algorithm.segments.push(after.clone());
            self.body_mut(&anonymous_body_id)
                .items
                .push(BodyItem::Segment(after_id));
            self.record_operations(&after, step_id, &anonymous_body_id);
            self.record_body_invocations(&after, step_id, &anonymous_body_id);
            self.record_named_body_arguments(&after, step_id, &anonymous_body_id);
            self.record_continuation(&after, step_id, &anonymous_body_id);
            self.step_mut(step_id)
                .items
                .push(StepItem::Body(anonymous_body_id.clone()));

            if let Some(operation_id) = self
                .algorithm
                .operation_sites
                .iter()
                .find(|site| {
                    site.segment_id == before.source.node_id && site.link_id == initiating_link_id
                })
                .map(|site| site.source.node_id.clone())
            {
                self.attach_body_argument(
                    &operation_id,
                    step_id,
                    &anonymous_body_id,
                    BodyArgumentBinding::Anonymous,
                );
            }
            return;
        }

        let segment_id = segment.source.node_id.clone();
        self.algorithm.segments.push(segment.clone());
        self.step_mut(step_id)
            .items
            .push(StepItem::Segment(segment_id));
        self.record_operations(&segment, step_id, body_id);
        self.record_body_invocations(&segment, step_id, body_id);
        self.record_named_body_arguments(&segment, step_id, body_id);
        self.record_continuation(&segment, step_id, body_id);
    }

    fn record_operations(&mut self, segment: &StructuralSegment, step_id: &str, body_id: &str) {
        for link in &segment.links {
            let role = if link_is_in_mention_context(segment, link)
                || link_is_type_mention(segment, link)
            {
                ReferenceRole::Mention
            } else {
                ReferenceRole::Operation
            };
            let path = format!("{}/operation/{}", segment.source.node_id, link.id);
            let mut source = source_identity(&self.ctx, "operation", &path, None);
            if let Some(generator_id) = &link.generator_id {
                source.url = canonical_url(self.ctx.base_url, generator_id);
            }
            self.algorithm.operation_sites.push(OperationSite {
                source,
                step_id: step_id.to_string(),
                body_id: body_id.to_string(),
                segment_id: segment.source.node_id.clone(),
                span: link.span,
                link_id: link.id.clone(),
                target: link.target.clone(),
                role,
                actual_body_ids: Vec::new(),
            });
        }
    }

    fn record_body_invocations(
        &mut self,
        segment: &StructuralSegment,
        step_id: &str,
        body_id: &str,
    ) {
        for captures in body_invocation_re().captures_iter(&segment.text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            let Some(name_match) = captures.name("name") else {
                continue;
            };
            if !segment.tokens.iter().any(|token| {
                token.kind == InlineTokenKind::Variable
                    && token.span.start <= name_match.start()
                    && token.span.end >= name_match.end()
            }) {
                continue;
            }
            let name = name_match.as_str().trim_matches('*').to_string();
            let verb = if captures.name("run").is_some() {
                BodyInvocationVerb::Run
            } else {
                BodyInvocationVerb::Perform
            };
            let candidate_body_ids = self.visible_bindings(body_id, &name);
            let unresolved = candidate_body_ids.is_empty();
            let source = source_identity(
                &self.ctx,
                "body-invocation",
                &segment.source.node_id,
                Some(&format!("{}:{}", whole.start(), whole.end())),
            );
            self.algorithm.body_invocations.push(BodyInvocationSite {
                source: source.clone(),
                step_id: step_id.to_string(),
                scope_body_id: body_id.to_string(),
                verb,
                name: name.clone(),
                span: TextSpan {
                    start: whole.start(),
                    end: whole.end(),
                },
                candidate_body_ids,
            });
            if unresolved {
                self.algorithm.issues.push(StructuralIssue {
                    code: "unresolved_body_binding".to_string(),
                    message: format!("no lexical body named {name:?} is visible here"),
                    site_id: Some(source.node_id),
                });
            }
        }
    }

    fn record_named_body_arguments(
        &mut self,
        segment: &StructuralSegment,
        step_id: &str,
        body_id: &str,
    ) {
        let operation_ids: Vec<String> = self
            .algorithm
            .operation_sites
            .iter()
            .filter(|site| {
                site.segment_id == segment.source.node_id && site.role == ReferenceRole::Operation
            })
            .map(|site| site.source.node_id.clone())
            .collect();
        if operation_ids.is_empty() {
            return;
        }
        let bindings = self.visible_binding_entries(body_id);
        for (name, bound_body_id) in bindings {
            for argument_span in explicit_body_argument_spans(segment, &name) {
                let clause_start = preceding_clause_boundary(&segment.text, argument_span.start);
                let local_operation_ids: Vec<String> = self
                    .algorithm
                    .operation_sites
                    .iter()
                    .filter(|site| {
                        operation_ids.contains(&site.source.node_id)
                            && site.span.start >= clause_start
                            && site.span.end <= argument_span.start
                    })
                    .map(|site| site.source.node_id.clone())
                    .collect();
                // Prefer the scheduling operation that introduces the body.
                // Other linked arguments (task source, global object, and so
                // on) can occur between that operation and the body argument.
                let Some(operation_id) = self.named_body_operation_id(&local_operation_ids) else {
                    let message =
                        format!("could not identify the operation receiving lexical body {name:?}");
                    if !self.algorithm.issues.iter().any(|issue| {
                        issue.code == "unresolved_body_binding"
                            && issue.site_id.as_deref() == Some(segment.source.node_id.as_str())
                            && issue.message == message
                    }) {
                        self.algorithm.issues.push(StructuralIssue {
                            code: "unresolved_body_binding".to_string(),
                            message,
                            site_id: Some(segment.source.node_id.clone()),
                        });
                    }
                    continue;
                };
                self.attach_body_argument(
                    &operation_id,
                    step_id,
                    &bound_body_id,
                    BodyArgumentBinding::Named,
                );
            }
        }
    }

    fn attach_body_argument(
        &mut self,
        operation_id: &str,
        step_id: &str,
        body_id: &str,
        binding: BodyArgumentBinding,
    ) {
        let source = source_identity(&self.ctx, "body-argument", operation_id, Some(body_id));
        if let Some(operation) = self
            .algorithm
            .operation_sites
            .iter_mut()
            .find(|site| site.source.node_id == operation_id)
        {
            if !operation.actual_body_ids.iter().any(|id| id == body_id) {
                operation.actual_body_ids.push(body_id.to_string());
            }
        }
        if !self.algorithm.body_arguments.iter().any(|argument| {
            argument.operation_site_id == operation_id && argument.body_id == body_id
        }) {
            self.algorithm.body_arguments.push(BodyArgumentSite {
                source,
                operation_site_id: operation_id.to_string(),
                step_id: step_id.to_string(),
                body_id: body_id.to_string(),
                binding,
            });
        }
    }

    fn record_continuation(&mut self, segment: &StructuralSegment, step_id: &str, body_id: &str) {
        for continuation_match in continue_re().find_iter(&segment.text) {
            let syntax = continuation_syntax(
                &segment.text,
                continuation_match.start(),
                continuation_match.end(),
            );
            let source = source_identity(
                &self.ctx,
                "continuation",
                &segment.source.node_id,
                Some(&format!(
                    "{}:{}",
                    continuation_match.start(),
                    continuation_match.end()
                )),
            );
            let index = self.algorithm.continuations.len();
            self.algorithm.continuations.push(ContinuationSite {
                source,
                step_id: step_id.to_string(),
                enclosing_body_id: body_id.to_string(),
                segment_id: segment.source.node_id.clone(),
                span: TextSpan {
                    start: continuation_match.start(),
                    end: continuation_match.end(),
                },
                syntax,
                remainder_body_id: None,
            });
            self.pending_continuations.push(index);
        }
    }

    fn parse_nested_list(
        &mut self,
        list: &ElementRef<'_>,
        step_id: &str,
        body_id: &str,
        path: &[u32],
    ) {
        let preceding = self.step_text(step_id);
        if let Some(name) = body_definition_name(&preceding) {
            let body_source = source_identity(&self.ctx, "body", &self.path(list), Some(&name));
            let new_body_id = body_source.node_id.clone();
            self.algorithm.bodies.push(StructuralBody {
                source: body_source.clone(),
                kind: BodyKind::Named,
                name: Some(name.clone()),
                parent_body_id: Some(body_id.to_string()),
                defined_at_step_id: Some(step_id.to_string()),
                items: Vec::new(),
                range: None,
            });
            self.algorithm.body_definitions.push(BodyDefinitionSite {
                source: source_identity(
                    &self.ctx,
                    "body-definition",
                    &self.path(list),
                    Some(&name),
                ),
                step_id: step_id.to_string(),
                scope_body_id: body_id.to_string(),
                name: name.clone(),
                body_id: new_body_id.clone(),
            });
            self.body_bindings
                .entry((body_id.to_string(), name.to_ascii_lowercase()))
                .or_default()
                .push(new_body_id.clone());
            self.step_mut(step_id)
                .items
                .push(StepItem::Body(new_body_id.clone()));
            self.parse_list(list, &new_body_id, None, path);
        } else if anonymous_body_intro(&preceding) {
            let body_source =
                source_identity(&self.ctx, "body", &self.path(list), Some("anonymous"));
            let new_body_id = body_source.node_id.clone();
            self.algorithm.bodies.push(StructuralBody {
                source: body_source,
                kind: BodyKind::Anonymous,
                name: None,
                parent_body_id: Some(body_id.to_string()),
                defined_at_step_id: Some(step_id.to_string()),
                items: Vec::new(),
                range: None,
            });
            self.step_mut(step_id)
                .items
                .push(StepItem::Body(new_body_id.clone()));
            self.parse_list(list, &new_body_id, None, path);
            let operation_ids: Vec<_> = self
                .algorithm
                .operation_sites
                .iter()
                .filter(|site| site.step_id == step_id && site.body_id == body_id)
                .map(|site| site.source.node_id.clone())
                .collect();
            if let Some(operation_id) = self.preferred_body_operation_id(&operation_ids) {
                self.attach_body_argument(
                    &operation_id,
                    step_id,
                    &new_body_id,
                    BodyArgumentBinding::Anonymous,
                );
            }
        } else {
            self.parse_list(list, body_id, Some(step_id), path);
        }
    }

    fn preferred_body_operation_id(&self, operation_ids: &[String]) -> Option<String> {
        self.scheduling_body_operation_id(operation_ids)
            .or_else(|| operation_ids.last().cloned())
    }

    fn scheduling_body_operation_id(&self, operation_ids: &[String]) -> Option<String> {
        operation_ids
            .iter()
            .rev()
            .find(|operation_id| {
                let Some(operation) = self
                    .algorithm
                    .operation_sites
                    .iter()
                    .find(|site| &site.source.node_id == *operation_id)
                else {
                    return false;
                };
                let Some(segment) = self
                    .algorithm
                    .segments
                    .iter()
                    .find(|segment| segment.source.node_id == operation.segment_id)
                else {
                    return false;
                };
                segment.links.iter().any(|link| {
                    if link.id != operation.link_id {
                        return false;
                    }
                    let text = link.visible_text.to_ascii_lowercase();
                    is_body_receiving_operation(&text)
                })
            })
            .cloned()
    }

    fn named_body_operation_id(&self, operation_ids: &[String]) -> Option<String> {
        let scheduling_operations: Vec<&String> = operation_ids
            .iter()
            .filter(|operation_id| {
                let Some(operation) = self
                    .algorithm
                    .operation_sites
                    .iter()
                    .find(|site| &site.source.node_id == *operation_id)
                else {
                    return false;
                };
                let Some(segment) = self
                    .algorithm
                    .segments
                    .iter()
                    .find(|segment| segment.source.node_id == operation.segment_id)
                else {
                    return false;
                };
                segment.links.iter().any(|link| {
                    link.id == operation.link_id && {
                        let text = link.visible_text.to_ascii_lowercase();
                        is_body_receiving_operation(&text)
                    }
                })
            })
            .collect();
        match scheduling_operations.as_slice() {
            [operation_id] => Some((*operation_id).clone()),
            [] if operation_ids.len() == 1 => operation_ids.first().cloned(),
            _ => None,
        }
    }

    fn parse_ul_branches(
        &mut self,
        list: &ElementRef<'_>,
        step_id: &str,
        body_id: &str,
        path: &[u32],
    ) {
        for (position, item) in direct_children_named(list, "li").into_iter().enumerate() {
            let branch_source = source_identity(
                &self.ctx,
                "branch",
                &self.path(&item),
                Some(&(position + 1).to_string()),
            );
            let branch_id = branch_source.node_id.clone();
            let label = own_plain_text(&item);
            self.algorithm.branches.push(StructuralBranch {
                source: branch_source,
                parent_step_id: step_id.to_string(),
                label,
                label_text: String::new(),
                label_tokens: Vec::new(),
                label_links: Vec::new(),
                items: Vec::new(),
            });
            self.step_mut(step_id)
                .items
                .push(StepItem::Branch(branch_id.clone()));
            self.parse_branch_contents(&item, &branch_id, step_id, body_id, path);
        }
    }

    fn parse_dl_branches(
        &mut self,
        list: &ElementRef<'_>,
        step_id: &str,
        body_id: &str,
        path: &[u32],
    ) {
        let mut current: Option<String> = None;
        for child in list.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "dt" => {
                    let source = source_identity(&self.ctx, "branch", &self.path(&child), None);
                    let id = source.node_id.clone();
                    let mut label = CanonicalBuilder::new(&self.ctx);
                    for node in child.children() {
                        label.walk(node);
                    }
                    self.algorithm.branches.push(StructuralBranch {
                        source,
                        parent_step_id: step_id.to_string(),
                        label: normalize_plain_text(&child.text().collect::<String>()),
                        label_text: label.text.trim().to_string(),
                        label_tokens: label.tokens,
                        label_links: label.links,
                        items: Vec::new(),
                    });
                    self.step_mut(step_id)
                        .items
                        .push(StepItem::Branch(id.clone()));
                    current = Some(id);
                }
                "dd" => {
                    if let Some(branch_id) = &current {
                        let branch_id = branch_id.clone();
                        self.parse_branch_contents(&child, &branch_id, step_id, body_id, path);
                    }
                }
                _ => {}
            }
        }
    }

    fn parse_branch_contents(
        &mut self,
        container: &ElementRef<'_>,
        branch_id: &str,
        step_id: &str,
        body_id: &str,
        path: &[u32],
    ) {
        let first_new_item = self.step_mut(step_id).items.len();
        self.parse_step_children(container, step_id, body_id, path);
        let new_items = self
            .algorithm
            .steps
            .iter()
            .find(|step| step.source.node_id == step_id)
            .map(|step| step.items[first_new_item..].to_vec())
            .unwrap_or_default();
        if let Some(branch) = self
            .algorithm
            .branches
            .iter_mut()
            .find(|branch| branch.source.node_id == branch_id)
        {
            branch.items.extend(new_items);
        }
    }

    fn record_note(&mut self, element: &ElementRef<'_>, step_id: &str, body_id: &str) {
        let Some(kind) = note_kind(element) else {
            return;
        };
        let path = self.path(element);
        let source = source_identity(&self.ctx, "note", &path, None);
        let note_id = source.node_id.clone();
        let nodes: Vec<_> = element.children().collect();
        let canonical =
            canonical_segment(&nodes, source.clone(), Some(step_id), body_id, 0, &self.ctx);
        self.algorithm.notes.push(StructuralNote {
            source,
            kind,
            text: canonical.text,
            links: canonical.links,
        });
        self.step_mut(step_id).items.push(StepItem::Note(note_id));
    }

    fn parse_source_emu_alg(&mut self, emu_alg: &ElementRef<'_>, body_id: &str) {
        let line_re = source_step_re();
        let mut stack: Vec<(usize, String, Vec<u32>)> = Vec::new();
        let mut sibling_counts: HashMap<(Option<String>, usize), u32> = HashMap::new();
        for (line_index, raw_line) in emu_alg.inner_html().lines().enumerate() {
            let Some(captures) = line_re.captures(raw_line) else {
                continue;
            };
            let indent = captures.name("indent").map_or(0, |m| m.as_str().len());
            while stack.last().is_some_and(|(level, _, _)| *level >= indent) {
                stack.pop();
            }
            let parent_id = stack.last().map(|(_, id, _)| id.clone());
            let ordinal = sibling_counts
                .entry((parent_id.clone(), indent))
                .or_insert(0);
            *ordinal += 1;
            let mut path = stack
                .last()
                .map(|(_, _, path)| path.clone())
                .unwrap_or_default();
            path.push(*ordinal);
            let step_path = format!("{}/source-line-{}", self.path(emu_alg), line_index + 1);
            let source = source_identity(&self.ctx, "step", &step_path, None);
            let step_id = source.node_id.clone();
            self.algorithm.steps.push(StructuralStep {
                source,
                body_id: body_id.to_string(),
                parent_step_id: parent_id.clone(),
                path: path.clone(),
                ordinal: *ordinal,
                items: Vec::new(),
            });
            if let Some(parent_id) = parent_id {
                self.step_mut(&parent_id)
                    .items
                    .push(StepItem::ChildStep(step_id.clone()));
            } else {
                self.body_mut(body_id)
                    .items
                    .push(BodyItem::Step(step_id.clone()));
            }
            let fragment = Html::parse_fragment(captures.name("content").unwrap().as_str());
            let nodes: Vec<_> = fragment.root_element().children().collect();
            self.record_segment_nodes(&nodes, &step_path, &step_id, body_id);
            stack.push((indent, step_id, path));
        }
        if self.algorithm.steps.is_empty() {
            self.algorithm.issues.push(StructuralIssue {
                code: "unsupported_structure".to_string(),
                message: "source-form emu-alg contains no recognized numbered lines".to_string(),
                site_id: Some(self.algorithm.source.node_id.clone()),
            });
        }
    }

    fn finish_remainders(&mut self) {
        let pending = std::mem::take(&mut self.pending_continuations);
        let mut remainders: HashMap<(String, String), String> = HashMap::new();
        for continuation_index in pending {
            let enclosing_body_id = self.algorithm.continuations[continuation_index]
                .enclosing_body_id
                .clone();
            let continuation_step_id = self.algorithm.continuations[continuation_index]
                .step_id
                .clone();
            let Some(top_step_id) =
                self.top_level_step_for_body(&enclosing_body_id, &continuation_step_id)
            else {
                continue;
            };
            let key = (enclosing_body_id.clone(), top_step_id.clone());
            let remainder_id = if let Some(id) = remainders.get(&key) {
                id.clone()
            } else {
                let body_items = self.body(&enclosing_body_id).items.clone();
                let Some(cut) = body_items.iter().position(|item| {
                    let BodyItem::Step(id) = item else {
                        return false;
                    };
                    id == &top_step_id
                }) else {
                    continue;
                };
                let suffix: Vec<BodyItem> = body_items.iter().skip(cut + 1).cloned().collect();
                let step_ids: Vec<String> = suffix
                    .iter()
                    .filter_map(|item| match item {
                        BodyItem::Step(id) => Some(id.clone()),
                        _ => None,
                    })
                    .collect();
                if step_ids.is_empty() {
                    self.algorithm.issues.push(StructuralIssue {
                        code: "unsupported_structure".to_string(),
                        message: "continue-remaining-steps has no following sibling steps"
                            .to_string(),
                        site_id: Some(
                            self.algorithm.continuations[continuation_index]
                                .source
                                .node_id
                                .clone(),
                        ),
                    });
                    continue;
                }
                let path = format!("{enclosing_body_id}/remainder/{top_step_id}");
                let source = source_identity(&self.ctx, "body", &path, Some("remainder"));
                let id = source.node_id.clone();
                let range = BodyRange {
                    start_step_id: step_ids.first().unwrap().clone(),
                    end_step_id: step_ids.last().unwrap().clone(),
                };
                self.body_mut(&enclosing_body_id).items.truncate(cut + 1);
                self.algorithm.bodies.push(StructuralBody {
                    source,
                    kind: BodyKind::Remainder,
                    name: None,
                    parent_body_id: Some(enclosing_body_id.clone()),
                    defined_at_step_id: Some(
                        self.algorithm.continuations[continuation_index]
                            .step_id
                            .clone(),
                    ),
                    items: suffix,
                    range: Some(range),
                });
                for step_id in &step_ids {
                    self.reassign_step_body(step_id, &id);
                }
                remainders.insert(key.clone(), id.clone());
                id
            };
            self.algorithm.continuations[continuation_index].remainder_body_id =
                Some(remainder_id.clone());

            if self.algorithm.continuations[continuation_index].syntax == ContinuationSyntax::Queued
            {
                let continuation = self.algorithm.continuations[continuation_index].clone();
                let step_id = continuation.step_id;
                let clause_start = self
                    .algorithm
                    .segments
                    .iter()
                    .find(|segment| segment.source.node_id == continuation.segment_id)
                    .map_or(0, |segment| {
                        preceding_clause_boundary(&segment.text, continuation.span.start)
                    });
                let operation_ids: Vec<_> = self
                    .algorithm
                    .operation_sites
                    .iter()
                    .filter(|site| {
                        site.step_id == step_id
                            && site.segment_id == continuation.segment_id
                            && site.role == ReferenceRole::Operation
                            && site.span.start >= clause_start
                            && site.span.end <= continuation.span.start
                    })
                    .map(|site| site.source.node_id.clone())
                    .collect();
                // Argument links are not consumers of the remainder. If the
                // scheduling verb is unlinked, the continuation remains standalone.
                if let Some(operation_id) = self.scheduling_body_operation_id(&operation_ids) {
                    self.attach_body_argument(
                        &operation_id,
                        &step_id,
                        &remainder_id,
                        BodyArgumentBinding::Remainder,
                    );
                }
            }
        }
    }

    fn reassign_step_body(&mut self, step_id: &str, new_body_id: &str) {
        let child_ids: Vec<String> = self
            .step(step_id)
            .items
            .iter()
            .filter_map(|item| match item {
                StepItem::ChildStep(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.step_mut(step_id).body_id = new_body_id.to_string();
        for segment in self
            .algorithm
            .segments
            .iter_mut()
            .filter(|segment| segment.owner_step_id.as_deref() == Some(step_id))
        {
            segment.owner_body_id = new_body_id.to_string();
        }
        for operation in self
            .algorithm
            .operation_sites
            .iter_mut()
            .filter(|operation| operation.step_id == step_id)
        {
            operation.body_id = new_body_id.to_string();
        }
        for invocation in self
            .algorithm
            .body_invocations
            .iter_mut()
            .filter(|invocation| invocation.step_id == step_id)
        {
            invocation.scope_body_id = new_body_id.to_string();
        }
        let defined_body_ids: Vec<String> = self
            .algorithm
            .body_definitions
            .iter_mut()
            .filter(|definition| definition.step_id == step_id)
            .map(|definition| {
                definition.scope_body_id = new_body_id.to_string();
                definition.body_id.clone()
            })
            .collect();
        for defined_body_id in defined_body_ids {
            if let Some(body) = self
                .algorithm
                .bodies
                .iter_mut()
                .find(|body| body.source.node_id == defined_body_id)
            {
                body.parent_body_id = Some(new_body_id.to_string());
            }
        }
        for continuation in self
            .algorithm
            .continuations
            .iter_mut()
            .filter(|continuation| continuation.step_id == step_id)
        {
            continuation.enclosing_body_id = new_body_id.to_string();
        }
        for body in self
            .algorithm
            .bodies
            .iter_mut()
            .filter(|body| body.defined_at_step_id.as_deref() == Some(step_id))
        {
            body.parent_body_id = Some(new_body_id.to_string());
        }
        for child in child_ids {
            self.reassign_step_body(&child, new_body_id);
        }
    }

    fn top_level_step_for_body(&self, body_id: &str, step_id: &str) -> Option<String> {
        let direct_steps: HashSet<&str> = self
            .body(body_id)
            .items
            .iter()
            .filter_map(|item| match item {
                BodyItem::Step(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let mut current = step_id;
        loop {
            if direct_steps.contains(current) {
                return Some(current.to_string());
            }
            current = self.step(current).parent_step_id.as_deref()?;
        }
    }

    fn visible_bindings(&self, scope_body_id: &str, name: &str) -> Vec<String> {
        let mut current = Some(scope_body_id);
        while let Some(body_id) = current {
            if let Some(ids) = self
                .body_bindings
                .get(&(body_id.to_string(), name.to_ascii_lowercase()))
            {
                return ids.clone();
            }
            current = self.body(body_id).parent_body_id.as_deref();
        }
        Vec::new()
    }

    fn visible_binding_entries(&self, scope_body_id: &str) -> Vec<(String, String)> {
        let mut result = Vec::new();
        let mut seen_names = BTreeSet::new();
        let mut current = Some(scope_body_id);
        while let Some(body_id) = current {
            let start = (body_id.to_string(), String::new());
            for ((binding_body_id, name), ids) in self.body_bindings.range(start..) {
                if binding_body_id != body_id {
                    break;
                }
                if seen_names.insert(name.clone()) {
                    result.extend(ids.iter().map(|id| (name.clone(), id.clone())));
                }
            }
            current = self.body(body_id).parent_body_id.as_deref();
        }
        result
    }

    fn step_text(&self, step_id: &str) -> String {
        let segment_ids: Vec<&str> = self
            .step(step_id)
            .items
            .iter()
            .filter_map(|item| match item {
                StepItem::Segment(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        segment_ids
            .iter()
            .filter_map(|id| {
                self.algorithm
                    .segments
                    .iter()
                    .find(|segment| segment.source.node_id == *id)
            })
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn body(&self, id: &str) -> &StructuralBody {
        self.algorithm
            .bodies
            .iter()
            .find(|body| body.source.node_id == id)
            .expect("known body id")
    }

    fn body_mut(&mut self, id: &str) -> &mut StructuralBody {
        self.algorithm
            .bodies
            .iter_mut()
            .find(|body| body.source.node_id == id)
            .expect("known body id")
    }

    fn step(&self, id: &str) -> &StructuralStep {
        self.algorithm
            .steps
            .iter()
            .find(|step| step.source.node_id == id)
            .expect("known step id")
    }

    fn step_mut(&mut self, id: &str) -> &mut StructuralStep {
        self.algorithm
            .steps
            .iter_mut()
            .find(|step| step.source.node_id == id)
            .expect("known step id")
    }
}

struct CanonicalBuilder<'a, 'b> {
    ctx: &'a ExtractContext<'b>,
    text: String,
    tokens: Vec<InlineToken>,
    links: Vec<LinkSpan>,
    pending_space: bool,
    link_ordinal: usize,
    skip_nested_blocks: bool,
    mark: Option<ego_tree::NodeId>,
    mark_span: Option<TextSpan>,
    var_nodes: Vec<(TextSpan, ego_tree::NodeId)>,
}

impl<'a, 'b> CanonicalBuilder<'a, 'b> {
    fn new(ctx: &'a ExtractContext<'b>) -> Self {
        Self {
            ctx,
            text: String::new(),
            tokens: Vec::new(),
            links: Vec::new(),
            pending_space: false,
            link_ordinal: 0,
            skip_nested_blocks: false,
            mark: None,
            mark_span: None,
            var_nodes: Vec::new(),
        }
    }

    fn walk(&mut self, node: ego_tree::NodeRef<'_, Node>) {
        if self.mark == Some(node.id()) {
            let start = self.text.len();
            self.walk_node(node);
            // Spaces flushed at the element's edges are not part of it.
            let rendered = &self.text[start..];
            let start = start + (rendered.len() - rendered.trim_start_matches(' ').len());
            let end = start + self.text[start..].trim_end_matches(' ').len();
            if start < end {
                self.mark_span = Some(TextSpan { start, end });
            }
        } else {
            self.walk_node(node);
        }
    }

    fn walk_node(&mut self, node: ego_tree::NodeRef<'_, Node>) {
        match node.value() {
            Node::Text(text) => self.push_text(&text.text),
            Node::Element(element) => {
                let Some(element_ref) = ElementRef::wrap(node) else {
                    return;
                };
                match element.name() {
                    "var" => {
                        self.push_delimited(&element_ref, InlineTokenKind::Variable, '*');
                        let span = self.tokens.last().expect("variable token").span;
                        self.var_nodes.push((span, node.id()));
                    }
                    "code" | "samp" => {
                        let plain = element_ref.text().collect::<String>();
                        let kind = if looks_literal(&plain) {
                            InlineTokenKind::Literal
                        } else {
                            InlineTokenKind::Code
                        };
                        self.push_delimited(&element_ref, kind, '`');
                    }
                    "emu-val" | "emu-const" => {
                        self.push_delimited(&element_ref, InlineTokenKind::Literal, '`')
                    }
                    "a" | "emu-xref" => self.push_link(&element_ref),
                    "br" => self.pending_space = true,
                    _ => {
                        if self.skip_nested_blocks
                            && (matches!(element.name(), "ol" | "ul" | "dl")
                                || note_kind(&element_ref).is_some())
                        {
                            return;
                        }
                        for child in node.children() {
                            self.walk(child);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn push_text(&mut self, raw: &str) {
        let start = self.text.len();
        for character in raw.chars() {
            if character.is_whitespace() {
                self.pending_space = !self.text.is_empty();
                continue;
            }
            self.flush_space();
            if matches!(character, '\\' | '*' | '`') {
                self.text.push('\\');
            }
            self.text.push(character);
        }
        let end = self.text.len();
        if start < end {
            self.tokens.push(InlineToken {
                kind: InlineTokenKind::Text,
                span: TextSpan { start, end },
                source_text: raw.to_string(),
            });
        }
    }

    fn push_delimited(&mut self, element: &ElementRef<'_>, kind: InlineTokenKind, delimiter: char) {
        self.flush_space();
        let source_text = normalize_plain_text(&element.text().collect::<String>());
        let start = self.text.len();
        self.text.push(delimiter);
        // Canonical byte offset of every source byte offset, escapes included.
        let mut offsets = Vec::with_capacity(source_text.len() + 1);
        for (index, character) in source_text.char_indices() {
            offsets.resize(index + 1, self.text.len());
            if character == delimiter || character == '\\' {
                self.text.push('\\');
            }
            self.text.push(character);
        }
        offsets.resize(source_text.len() + 1, self.text.len());
        self.text.push(delimiter);
        let end = self.text.len();

        let mut cursor = 0;
        for link in element
            .descendants()
            .filter_map(ElementRef::wrap)
            .filter(|descendant| matches!(descendant.value().name(), "a" | "emu-xref"))
        {
            let link_text = normalize_plain_text(&link.text().collect::<String>());
            if link_text.is_empty() {
                continue;
            }
            let span = if link_text == source_text {
                TextSpan { start, end }
            } else if let Some(found) = source_text[cursor..].find(&link_text) {
                let from = cursor + found;
                cursor = from + link_text.len();
                TextSpan {
                    start: offsets[from],
                    end: offsets[cursor],
                }
            } else {
                continue;
            };
            self.record_link(&link, span);
        }

        self.tokens.push(InlineToken {
            kind,
            span: TextSpan { start, end },
            source_text,
        });
    }

    fn push_link(&mut self, link: &ElementRef<'_>) {
        self.flush_space();
        let start = self.text.len();
        for child in link.children() {
            self.walk(child);
        }
        let end = self.text.len();
        if start == end {
            return;
        }
        self.record_link(link, TextSpan { start, end });
    }

    fn record_link(&mut self, link: &ElementRef<'_>, span: TextSpan) {
        self.link_ordinal += 1;
        let href = link
            .value()
            .attr("href")
            .map(str::to_string)
            .or_else(|| link.value().attr("aoid").map(|aoid| format!("aoid:{aoid}")))
            .unwrap_or_default();
        let target = resolve_target(&href, self.ctx.spec, self.ctx.base_url);
        let id = link
            .value()
            .attr("id")
            .map(str::to_string)
            .unwrap_or_else(|| {
                stable_id(
                    self.ctx,
                    "link",
                    &algorithm_path(&self.ctx.owner, link),
                    Some(&self.link_ordinal.to_string()),
                )
            });
        self.links.push(LinkSpan {
            id,
            span,
            visible_text: self.text[span.start..span.end].to_string(),
            href,
            target,
            generator_id: link.value().attr("id").map(str::to_string),
            link_type: link.value().attr("data-link-type").map(str::to_string),
        });
    }

    fn flush_space(&mut self) {
        if self.pending_space && !self.text.is_empty() && !self.text.ends_with(' ') {
            self.text.push(' ');
        }
        self.pending_space = false;
    }
}

fn canonical_segment(
    nodes: &[ego_tree::NodeRef<'_, Node>],
    source: SourceIdentity,
    owner_step_id: Option<&str>,
    owner_body_id: &str,
    ordinal: u32,
    ctx: &ExtractContext<'_>,
) -> StructuralSegment {
    let mut builder = CanonicalBuilder::new(ctx);
    for node in nodes {
        builder.walk(*node);
    }
    StructuralSegment {
        source,
        owner_step_id: owner_step_id.map(str::to_string),
        owner_body_id: owner_body_id.to_string(),
        ordinal,
        text: builder.text.trim().to_string(),
        tokens: builder.tokens,
        links: builder.links,
    }
}

fn split_inline_body(
    segment: &StructuralSegment,
) -> Option<(StructuralSegment, StructuralSegment, String)> {
    if segment.links.len() < 2 {
        return None;
    }
    let lower = segment.text.to_ascii_lowercase();
    if !(lower.contains("queue a ") || lower.contains("enqueue")) {
        return None;
    }
    let mut split = None;
    for pair in segment.links.windows(2) {
        let queue = &pair[0];
        let body_operation = &pair[1];
        let queue_context = lower.get(..queue.span.end)?;
        if !(queue_context.contains("queue") || queue_context.contains("enqueue")) {
            continue;
        }
        let between = lower.get(queue.span.end..body_operation.span.start)?;
        if let Some(relative) = between.rfind(" to ") {
            split = Some(queue.span.end + relative);
            break;
        }
    }
    let split = split?;
    let initiating_link_id = segment
        .links
        .iter()
        .filter(|link| link.span.end <= split)
        .rev()
        .find(|link| {
            let text = link.visible_text.to_ascii_lowercase();
            is_body_receiving_operation(&text)
        })?
        .id
        .clone();
    let suffix_start = split + 4;
    if !segment.links.iter().any(|link| link.span.end <= split)
        || !segment
            .links
            .iter()
            .any(|link| link.span.start >= suffix_start)
    {
        return None;
    }
    let before = slice_segment(segment, 0, split, "prefix")?;
    let after = slice_segment(segment, suffix_start, segment.text.len(), "body")?;
    Some((before, after, initiating_link_id))
}

fn split_inline_named_definition(
    segment: &StructuralSegment,
) -> Option<(String, StructuralSegment, StructuralSegment)> {
    let captures = inline_body_definition_re().captures(&segment.text)?;
    let whole = captures.get(0)?;
    let name = captures
        .name("name")?
        .as_str()
        .trim_matches('*')
        .to_string();
    if whole.end() >= segment.text.len()
        || !segment
            .links
            .iter()
            .any(|link| link.span.start >= whole.end())
    {
        return None;
    }
    let before = slice_segment(segment, 0, whole.end(), "definition")?;
    let body = slice_segment(segment, whole.end(), segment.text.len(), "defined-body")?;
    Some((name, before, body))
}

fn slice_segment(
    segment: &StructuralSegment,
    start: usize,
    end: usize,
    suffix: &str,
) -> Option<StructuralSegment> {
    let raw = segment.text.get(start..end)?;
    let leading = raw.len() - raw.trim_start().len();
    let trailing = raw.trim_end().len();
    let real_start = start + leading;
    let real_end = start + trailing;
    if real_start >= real_end {
        return None;
    }
    let adjust_span = |span: TextSpan| {
        if span.start >= real_start && span.end <= real_end {
            Some(TextSpan {
                start: span.start - real_start,
                end: span.end - real_start,
            })
        } else {
            None
        }
    };
    let mut result = segment.clone();
    result.source.node_id = format!("{}-{suffix}", segment.source.node_id);
    result.text = segment.text[real_start..real_end].to_string();
    result.tokens = segment
        .tokens
        .iter()
        .filter_map(|token| {
            adjust_span(token.span).map(|span| InlineToken {
                kind: token.kind,
                span,
                source_text: token.source_text.clone(),
            })
        })
        .collect();
    result.links = segment
        .links
        .iter()
        .filter_map(|link| {
            adjust_span(link.span).map(|span| LinkSpan {
                span,
                ..link.clone()
            })
        })
        .collect();
    Some(result)
}

fn source_identity(
    ctx: &ExtractContext<'_>,
    kind: &str,
    path: &str,
    suffix: Option<&str>,
) -> SourceIdentity {
    SourceIdentity {
        spec: ctx.spec.to_string(),
        snapshot_sha: ctx.snapshot_sha.to_string(),
        section_anchor: ctx.anchor.to_string(),
        node_id: stable_id(ctx, kind, path, suffix),
        url: canonical_url(ctx.base_url, ctx.anchor),
    }
}

fn stable_id(ctx: &ExtractContext<'_>, kind: &str, path: &str, suffix: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    for component in [ctx.spec, ctx.anchor, kind, path, suffix.unwrap_or_default()] {
        hasher.update(component.as_bytes());
        hasher.update([0]);
    }
    format!("src-{:x}", hasher.finalize())
}

fn canonical_url(base_url: &str, anchor: &str) -> String {
    let base = base_url.split('#').next().unwrap_or(base_url);
    format!("{base}#{anchor}")
}

fn resolve_target(href: &str, current_spec: &str, base_url: &str) -> Option<AnchorTarget> {
    if let Some(anchor) = href.strip_prefix('#') {
        return Some(AnchorTarget {
            spec: current_spec.to_string(),
            anchor: anchor.to_string(),
        });
    }
    if href.starts_with("aoid:") {
        return None;
    }
    let absolute = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else {
        url::Url::parse(base_url).ok()?.join(href).ok()?.to_string()
    };
    let registry = crate::spec_registry::SpecRegistry::new();
    if let Some((spec, anchor)) = registry.resolve_url(&absolute) {
        return Some(AnchorTarget { spec, anchor });
    }
    // A relative cross-page link remains within the current multipage spec,
    // even when a synthetic or newly discovered spec is absent from the
    // built-in registry.
    if !href.starts_with("http://") && !href.starts_with("https://") {
        return url::Url::parse(&absolute)
            .ok()?
            .fragment()
            .map(|anchor| AnchorTarget {
                spec: current_spec.to_string(),
                anchor: anchor.to_string(),
            });
    }
    None
}

/// Path of `element` relative to the algorithm owner: `.` for the owner, a
/// path below it, or `+k/…` below the owner's k-th following element sibling
/// (the Wattsi `<p>To <dfn>…</dfn>:</p><ol>` pattern). Nothing before the
/// algorithm in the document affects it.
fn algorithm_path(owner: &ElementRef<'_>, element: &ElementRef<'_>) -> String {
    let mut parts = Vec::new();
    let mut current = *element;
    loop {
        if current.id() == owner.id() {
            parts.push(".".to_string());
            break;
        }
        let same_parent = current.parent().map(|p| p.id()) == owner.parent().map(|p| p.id());
        if same_parent {
            if let Some(k) = owner
                .next_siblings()
                .filter_map(ElementRef::wrap)
                .position(|sibling| sibling.id() == current.id())
            {
                parts.push(format!("+{}", k + 1));
                break;
            }
        }
        let Some(parent) = current.parent().and_then(ElementRef::wrap) else {
            parts.push("^".to_string());
            break;
        };
        let index = parent
            .children()
            .filter_map(ElementRef::wrap)
            .take_while(|sibling| sibling.id() != current.id())
            .filter(|sibling| sibling.value().name() == current.value().name())
            .count()
            + 1;
        parts.push(format!("{}[{index}]", current.value().name()));
        current = parent;
    }
    parts.reverse();
    parts.join("/")
}

fn direct_child_named<'a>(element: &ElementRef<'a>, name: &str) -> Option<ElementRef<'a>> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .find(|child| child.value().name() == name)
}

fn direct_children_named<'a>(element: &ElementRef<'a>, name: &str) -> Vec<ElementRef<'a>> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|child| child.value().name() == name)
        .collect()
}

fn first_outer_list<'a>(container: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let selector = Selector::parse("ol").ok()?;
    container.select(&selector).find(|list| {
        list.ancestors()
            .filter_map(ElementRef::wrap)
            .take_while(|ancestor| ancestor.id() != container.id())
            .all(|ancestor| ancestor.value().name() != "ol")
    })
}

fn first_definition_outside_list<'a>(container: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let selector = Selector::parse("dfn[id]").ok()?;
    container.select(&selector).find(|dfn| {
        dfn.ancestors()
            .filter_map(ElementRef::wrap)
            .take_while(|ancestor| ancestor.id() != container.id())
            .all(|ancestor| !matches!(ancestor.value().name(), "ol" | "ul" | "dl"))
    })
}

/// `(dfn, intro block, steps)` for every `<ol>` of `container` outside other
/// lists whose previous element sibling is a `p`, `div` or `dd` that defines a
/// term outside lists and ends in a colon, in document order. The colon keeps
/// a definition that sits between an algorithm's intro and its steps ("Parts
/// marked fragment case …") from taking the steps over.
fn algorithm_pairs<'a>(
    container: &ElementRef<'a>,
) -> Vec<(ElementRef<'a>, ElementRef<'a>, ElementRef<'a>)> {
    let selector = Selector::parse("ol").expect("valid selector");
    container
        .select(&selector)
        .filter(|list| {
            list.ancestors()
                .filter_map(ElementRef::wrap)
                .take_while(|ancestor| ancestor.id() != container.id())
                .all(|ancestor| !matches!(ancestor.value().name(), "ol" | "ul"))
        })
        .filter_map(|list| {
            let intro = list
                .prev_siblings()
                .find(|sibling| match sibling.value() {
                    Node::Text(text) => !text.text.trim().is_empty(),
                    _ => true,
                })
                .and_then(ElementRef::wrap)?;
            if !matches!(intro.value().name(), "p" | "div" | "dd")
                || !own_plain_text(&intro).ends_with(':')
            {
                return None;
            }
            let dfn = first_definition_outside_list(&intro)?;
            Some((dfn, intro, list))
        })
        .collect()
}

fn following_algorithm_block<'a>(owner: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    for sibling in owner.next_siblings() {
        match sibling.value() {
            Node::Text(text) if text.text.trim().is_empty() => continue,
            Node::Element(_) => {
                let element = ElementRef::wrap(sibling)?;
                let is_switch = element.value().name() == "dl"
                    && element.value().classes().any(|class| class == "switch");
                return (element.value().name() == "ol" || is_switch).then_some(element);
            }
            _ => return None,
        }
    }
    None
}

fn has_algorithm_ancestor(element: &ElementRef<'_>) -> bool {
    element
        .ancestors()
        .skip(1)
        .filter_map(ElementRef::wrap)
        .any(|ancestor| {
            ancestor.value().name() == "div"
                && (ancestor.value().classes().any(|class| class == "algorithm")
                    || ancestor.value().attr("data-algorithm").is_some())
        })
}

fn inside_emu_clause(element: &ElementRef<'_>) -> bool {
    element
        .ancestors()
        .skip(1)
        .filter_map(ElementRef::wrap)
        .any(|ancestor| matches!(ancestor.value().name(), "emu-clause" | "emu-annex"))
}

fn nonempty_text(element: &ElementRef<'_>) -> Option<String> {
    let text = normalize_plain_text(&element.text().collect::<String>());
    (!text.is_empty()).then_some(text)
}

fn normalize_plain_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn own_plain_text(element: &ElementRef<'_>) -> String {
    let mut text = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(value) => text.push_str(&value.text),
            Node::Element(value) if matches!(value.name(), "ol" | "ul" | "dl") => {}
            Node::Element(_) => {
                if let Some(child_element) = ElementRef::wrap(child) {
                    text.push_str(&child_element.text().collect::<String>());
                }
            }
            _ => {}
        }
    }
    normalize_plain_text(&text)
}

fn is_block_element(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "section" | "aside" | "blockquote" | "pre" | "figure" | "table"
    )
}

fn note_kind(element: &ElementRef<'_>) -> Option<NoteKind> {
    if element.value().classes().any(|class| class == "note") {
        Some(NoteKind::Note)
    } else if element.value().classes().any(|class| class == "example") {
        Some(NoteKind::Example)
    } else if element.value().classes().any(|class| class == "warning") {
        Some(NoteKind::Warning)
    } else if element.value().classes().any(|class| class == "advisement") {
        Some(NoteKind::Advisement)
    } else {
        None
    }
}

fn looks_literal(text: &str) -> bool {
    let trimmed = text.trim();
    (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || matches!(trimmed, "true" | "false" | "null" | "undefined")
}

fn is_mention_context(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("see ") || lower.contains(" see the ") || lower.contains(" for background")
}

fn link_is_in_mention_context(segment: &StructuralSegment, link: &LinkSpan) -> bool {
    let start = preceding_clause_boundary(&segment.text, link.span.start);
    let end = following_clause_boundary(&segment.text, link.span.end);
    is_mention_context(&segment.text[start..end])
}

fn preceding_clause_boundary(text: &str, offset: usize) -> usize {
    text[..offset]
        .rfind(['.', ';'])
        .map_or(0, |boundary| boundary + 1)
}

fn following_clause_boundary(text: &str, offset: usize) -> usize {
    text[offset..]
        .find(['.', ';'])
        .map_or(text.len(), |boundary| offset + boundary)
}

fn link_is_type_mention(segment: &StructuralSegment, link: &LinkSpan) -> bool {
    if link.link_type.as_deref().is_some_and(|link_type| {
        matches!(
            link_type,
            "element" | "interface" | "dictionary" | "enum" | "typedef" | "attribute" | "type"
        )
    }) {
        return true;
    }
    let text = link.visible_text.to_ascii_lowercase();
    if text.ends_with(" element") || text.ends_with(" interface") || text.ends_with(" type") {
        return true;
    }
    let before = segment.text[..link.span.start]
        .trim_end()
        .to_ascii_lowercase();
    let after = segment.text[link.span.end..]
        .trim_start()
        .to_ascii_lowercase();
    (before.ends_with(" is a") || before.ends_with(" is an"))
        && (after.starts_with("element")
            || after.starts_with("interface")
            || after.starts_with("type"))
}

fn body_definition_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:let|set)\s+\*?(?P<name>[A-Za-z][A-Za-z0-9_-]*(?:\s+[A-Za-z][A-Za-z0-9_-]*){0,4})\*?\s+(?:be|to)\s+(?:the\s+)?following\s+steps\b",
        )
        .unwrap()
    })
}

fn inline_body_definition_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:let|set)\s+\*?(?P<name>[A-Za-z][A-Za-z0-9_-]*)\*?\s+(?:be|to)\s+an\s+algorithm\s+step\s+which\s+is\s+to\s+",
        )
        .unwrap()
    })
}

fn body_definition_name(text: &str) -> Option<String> {
    body_definition_re()
        .captures(text)
        .and_then(|captures| captures.name("name"))
        .map(|name| name.as_str().trim_matches('*').to_string())
}

fn body_invocation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:(?P<run>run)|(?P<perform>perform))\s+(?:the\s+)?\*?(?P<name>[A-Za-z][A-Za-z0-9_-]*)\*?\b",
        )
        .unwrap()
    })
}

fn continue_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bcontinue\s+(?:(?:with\s+)?the\s+remaining|these)\s+steps\b").unwrap()
    })
}

fn continuation_syntax(text: &str, start: usize, end: usize) -> ContinuationSyntax {
    let clause_start = text[..start]
        .rfind(['.', ';'])
        .map_or(0, |boundary| boundary + 1);
    let clause_end = text[end..]
        .find(['.', ';'])
        .map_or(text.len(), |boundary| end + boundary);
    let clause = text[clause_start..clause_end].to_ascii_lowercase();
    if clause.contains("queue") || clause.contains("enqueue") || clause.contains("task") {
        ContinuationSyntax::Queued
    } else {
        // An explicit continuation with no scheduling syntax stays in the
        // current flow. The analysis layer still decides how a linked queue
        // operation behaves when such syntax is present.
        ContinuationSyntax::Inline
    }
}

fn source_step_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?P<indent>\s*)\d+\.\s*(?P<content>.*)\s*$").unwrap())
}

fn is_body_receiving_operation(lower: &str) -> bool {
    lower.starts_with("queue ")
        || lower.starts_with("enqueue ")
        || (lower.starts_with("append ") && lower.contains("steps"))
}

fn anonymous_body_intro(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    static FOLLOWING_STEPS: OnceLock<Regex> = OnceLock::new();
    static RUN_STEPS: OnceLock<Regex> = OnceLock::new();
    (lower.contains("following steps")
        || lower.contains("following substeps")
        || RUN_STEPS
            .get_or_init(|| {
                Regex::new(r"\b(?:run|perform|execute) (?:the|these) (?:sub)?steps\b").unwrap()
            })
            .is_match(&lower)
        || (lower.contains("append")
            && FOLLOWING_STEPS
                .get_or_init(|| Regex::new(r"\bfollowing(?: [a-z-]+)* steps\b").unwrap())
                .is_match(&lower)))
        && (lower.contains("queue")
            || (lower.contains("append") && lower.contains("steps"))
            || lower.contains("parallel")
            || lower.contains("completion")
            || lower.contains("success")
            || lower.contains("failure"))
}

fn explicit_body_argument_spans(segment: &StructuralSegment, name: &str) -> Vec<TextSpan> {
    let escaped = regex::escape(name);
    let Ok(pattern) = Regex::new(&format!(
        r"(?i)(?:given|with|using|passing|,|\()\s+(?:the\s+)?\*?(?P<name>{escaped})\*?(?:\s|[,.;)]|$)"
    )) else {
        return Vec::new();
    };
    pattern
        .captures_iter(&segment.text)
        .filter_map(|captures| captures.name("name"))
        .filter(|name_match| {
            segment.tokens.iter().any(|token| {
                token.kind == InlineTokenKind::Variable
                    && token.span.start <= name_match.start()
                    && token.span.end >= name_match.end()
            })
        })
        .map(|name_match| TextSpan {
            start: name_match.start(),
            end: name_match.end(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://test.example/spec";
    const SHA: &str = "0123456789abcdef";

    fn fixture(name: &str) -> StructuralSpec {
        let html = match name {
            "wattsi" => include_str!("../../tests/fixtures/effects/structure/wattsi.html"),
            "bikeshed" => include_str!("../../tests/fixtures/effects/structure/bikeshed.html"),
            "ecmarkup" => include_str!("../../tests/fixtures/effects/structure/ecmarkup.html"),
            "remainder" => {
                include_str!("../../tests/fixtures/effects/structure/conditional_remainder.html")
            }
            _ => panic!("unknown fixture"),
        };
        extract_step_structure(html, "TEST", BASE, SHA)
    }

    fn algorithm<'a>(spec: &'a StructuralSpec, anchor: &str) -> &'a StructuralAlgorithm {
        spec.algorithms
            .iter()
            .find(|algorithm| algorithm.source.section_anchor == anchor)
            .unwrap()
    }

    #[test]
    fn wattsi_extracts_named_definition_and_invocation_without_executing_definition() {
        let spec = fixture("wattsi");
        let algorithm = algorithm(&spec, "named-definition");
        assert_eq!(algorithm.steps.len(), 3);
        assert_eq!(algorithm.body_definitions.len(), 1);
        assert_eq!(algorithm.body_definitions[0].name, "A");
        assert_eq!(algorithm.body_invocations.len(), 1);
        assert_eq!(algorithm.body_invocations[0].candidate_body_ids.len(), 1);
        let named = algorithm
            .bodies
            .iter()
            .find(|body| body.kind == BodyKind::Named)
            .unwrap();
        assert_ne!(named.source.node_id, algorithm.root_body_id);
        assert!(algorithm.steps.iter().any(|step| {
            step.body_id == named.source.node_id
                && step.path == vec![1, 1]
                && step
                    .items
                    .iter()
                    .any(|item| matches!(item, StepItem::Segment(_)))
        }));
    }

    #[test]
    fn bikeshed_keeps_callback_arguments_local_to_each_caller() {
        let spec = fixture("bikeshed");
        let r = algorithm(&spec, "caller-r");
        let s = algorithm(&spec, "caller-s");
        assert_eq!(r.body_arguments.len(), 1);
        assert_eq!(s.body_arguments.len(), 1);
        let r_body = &r.body_arguments[0].body_id;
        let s_body = &s.body_arguments[0].body_id;
        assert_ne!(r_body, s_body);
        assert!(r
            .operation_sites
            .iter()
            .all(|site| !site.actual_body_ids.iter().any(|id| id == s_body)));
        assert!(s
            .operation_sites
            .iter()
            .all(|site| !site.actual_body_ids.iter().any(|id| id == r_body)));
    }

    #[test]
    fn same_segment_queue_creates_anonymous_body_and_distinct_operations() {
        let spec = fixture("bikeshed");
        let algorithm = algorithm(&spec, "same-segment");
        let anonymous = algorithm
            .bodies
            .iter()
            .find(|body| body.kind == BodyKind::Anonymous)
            .unwrap();
        assert_eq!(algorithm.operation_sites.len(), 2);
        assert_ne!(
            algorithm.operation_sites[0].segment_id,
            algorithm.operation_sites[1].segment_id
        );
        assert_eq!(algorithm.body_arguments.len(), 1);
        assert!(algorithm.operation_sites.iter().any(|site| {
            site.target.as_ref()
                == Some(&AnchorTarget {
                    spec: "TEST".to_string(),
                    anchor: "fire".to_string(),
                })
        }));
        assert_eq!(
            algorithm.body_arguments[0].body_id,
            anonymous.source.node_id
        );
        assert_eq!(
            algorithm.operation_sites[1].body_id,
            anonymous.source.node_id
        );
        let body_segment = algorithm
            .segments
            .iter()
            .find(|segment| segment.source.node_id == algorithm.operation_sites[1].segment_id)
            .unwrap();
        assert_eq!(body_segment.owner_body_id, anonymous.source.node_id);
    }

    #[test]
    fn inline_queue_body_binds_to_queue_before_intermediate_argument_links() {
        let html = r##"
          <div class="algorithm">
            <p>To <dfn id="hashchange">update history</dfn>:</p>
            <ol><li><p>If the old URL's <a href="#fragment">fragment</a> changed,
              then <a href="#queue">queue a global task</a> on the
              <a href="#task-source">DOM manipulation task source</a> given the document's
              <a href="#global">relevant global object</a> to
              <a href="#fire">fire an event</a> named <code>hashchange</code>.</p></li></ol>
          </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "hashchange");
        let argument = algorithm.body_arguments.first().unwrap();
        let owner = algorithm
            .operation_sites
            .iter()
            .find(|operation| operation.source.node_id == argument.operation_site_id)
            .unwrap();

        assert_eq!(owner.target.as_ref().unwrap().anchor, "queue");
        assert!(algorithm.operation_sites.iter().any(|operation| {
            operation
                .target
                .as_ref()
                .map(|target| target.anchor.as_str())
                == Some("fire")
                && operation.body_id == argument.body_id
        }));
    }

    #[test]
    fn queued_remainder_binds_to_scheduler_not_linked_arguments() {
        let html = r##"<div class="algorithm"><p>To <dfn id="remainder">run remainder</dfn>:</p><ol>
            <li><a href="#queue">Queue a global task</a> on the <a href="#source">task source</a>
                given the <a href="#window">active window</a> to continue these steps.</li>
            <li><a href="#fire">Fire an event</a>.</li></ol></div>"##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "remainder");
        let argument = algorithm
            .body_arguments
            .iter()
            .find(|argument| argument.binding == BodyArgumentBinding::Remainder)
            .unwrap();
        let operation = algorithm
            .operation_sites
            .iter()
            .find(|op| op.source.node_id == argument.operation_site_id)
            .unwrap();
        assert_eq!(operation.target.as_ref().unwrap().anchor, "queue");
        assert!(algorithm
            .operation_sites
            .iter()
            .filter(|op| op
                .target
                .as_ref()
                .is_some_and(|target| target.anchor != "queue"))
            .all(|op| op.actual_body_ids.is_empty()));
    }

    #[test]
    fn unlinked_queue_does_not_bind_remainder_to_an_argument_or_previous_clause() {
        let html = r##"<div class="algorithm"><p>To <dfn id="remainder">run remainder</dfn>:</p><ol>
            <li><a href="#earlier">Queue a task</a>.
                Queue a task given the <a href="#window">active window</a> to continue these steps.</li>
            <li>Return.</li></ol></div>"##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "remainder");
        assert!(algorithm
            .body_arguments
            .iter()
            .all(|argument| argument.binding != BodyArgumentBinding::Remainder));
        assert!(algorithm
            .continuations
            .iter()
            .any(
                |continuation| continuation.syntax == ContinuationSyntax::Queued
                    && continuation.remainder_body_id.is_some()
            ));
    }

    #[test]
    fn block_and_named_queue_bodies_skip_intermediate_argument_links() {
        let html = r##"
          <div class="algorithm">
            <p>To <dfn id="block-body">queue a block body</dfn>:</p>
            <ol><li><p><a href="#queue-block">Queue a task</a> on the
              <a href="#task-source">task source</a> given the
              <a href="#global">relevant global object</a> to run the following steps:</p>
              <ol><li><p><a href="#fire">Fire an event</a>.</p></li></ol>
            </li></ol>
          </div>
          <div class="algorithm">
            <p>To <dfn id="named-body">queue a named body</dfn>:</p>
            <ol>
              <li><p>Let <var>A</var> be the following steps:</p>
                <ol><li><p><a href="#fire">Fire an event</a>.</p></li></ol>
              </li>
              <li><p><a href="#queue-named">Queue a task</a> on the
                <a href="#task-source">task source</a> given the
                <a href="#global">relevant global object</a>, with <var>A</var>.</p></li>
            </ol>
          </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);

        for (anchor, queue) in [("block-body", "queue-block"), ("named-body", "queue-named")] {
            let algorithm = algorithm(&spec, anchor);
            let argument = algorithm.body_arguments.first().unwrap();
            let owner = algorithm
                .operation_sites
                .iter()
                .find(|operation| operation.source.node_id == argument.operation_site_id)
                .unwrap();
            assert_eq!(owner.target.as_ref().unwrap().anchor, queue);
        }
    }

    #[test]
    fn mention_context_does_not_hide_a_later_operation_in_the_same_segment() {
        let html = r##"
          <div class="algorithm">
            <p>To <dfn id="mixed-reference-roles">use mixed reference roles</dfn>:</p>
            <ol><li><p>See <a href="#background">the background algorithm</a> for
              background; then <a href="#fire">fire an event</a> named
              <code>a</code>.</p></li></ol>
          </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "mixed-reference-roles");
        let role = |anchor: &str| {
            algorithm
                .operation_sites
                .iter()
                .find(|site| {
                    site.target
                        .as_ref()
                        .is_some_and(|target| target.anchor == anchor)
                })
                .unwrap()
                .role
        };

        assert_eq!(role("background"), ReferenceRole::Mention);
        assert_eq!(role("fire"), ReferenceRole::Operation);
    }

    #[test]
    fn named_bodies_bind_to_separate_calls_in_one_segment() {
        let html = r##"
          <div class="algorithm">
            <p>To <dfn id="two-body-arguments">pass two body arguments</dfn>:</p>
            <ol>
              <li><p>Let <var>A</var> be the following steps:</p>
                <ol><li><p>Return.</p></li></ol>
              </li>
              <li><p>Let <var>B</var> be the following steps:</p>
                <ol><li><p>Return.</p></li></ol>
              </li>
              <li><p>Call <a href="#inline-op">the inline operation</a>, given
                <var>A</var>; <a href="#queue">queue a microtask</a>, given
                <var>B</var>.</p></li>
            </ol>
          </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "two-body-arguments");
        let body_id = |name: &str| {
            algorithm
                .body_definitions
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .body_id
                .clone()
        };
        let argument_body_for = |anchor: &str| {
            let operation = algorithm
                .operation_sites
                .iter()
                .find(|site| {
                    site.target
                        .as_ref()
                        .is_some_and(|target| target.anchor == anchor)
                })
                .unwrap();
            algorithm
                .body_arguments
                .iter()
                .filter(|argument| argument.operation_site_id == operation.source.node_id)
                .map(|argument| argument.body_id.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(argument_body_for("inline-op"), vec![body_id("A")]);
        assert_eq!(argument_body_for("queue"), vec![body_id("B")]);
    }

    #[test]
    fn ambiguous_named_body_recipient_is_left_unbound() {
        let html = r##"
          <div class="algorithm">
            <p>To <dfn id="ambiguous-body-argument">pass an ambiguous body argument</dfn>:</p>
            <ol>
              <li><p>Let <var>A</var> be the following steps:</p>
                <ol><li><p>Return.</p></li></ol>
              </li>
              <li><p>Call <a href="#first">the first operation</a> with
                <a href="#second">the second operation</a>, given <var>A</var>.</p></li>
            </ol>
          </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "ambiguous-body-argument");

        assert!(algorithm.body_arguments.is_empty());
        assert!(algorithm
            .operation_sites
            .iter()
            .all(|operation| operation.actual_body_ids.is_empty()));
        assert!(algorithm.issues.iter().any(|issue| {
            issue.code == "unresolved_body_binding"
                && issue.site_id.as_deref().is_some_and(|site_id| {
                    algorithm.segments.iter().any(|segment| {
                        segment.source.node_id == site_id && segment.text.contains("given *A*")
                    })
                })
        }));
    }

    #[test]
    fn conditional_continuation_moves_suffix_to_one_remainder_body() {
        let spec = fixture("remainder");
        let algorithm = algorithm(&spec, "conditional-remainder");
        assert_eq!(algorithm.continuations.len(), 2);
        assert_eq!(
            algorithm.continuations[0].syntax,
            ContinuationSyntax::Inline
        );
        assert_eq!(
            algorithm.continuations[1].syntax,
            ContinuationSyntax::Queued
        );
        let remainder_ids: HashSet<_> = algorithm
            .continuations
            .iter()
            .map(|site| site.remainder_body_id.as_deref().unwrap())
            .collect();
        assert_eq!(remainder_ids.len(), 1);
        let remainder = algorithm
            .bodies
            .iter()
            .find(|body| body.kind == BodyKind::Remainder)
            .unwrap();
        assert!(remainder.range.is_some());
        assert_eq!(algorithm.body(&algorithm.root_body_id).items.len(), 1);
        let step_two = algorithm
            .steps
            .iter()
            .find(|step| step.path == vec![2])
            .unwrap();
        assert_eq!(step_two.body_id, remainder.source.node_id);
        assert!(algorithm
            .segments
            .iter()
            .filter(|segment| segment.owner_step_id.as_deref() == Some(&step_two.source.node_id))
            .all(|segment| segment.owner_body_id == remainder.source.node_id));
        assert!(algorithm
            .operation_sites
            .iter()
            .filter(|site| site.step_id == step_two.source.node_id)
            .all(|site| site.body_id == remainder.source.node_id));
        assert_eq!(
            algorithm
                .body_arguments
                .iter()
                .filter(|argument| argument.binding == BodyArgumentBinding::Remainder)
                .count(),
            1
        );
    }

    #[test]
    fn ecmarkup_preserves_typed_tokens_link_spans_and_note_distinction() {
        let spec = fixture("ecmarkup");
        let algorithm = algorithm(&spec, "sec-ecma-operation");
        assert_eq!(algorithm.steps.len(), 2);
        let segment = algorithm
            .segments
            .iter()
            .find(|segment| segment.text.contains("Call"))
            .unwrap();
        assert!(segment
            .tokens
            .iter()
            .any(|token| token.kind == InlineTokenKind::Variable));
        assert!(segment
            .tokens
            .iter()
            .any(|token| token.kind == InlineTokenKind::Literal));
        let link = &segment.links[0];
        assert_eq!(
            &segment.text[link.span.start..link.span.end],
            link.visible_text
        );
        assert_eq!(algorithm.notes.len(), 1);
        assert!(algorithm.operation_sites.iter().any(|site| {
            site.role == ReferenceRole::Mention
                && site.target.as_ref().map(|target| target.anchor.as_str()) == Some("iframe")
        }));
        assert!(algorithm.operation_sites.iter().all(|site| site
            .target
            .as_ref()
            .map(|target| target.anchor.as_str())
            != Some("note-target")));
    }

    #[test]
    fn ecmarkup_aoid_links_resolve_only_unique_local_targets() {
        let html = r#"
            <emu-clause id="source"><h1>Source</h1><emu-alg><ol><li>
              Perform <emu-xref aoid="Called">Called</emu-xref>.
              Perform <emu-xref aoid="Ambiguous">Ambiguous</emu-xref>.
              Perform <emu-xref aoid="Unknown">Unknown</emu-xref>.
            </li></ol></emu-alg></emu-clause>
            <emu-clause id="called" aoid="Called"><h1>Called</h1><emu-alg><ol><li>Return.</li></ol></emu-alg></emu-clause>
            <emu-clause id="ambiguous-one" aoid="Ambiguous"><h1>First</h1></emu-clause>
            <emu-clause id="ambiguous-two" aoid="Ambiguous"><h1>Second</h1></emu-clause>
        "#;
        let spec = extract_step_structure(html, "ECMA-262", "https://tc39.es/ecma262/", "sha");
        let source = algorithm(&spec, "source");
        let operations = &source.operation_sites;
        assert_eq!(operations.len(), 3);
        assert_eq!(
            operations[0]
                .target
                .as_ref()
                .map(|target| target.anchor.as_str()),
            Some("called")
        );
        assert!(operations[1].target.is_none());
        assert!(operations[2].target.is_none());
        assert_eq!(source.segments[0].links[0].target, operations[0].target);
    }

    #[test]
    fn source_form_ecmarkup_keeps_relative_numbering_and_inline_markup() {
        let spec = fixture("ecmarkup");
        let algorithm = algorithm(&spec, "sec-source-form");
        assert_eq!(
            algorithm
                .steps
                .iter()
                .map(|step| step.path.clone())
                .collect::<Vec<_>>(),
            vec![vec![1], vec![1, 1], vec![2]]
        );
        let call = algorithm
            .operation_sites
            .iter()
            .find(|site| {
                site.target.as_ref().map(|target| target.anchor.as_str()) == Some("target")
            })
            .unwrap();
        let segment = algorithm
            .segments
            .iter()
            .find(|segment| segment.source.node_id == call.segment_id)
            .unwrap();
        assert!(segment
            .tokens
            .iter()
            .any(|token| token.kind == InlineTokenKind::Variable));
    }

    #[test]
    fn inline_named_definition_owns_its_operation_and_binds_later_sites() {
        let spec = fixture("wattsi");
        let algorithm = algorithm(&spec, "inline-definition");
        let definition = &algorithm.body_definitions[0];
        let named_body = algorithm.body(&definition.body_id);
        assert_eq!(named_body.kind, BodyKind::Named);
        let update_site = algorithm
            .operation_sites
            .iter()
            .find(|site| {
                site.target.as_ref().map(|target| target.anchor.as_str()) == Some("update-document")
            })
            .unwrap();
        assert_eq!(update_site.body_id, named_body.source.node_id);
        assert_eq!(
            algorithm.body_invocations[0].candidate_body_ids,
            vec![named_body.source.node_id.clone()]
        );
        assert!(algorithm.body_arguments.iter().any(|argument| {
            argument.body_id == named_body.source.node_id
                && argument.binding == BodyArgumentBinding::Named
        }));
    }

    #[test]
    fn remainder_range_uses_the_current_named_body_scope() {
        let spec = fixture("wattsi");
        let algorithm = algorithm(&spec, "named-remainder");
        let named = algorithm
            .bodies
            .iter()
            .find(|body| body.kind == BodyKind::Named)
            .unwrap();
        let remainder = algorithm
            .bodies
            .iter()
            .find(|body| body.kind == BodyKind::Remainder)
            .unwrap();
        assert_eq!(
            remainder.parent_body_id.as_deref(),
            Some(named.source.node_id.as_str())
        );
        assert_eq!(
            algorithm.continuations[0].enclosing_body_id,
            named.source.node_id
        );
        let ranged_step = algorithm
            .steps
            .iter()
            .find(|step| step.source.node_id == remainder.range.as_ref().unwrap().start_step_id)
            .unwrap();
        assert_eq!(ranged_step.path, vec![1, 2]);
        assert_eq!(ranged_step.body_id, remainder.source.node_id);
    }

    #[test]
    fn ids_are_deterministic_and_independent_of_the_snapshot() {
        let html = include_str!("../../tests/fixtures/effects/structure/wattsi.html");
        let first = extract_step_structure(html, "TEST", BASE, "hash:x");
        let second = extract_step_structure(html, "TEST", BASE, "hash:x");
        assert_eq!(first, second);
        let json = serde_json::to_string(&first).unwrap();
        assert_eq!(
            serde_json::from_str::<StructuralSpec>(&json).unwrap(),
            first
        );
        assert_eq!(first.version, STRUCTURE_VERSION);
        let changed = extract_step_structure(html, "TEST", BASE, "different-snapshot");
        for (a, b) in first.algorithms.iter().zip(&changed.algorithms) {
            assert_eq!(a.source.node_id, b.source.node_id);
        }
        assert_eq!(
            changed.algorithms[0].source.snapshot_sha,
            "different-snapshot"
        );
    }

    fn all_ids(spec: &StructuralSpec, anchor: &str) -> Vec<String> {
        let json = serde_json::to_string(algorithm(spec, anchor)).unwrap();
        let re = regex::Regex::new(r"src-[0-9a-f]{64}").unwrap();
        let mut ids: Vec<String> = re
            .find_iter(&json)
            .map(|m| m.as_str().to_string())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    const STABLE: &str = r##"<h2 id="s">S</h2>
<div class="algorithm"><p>To <dfn id="a">run a</dfn>:</p><ol><li>Let <var>x</var> be the result of <a href="#b">run b</a>.</li><li>Return <var>x</var>.</li></ol></div>
<p>To <dfn id="b">run b</dfn>:</p><ol><li>Return 1.</li></ol>"##;

    #[test]
    fn ids_survive_unrelated_edits_and_new_snapshots() {
        let edited = STABLE
            .replace(
                r#"<h2 id="s">S</h2>"#,
                r#"<h2 id="s">S</h2><p>A new paragraph.</p><div><p>More.</p></div>"#,
            )
            .replace("<li>Return 1.</li>", "<li>Return 1.</li><li>Return 2.</li>");
        let before = extract_step_structure(STABLE, "TEST", BASE, "hash:one");
        let after = extract_step_structure(&edited, "TEST", BASE, "hash:two");
        assert_eq!(all_ids(&before, "a"), all_ids(&after, "a"));
        assert_eq!(algorithm(&after, "a").source.snapshot_sha, "hash:two");
    }

    #[test]
    fn a_structural_edit_changes_only_ids_under_that_step() {
        let edited = STABLE.replace(
            "<li>Return <var>x</var>.</li>",
            "<li>Return <var>x</var>.<ol><li>Assert: true.</li></ol></li>",
        );
        let before = all_ids(&extract_step_structure(STABLE, "TEST", BASE, "hash:t"), "a");
        let after = all_ids(
            &extract_step_structure(&edited, "TEST", BASE, "hash:t"),
            "a",
        );
        assert!(
            before.iter().all(|id| after.contains(id)),
            "existing ids are kept"
        );
        assert!(
            after.len() > before.len(),
            "the new child step gets new ids"
        );
    }

    #[test]
    fn wattsi_sibling_bodies_use_owner_relative_paths() {
        let prefixed = format!("<p>filler</p>{STABLE}");
        let a = extract_step_structure(STABLE, "TEST", BASE, "hash:t");
        let b = extract_step_structure(&prefixed, "TEST", BASE, "hash:t");
        assert_eq!(all_ids(&a, "b"), all_ids(&b, "b"));
    }

    #[test]
    fn recognized_unsupported_algorithm_body_reports_an_issue() {
        let html = r#"
            <p>To <dfn id="switch-only">choose a branch</dfn>:</p>
            <dl class="switch"><dt>Otherwise</dt><dd>Return.</dd></dl>
        "#;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        assert!(spec.algorithms.is_empty());
        assert_eq!(spec.issues.len(), 1);
        assert_eq!(spec.issues[0].code, "unsupported_structure");
        assert!(spec.issues[0].message.contains("switch-only"));
    }

    #[test]
    fn branch_prose_is_preserved_as_executable_segments() {
        let html = r##"
            <div class="algorithm">
              <p>To <dfn id="choose">choose</dfn>:</p>
              <ol><li>Choose a case:
                <ul>
                  <li>If ready, <a href="#fire">fire an event</a>.</li>
                </ul>
                <dl class="switch">
                  <dt>Otherwise</dt>
                  <dd><p><a href="#queue">Queue a task</a>.</p></dd>
                </dl>
              </li></ol>
            </div>
        "##;
        let spec = extract_step_structure(html, "TEST", BASE, SHA);
        let algorithm = algorithm(&spec, "choose");

        assert!(algorithm.issues.is_empty());
        assert_eq!(algorithm.branches.len(), 2);
        for target in ["fire", "queue"] {
            let operation = algorithm
                .operation_sites
                .iter()
                .find(|operation| {
                    operation
                        .target
                        .as_ref()
                        .map(|target| target.anchor.as_str())
                        == Some(target)
                })
                .unwrap_or_else(|| panic!("missing operation for {target}"));
            assert!(algorithm.branches.iter().any(|branch| {
                branch.items.iter().any(
                    |item| matches!(item, StepItem::Segment(id) if id == &operation.segment_id),
                )
            }));
        }
    }

    trait AlgorithmBodyLookup {
        fn body(&self, id: &str) -> &StructuralBody;
    }

    impl AlgorithmBodyLookup for StructuralAlgorithm {
        fn body(&self, id: &str) -> &StructuralBody {
            self.bodies
                .iter()
                .find(|body| body.source.node_id == id)
                .unwrap()
        }
    }

    #[test]
    fn document_entry_point_matches_string_entry_point() {
        let html = "<div class=algorithm><p>To <dfn id=a>a</dfn>:</p><ol><li>Set <var>x</var> to <a href='#b'>b</a>.</li></ol></div><p><dfn id=b>b</dfn></p>";
        let from_str = extract_step_structure(html, "T", "https://t.example/", "hash:x");
        let document = Html::parse_document(html);
        let from_doc =
            extract_step_structure_from_document(&document, "T", "https://t.example/", "hash:x");
        assert_eq!(from_str, from_doc);
    }

    #[test]
    fn canonical_inline_uses_segment_encoding_and_skips_nested_lists() {
        let html = "<div><p>The <dfn id=m>stopPropagation()</dfn> method steps are to set <a href='#this'>this</a>'s <a href='#spf'>stop propagation flag</a> to <code>true</code>.<ul><li>nested</li></ul><span class=note>n</span></p></div>";
        let document = Html::parse_document(html);
        let p = document
            .select(&Selector::parse("p").unwrap())
            .next()
            .unwrap();
        let ctx = InlineContext {
            spec: "DOM",
            base_url: "https://dom.spec.whatwg.org/",
            snapshot_sha: "hash:x",
            anchor: "m",
        };
        let (text, tokens, links) = canonical_inline(&p, &ctx);
        assert_eq!(
            text,
            "The stopPropagation() method steps are to set this's stop propagation flag to `true`."
        );
        assert_eq!(links.len(), 2);
        assert_eq!(
            &text[links[1].span.start..links[1].span.end],
            "stop propagation flag"
        );
        assert_eq!(links[1].target.as_ref().unwrap().anchor, "spf");
        assert!(tokens
            .iter()
            .any(|t| t.kind == InlineTokenKind::Literal
                && &text[t.span.start..t.span.end] == "`true`"));
        assert!(!text.contains("nested"));
    }

    #[test]
    fn marked_rendering_reports_the_dfn_span_and_var_nodes_without_changing_text() {
        let html = r##"<p>To <dfn id="insert">insert</dfn> a <var>node</var>, with <dfn data-dfn-for="insert" id="s"><var>suppress</var></dfn>:</p>"##;
        let document = Html::parse_document(html);
        let p = document
            .select(&Selector::parse("p").unwrap())
            .next()
            .unwrap();
        let dfn = document
            .select(&Selector::parse("#insert").unwrap())
            .next()
            .unwrap();
        let ctx = InlineContext {
            spec: "DOM",
            base_url: "https://dom.spec.whatwg.org/",
            snapshot_sha: "hash:x",
            anchor: "insert",
        };
        let (text, tokens, links, marks) = canonical_inline_marked(&p, &ctx, Some(dfn.id()));
        assert_eq!(
            (text.clone(), tokens.clone(), links.clone()),
            canonical_inline(&p, &ctx)
        );
        let mark = marks.mark.unwrap();
        assert_eq!(&text[mark.start..mark.end], "insert");
        assert_eq!(marks.vars.len(), 2);
        let second = ElementRef::wrap(document.tree.get(marks.vars[1].1).unwrap()).unwrap();
        assert_eq!(
            second
                .parent()
                .and_then(ElementRef::wrap)
                .unwrap()
                .value()
                .attr("id"),
            Some("s")
        );
    }

    #[test]
    fn structural_body_nodes_contains_only_algorithm_bodies() {
        let html = "<p>To <dfn id=a>a</dfn>:</p><ol id=body><li>Return.</li></ol><p>Each navigable has:</p><ul id=props><li><dfn id=p>p</dfn></li></ul>";
        let document = Html::parse_document(html);
        let nodes = structural_body_nodes(&document);
        let body = document
            .select(&Selector::parse("#body").unwrap())
            .next()
            .unwrap();
        let props = document
            .select(&Selector::parse("#props").unwrap())
            .next()
            .unwrap();
        assert!(nodes.contains(&body.id()));
        assert!(!nodes.contains(&props.id()));
    }

    fn algo(steps: &str) -> StructuralSpec {
        extract_step_structure(
            &format!(
                r#"<div class="algorithm"><p>To <dfn id="a">a</dfn>:</p><ol>{steps}</ol></div>"#
            ),
            "HTML",
            "https://html.spec.whatwg.org/",
            "hash:x",
        )
    }

    #[test]
    fn links_inside_code_and_var_are_kept() {
        let s = algo(
            r##"<li><p>Let <var>d</var> be a new <code><a href="#document">Document</a></code> and <var><a href="#x">x</a></var>, typed <code><a href="#promise">Promise</a>&lt;T&gt;</code>.</p></li>"##,
        );
        let seg = &s.algorithms[0].segments[0];
        let visible: Vec<_> = seg.links.iter().map(|l| l.visible_text.as_str()).collect();
        assert_eq!(visible, ["`Document`", "*x*", "Promise"]);
        let whole = &seg.links[0];
        assert!(seg
            .tokens
            .iter()
            .any(|t| t.kind == InlineTokenKind::Code && t.span == whole.span));
        let inner = &seg.links[2];
        assert_eq!(&seg.text[inner.span.start..inner.span.end], "Promise");
        assert!(seg.text[..inner.span.start].ends_with('`'));
    }

    #[test]
    fn dl_branch_labels_keep_canonical_text_and_links() {
        let s = algo(
            r##"<li><p>Let <var>document</var> be a new <code><a href="#document">Document</a></code>, with:</p>
      <dl class="props"><dt><a href="#is-initial-about:blank">is initial <code>about:blank</code></a></dt><dd>true</dd>
      <dt><a href="https://dom.spec.whatwg.org/#concept-document-type">type</a></dt><dd>"<code>html</code>"</dd></dl></li>"##,
        );
        assert_eq!(s.version, "10");
        let branch = &s.algorithms[0].branches[0];
        assert_eq!(branch.label, "is initial about:blank");
        assert_eq!(branch.label_text, "is initial `about:blank`");
        assert_eq!(
            &branch.label_text[branch.label_links[0].span.start..branch.label_links[0].span.end],
            "is initial `about:blank`"
        );
        assert_eq!(
            s.algorithms[0].branches[1].label_links[0]
                .target
                .as_ref()
                .unwrap()
                .spec,
            "DOM"
        );
    }

    #[test]
    fn v7_branch_payloads_still_deserialize() {
        let branch: StructuralBranch = serde_json::from_str(r#"{"source":{"spec":"T","snapshot_sha":"s","section_anchor":"a","node_id":"n","url":"u"},"parent_step_id":"p","label":"x","items":[]}"#).unwrap();
        assert!(branch.label_links.is_empty() && branch.label_text.is_empty());
    }

    #[test]
    fn every_algorithm_in_one_container_is_extracted() {
        let html = r#"<div data-algorithm=""><p>Each <code>Document</code> has an <dfn id="ancestor-origins-list">ancestor origins list</dfn>.</p>
      <p>The <dfn id="ancestor-origins-list-creation-steps">ancestor origins list creation steps</dfn> are:</p><ol><li><p>Return.</p></li></ol>
      <p>To <dfn id="second">do the second thing</dfn>:</p><ol><li><p>Return.</p></li></ol></div>"#;
        let s = extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:x");
        let anchors: Vec<_> = s
            .algorithms
            .iter()
            .map(|a| a.source.section_anchor.as_str())
            .collect();
        assert_eq!(anchors, ["ancestor-origins-list-creation-steps", "second"]);
    }

    #[test]
    fn setter_steps_div_is_an_extra_body_of_the_attribute() {
        let html = r##"<div data-algorithm=""><p>The <dfn id="x" data-dfn-type="attribute"><code>x</code></dfn> getter steps are:</p><ol><li><p>Return 1.</p></li></ol></div>
      <div data-algorithm=""><p>The <code><a href="#x">x</a></code> setter steps are:</p><ol id="setter"><li><p>Throw.</p></li><li><p>Return.</p></li></ol></div>"##;
        let s = extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:x");
        assert_eq!(s.algorithms.len(), 1, "{:?}", s.algorithms);
        let algorithm = &s.algorithms[0];
        assert_eq!(algorithm.source.section_anchor, "x");
        let entries: Vec<_> = algorithm
            .bodies
            .iter()
            .filter(|body| body.kind == BodyKind::Algorithm)
            .collect();
        assert_eq!(entries.len(), 2);
        let setter = entries[1];
        assert_eq!(
            setter.parent_body_id.as_deref(),
            Some(algorithm.root_body_id.as_str())
        );
        assert_eq!(setter.items.len(), 2);
        let root = &entries[0];
        assert_eq!(root.source.node_id, algorithm.root_body_id);
        assert_eq!(
            root.items.last(),
            Some(&BodyItem::Body(setter.source.node_id.clone()))
        );
        let setter_steps: Vec<_> = algorithm
            .steps
            .iter()
            .filter(|step| step.body_id == setter.source.node_id)
            .map(|step| step.path.clone())
            .collect();
        assert_eq!(setter_steps, [vec![1], vec![2]]);

        let document = Html::parse_document(html);
        let list = document
            .select(&Selector::parse("#setter").unwrap())
            .next()
            .unwrap();
        assert!(structural_body_nodes(&document).contains(&list.id()));
    }

    #[test]
    fn setter_steps_div_without_getter_steps_becomes_the_attribute_algorithm() {
        let html = r##"<p>The <dfn id="x" data-dfn-type="attribute"><code>x</code></dfn> getter steps are to return 1.</p>
      <div data-algorithm=""><p>The <code><a href="#x">x</a></code> setter steps are:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let s = extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:x");
        let anchors: Vec<_> = s
            .algorithms
            .iter()
            .map(|a| a.source.section_anchor.as_str())
            .collect();
        assert_eq!(anchors, ["x"]);
        assert_eq!(s.algorithms[0].steps.len(), 1);
        assert_eq!(s.algorithms[0].bodies.len(), 1);
    }

    #[test]
    fn definitions_between_intro_and_steps_do_not_split_the_algorithm() {
        let html = r#"<div data-algorithm=""><p>The <dfn id="fragment-parsing">fragment parsing algorithm</dfn>, given <var>input</var>, has the following steps:</p>
      <p>Parts marked <dfn id="fragment-case">fragment case</dfn> only occur when parsing fragments.</p><ol><li><p>Return.</p></li></ol>
      <ul><li><p>A <dfn id="component">component</dfn> is:</p><ol><li>a digit</li></ol></li></ul></div>"#;
        let s = extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:x");
        let anchors: Vec<_> = s
            .algorithms
            .iter()
            .map(|a| a.source.section_anchor.as_str())
            .collect();
        assert_eq!(anchors, ["fragment-parsing"]);
    }

    #[test]
    fn single_algorithm_containers_keep_their_node_ids() {
        let html = r#"<div class="algorithm"><p>To <dfn id="a">a</dfn>:</p><ol><li><p>Return.</p></li></ol></div>"#;
        let document = Html::parse_document(html);
        let container = document
            .select(&Selector::parse("div").unwrap())
            .next()
            .unwrap();
        let ctx = ExtractContext {
            spec: "HTML",
            base_url: "https://html.spec.whatwg.org/",
            snapshot_sha: "hash:x",
            anchor: "a",
            owner: container,
        };
        let expected = source_identity(&ctx, "algorithm", ".", None).node_id;
        let s = extract_step_structure_from_document(
            &document,
            "HTML",
            "https://html.spec.whatwg.org/",
            "hash:x",
        );
        assert_eq!(s.algorithms[0].source.node_id, expected);
    }

    #[test]
    fn streams_extraction_is_deterministic_within_one_process() {
        let html =
            include_str!("../../tests/fixtures/fuzz/new-29-ir-nondeterministic-body-ids.html");
        let extract = || {
            serde_json::to_string(&extract_step_structure(
                html,
                "STREAMS",
                "https://streams.spec.whatwg.org",
                "hash:t",
            ))
            .unwrap()
        };
        let first = extract();
        for _ in 0..8 {
            assert_eq!(
                first,
                extract(),
                "every HashMap gets its own seed, so repeats expose #29"
            );
        }
    }
}
