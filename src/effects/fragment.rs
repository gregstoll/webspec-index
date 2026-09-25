//! Per-spec effects fragments: everything the execution graph needs from one spec.
//!
//! A fragment reads only its own spec's structure and anchor list, the catalog and the
//! environment. Links into other specs stay symbolic as [`PendingSite`]s until
//! [`crate::effects::link::link`] resolves them.

use crate::effects::catalog::{Anchor, Catalog};
use crate::effects::engine::{IndexedAnchor, LocalOccurrence, SourceSpec};
use crate::effects::graph::{site_key, ExecutionEdge, ExecutionNode};
use crate::effects::matcher::{
    continuation_modes, match_operation_with_issues, match_segment, MatchedEffect,
};
use crate::effects::model::*;
use crate::parse::steps::{
    AnchorTarget, BodyItem, BodyKind, ContinuationSyntax, OperationSite, ReferenceRole,
    SourceIdentity, StepItem, StructuralAlgorithm, StructuralSegment, StructuralSpec,
    StructuralStep,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const FRAGMENT_FORMAT_VERSION: u32 = 1;

pub struct FragmentInput<'a> {
    pub spec: &'a str,
    pub snapshot_sha: &'a str,
    pub base_url: &'a str,
    pub structure: Option<&'a StructuralSpec>,
    /// Sorted anchor list; `text` is set only for catalog-reviewed anchors.
    pub anchors: &'a [IndexedAnchor],
    pub catalog: &'a Catalog,
    pub environment: &'a str,
}

impl<'a> FragmentInput<'a> {
    pub fn for_source(source: &'a SourceSpec, catalog: &'a Catalog, environment: &'a str) -> Self {
        Self {
            spec: &source.spec,
            snapshot_sha: &source.snapshot_sha,
            base_url: &source.base_url,
            structure: source.structure.as_ref(),
            anchors: &source.anchors,
            catalog,
            environment,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub format: u32,
    pub spec: String,
    pub snapshot_sha: String,
    pub base_url: String,
    pub anchors: Vec<IndexedAnchor>,
    /// Section anchor -> root body id.
    pub algorithm_roots: BTreeMap<String, String>,
    /// Catalog public id -> whether its `expect_text` matches this spec's subject text.
    pub expectations: BTreeMap<String, bool>,
    /// `source_order` is the local order.
    pub nodes: Vec<ExecutionNode>,
    /// Intra-spec edges only.
    pub edges: Vec<ExecutionEdge>,
    pub pending: Vec<PendingSite>,
    /// Own anchors targeted by operations but absent from the anchor list.
    pub local_missing: BTreeSet<String>,
    pub occurrences: Vec<LocalOccurrence>,
    /// Fragment-local issue catalog.
    pub issues: Vec<GraphIssue>,
    /// Node id -> (local issue index, local order when attached), in attach order.
    pub node_issues: Vec<(String, Vec<(u32, u64)>)>,
    pub definitions: BTreeMap<String, Vec<String>>,
    pub sites: BTreeMap<String, SourceSite>,
}

/// An operation site whose target lies in another spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSite {
    pub owner: String,
    pub target: AnchorTarget,
    pub relation: Relationship,
    pub execution: Execution,
    pub mention: bool,
    pub site_key: String,
    pub site_id: String,
    pub context: Vec<ContextRef>,
    pub context_truncated: bool,
    pub boundary: Option<Boundary>,
    pub order: u64,
}

pub fn build_fragment(input: &FragmentInput<'_>) -> Fragment {
    let mut builder = FragmentBuilder::new(input);
    builder.index();
    builder.add_relationships();
    builder.add_expectations();
    builder.finish()
}

pub fn encode_fragment(fragment: &Fragment) -> Vec<u8> {
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;

    let json = serde_json::to_vec(fragment).expect("serializable fragment");
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(1));
    encoder.write_all(&json).expect("deflate write");
    encoder.finish().expect("deflate finish")
}

pub fn decode_fragment(bytes: &[u8]) -> anyhow::Result<Fragment> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    let mut json = Vec::new();
    DeflateDecoder::new(bytes).read_to_end(&mut json)?;
    let fragment: Fragment = serde_json::from_slice(&json)?;
    anyhow::ensure!(
        fragment.format == FRAGMENT_FORMAT_VERSION,
        "fragment format {} is not {FRAGMENT_FORMAT_VERSION}",
        fragment.format
    );
    Ok(fragment)
}

/// Where an operation target resolves from the point of view of one fragment.
enum TargetResolution {
    Local(String),
    LocalMissing,
    Foreign,
}

struct FragmentBuilder<'a> {
    input: &'a FragmentInput<'a>,
    /// First position of each anchor in the anchor list.
    anchor_index: HashMap<&'a str, usize>,
    idl_type_anchors: HashSet<&'a str>,
    algorithm_roots: BTreeMap<String, String>,
    nodes: BTreeMap<String, ExecutionNode>,
    edges: BTreeMap<String, ExecutionEdge>,
    edge_triples: HashSet<(String, String, Execution)>,
    occurrences: BTreeMap<String, LocalOccurrence>,
    occurrences_by_site: HashMap<String, Vec<String>>,
    pending: Vec<PendingSite>,
    local_missing: BTreeSet<String>,
    issues: Vec<GraphIssue>,
    issue_ids: HashMap<GraphIssue, u32>,
    node_issues: BTreeMap<String, Vec<(u32, u64)>>,
    expectations: BTreeMap<String, bool>,
    definitions: BTreeMap<String, Vec<String>>,
    sites: BTreeMap<String, SourceSite>,
    order: u64,
}

impl<'a> FragmentBuilder<'a> {
    fn new(input: &'a FragmentInput<'a>) -> Self {
        let mut anchor_index = HashMap::new();
        let mut idl_type_anchors = HashSet::new();
        for (index, anchor) in input.anchors.iter().enumerate() {
            anchor_index.entry(anchor.anchor.as_str()).or_insert(index);
            if anchor.idl_kind.as_deref().is_some_and(is_idl_type_kind) {
                idl_type_anchors.insert(anchor.anchor.as_str());
            }
        }
        Self {
            input,
            anchor_index,
            idl_type_anchors,
            algorithm_roots: BTreeMap::new(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            edge_triples: HashSet::new(),
            occurrences: BTreeMap::new(),
            occurrences_by_site: HashMap::new(),
            pending: Vec::new(),
            local_missing: BTreeSet::new(),
            issues: Vec::new(),
            issue_ids: HashMap::new(),
            node_issues: BTreeMap::new(),
            expectations: BTreeMap::new(),
            definitions: BTreeMap::new(),
            sites: BTreeMap::new(),
            order: input.anchors.len() as u64,
        }
    }

    fn algorithms(&self) -> &'a [StructuralAlgorithm] {
        self.input
            .structure
            .map_or(&[], |structure| structure.algorithms.as_slice())
    }

    fn index(&mut self) {
        let Some(structure) = self.input.structure else {
            return;
        };
        for algorithm in &structure.algorithms {
            self.algorithm_roots.insert(
                algorithm.source.section_anchor.clone(),
                algorithm.root_body_id.clone(),
            );
        }
        for algorithm in &structure.algorithms {
            self.index_algorithm(algorithm);
        }
        for issue in &structure.issues {
            let targets: Vec<_> = match &issue.site_id {
                Some(site) => structure
                    .algorithms
                    .iter()
                    .find_map(|algorithm| owner_for_site(algorithm, site))
                    .into_iter()
                    .collect(),
                None => structure
                    .algorithms
                    .iter()
                    .map(|algorithm| algorithm.root_body_id.clone())
                    .collect(),
            };
            for target in targets {
                self.add_issue(
                    &target,
                    issue_code(&issue.code),
                    issue.message.clone(),
                    None,
                );
            }
        }
    }

    fn index_algorithm(&mut self, algorithm: &StructuralAlgorithm) {
        let root = algorithm.root_body_id.clone();
        for body in &algorithm.bodies {
            let definition_only =
                body.kind != BodyKind::Algorithm && body.kind != BodyKind::Remainder;
            self.add_node(
                body.source.node_id.clone(),
                subject_from_source(
                    &body.source,
                    None,
                    None,
                    (body.kind != BodyKind::Algorithm).then(|| body.source.node_id.clone()),
                ),
                true,
                definition_only,
            );
        }
        for step in &algorithm.steps {
            self.add_node(
                step.source.node_id.clone(),
                subject_from_source(
                    &step.source,
                    Some(step.source.node_id.clone()),
                    Some(step.path.clone()),
                    Some(step.body_id.clone()),
                ),
                false,
                false,
            );
        }
        for definition in &algorithm.body_definitions {
            self.definitions
                .entry(definition.step_id.clone())
                .or_default()
                .push(definition.body_id.clone());
        }
        for issue in &algorithm.issues {
            let code = issue_code(&issue.code);
            let subject_id = issue
                .site_id
                .as_ref()
                .and_then(|site| owner_for_site(algorithm, site))
                .unwrap_or_else(|| root.clone());
            self.add_issue(&subject_id, code, issue.message.clone(), None);
        }
        let segments: HashMap<_, _> = algorithm
            .segments
            .iter()
            .map(|segment| (segment.source.node_id.as_str(), segment))
            .collect();
        let steps_by_id = steps_by_id(algorithm);
        for segment in &algorithm.segments {
            let owner = segment_owner(algorithm, segment);
            for matched in match_segment(self.input.catalog, algorithm, segment) {
                self.add_segment_occurrence(&owner, algorithm, segment, matched);
            }
        }
        for operation in &algorithm.operation_sites {
            if operation.role == ReferenceRole::Mention {
                continue;
            }
            let Some(segment) = segments.get(operation.segment_id.as_str()) else {
                continue;
            };
            let owner = execution_owner(algorithm, &operation.step_id, &operation.body_id);
            let matched =
                match_operation_with_issues(self.input.catalog, algorithm, segment, operation);
            if matched.effects.is_empty() {
                for code in matched.issues {
                    self.add_issue(
                        &owner,
                        code,
                        "an anchor-text match covered multiple matching operation references",
                        site_for_identity(
                            &operation.source,
                            Some(&operation.step_id),
                            Some(&operation.body_id),
                            Some(segment),
                            &steps_by_id,
                        ),
                    );
                }
            }
            for effect in matched.effects {
                self.add_matched_occurrence(&owner, algorithm, segment, operation, effect);
            }
        }
    }

    fn add_node(&mut self, id: String, subject: Subject, is_body: bool, definition_only: bool) {
        let order = self.order;
        self.order += 1;
        self.nodes.insert(
            id.clone(),
            ExecutionNode {
                id,
                subject,
                source_order: order,
                is_body,
                definition_only,
            },
        );
    }

    fn add_matched_occurrence(
        &mut self,
        owner: &str,
        algorithm: &StructuralAlgorithm,
        segment: &StructuralSegment,
        operation: &OperationSite,
        matched: MatchedEffect,
    ) {
        let occurrence_id = format!(
            "occ_{}",
            &digest_serializable(&json!({
                "site": operation.source.node_id,
                "kind": matched.kind,
                "params": matched.params,
            }))
            .expect("JSON digest")[..20]
        );
        let site = source_site(
            algorithm,
            &operation.source,
            Some(&operation.step_id),
            Some(&operation.body_id),
            Some(&operation.segment_id),
            matched.span.or(Some(Span {
                start: operation.span.start as u64,
                end: operation.span.end as u64,
            })),
            Some(segment.text.clone()),
        );
        let evidence = evidence_for(&occurrence_id, &matched, &site, None);
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id.clone(),
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: matched.issues.iter().copied().collect(),
                source_order: self.order,
            },
        );
        self.occurrences_by_site
            .entry(site.id.clone())
            .or_default()
            .push(occurrence_id);
        for code in matched.issues {
            self.add_issue(
                owner,
                code,
                "effect argument could not be resolved from typed source tokens",
                Some(site.clone()),
            );
        }
        self.order += 1;
    }

    fn add_segment_occurrence(
        &mut self,
        owner: &str,
        algorithm: &StructuralAlgorithm,
        segment: &StructuralSegment,
        matched: MatchedEffect,
    ) {
        let span = matched
            .span
            .expect("anchorless text rules always have a regex occurrence span");
        let site_id = format!(
            "match_{}",
            &digest_serializable(&json!({
                "segment": segment.source.node_id,
                "span": span,
                "kind": matched.kind,
                "params": matched.params,
            }))
            .expect("JSON digest")[..20]
        );
        let occurrence_id = format!(
            "occ_{}",
            &digest_serializable(
                &json!({"site": site_id, "kind": matched.kind, "params": matched.params})
            )
            .expect("JSON digest")[..20]
        );
        let mut source = segment.source.clone();
        source.node_id = site_id;
        let site = source_site(
            algorithm,
            &source,
            segment.owner_step_id.as_deref(),
            Some(&segment.owner_body_id),
            Some(&segment.source.node_id),
            Some(span),
            Some(segment.text.clone()),
        );
        let evidence = evidence_for(&occurrence_id, &matched, &site, None);
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id.clone(),
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: matched.issues.iter().copied().collect(),
                source_order: self.order,
            },
        );
        self.occurrences_by_site
            .entry(site.id.clone())
            .or_default()
            .push(occurrence_id);
        for code in matched.issues {
            self.add_issue(
                owner,
                code,
                match code {
                    IssueCode::AmbiguousMatch => {
                        "segment rules emitted conflicting known parameter values"
                    }
                    _ => "effect argument could not be resolved from typed source tokens",
                },
                Some(site.clone()),
            );
        }
        self.order += 1;
    }

    fn add_relationships(&mut self) {
        let algorithms = self.algorithms();
        for algorithm in algorithms {
            self.add_algorithm_relationships(algorithm);
        }
        let mut targets_by_site_anchor: BTreeMap<(String, String, String), Vec<ContextTarget>> =
            BTreeMap::new();
        let site_anchor = |site: &SourceSite| {
            (
                site.subject.spec.clone(),
                site.subject.anchor.clone(),
                site.subject.snapshot_sha.clone(),
            )
        };
        for (id, edge) in &self.edges {
            if let Some(site) = self.sites.get(&edge.site_key) {
                targets_by_site_anchor
                    .entry(site_anchor(site))
                    .or_default()
                    .push(ContextTarget::Edge(id.clone()));
            }
        }
        for (index, pending) in self.pending.iter().enumerate() {
            if let Some(site) = self.sites.get(&pending.site_key) {
                targets_by_site_anchor
                    .entry(site_anchor(site))
                    .or_default()
                    .push(ContextTarget::Pending(index));
            }
        }
        for algorithm in algorithms {
            self.attach_algorithm_context(algorithm, &targets_by_site_anchor);
        }
    }

    /// Resolves an operation target: an own algorithm root or anchor, an own anchor
    /// absent from the anchor list, or a symbol in another spec.
    fn resolve(&mut self, target: &AnchorTarget) -> TargetResolution {
        if target.spec != self.input.spec {
            return TargetResolution::Foreign;
        }
        if let Some(root) = self.algorithm_roots.get(&target.anchor) {
            return TargetResolution::Local(root.clone());
        }
        let Some(&index) = self.anchor_index.get(target.anchor.as_str()) else {
            return TargetResolution::LocalMissing;
        };
        let id = format!("anchor:{}#{}", self.input.spec, target.anchor);
        self.nodes
            .entry(id.clone())
            .or_insert_with(|| ExecutionNode {
                id: id.clone(),
                subject: Subject {
                    spec: self.input.spec.to_owned(),
                    anchor: target.anchor.clone(),
                    snapshot_sha: self.input.snapshot_sha.to_owned(),
                    step_id: None,
                    step_path: None,
                    body_id: None,
                },
                source_order: index as u64,
                is_body: true,
                definition_only: false,
            });
        TargetResolution::Local(id)
    }

    fn add_pending(
        &mut self,
        owner: &str,
        target: &AnchorTarget,
        (relation, execution): (Relationship, Execution),
        mention: bool,
        site: SourceSite,
    ) {
        let site_id = site.id.clone();
        let site_key = self.intern_site(site);
        self.pending.push(PendingSite {
            owner: owner.to_owned(),
            target: target.clone(),
            relation,
            execution,
            mention,
            site_key,
            site_id,
            context: Vec::new(),
            context_truncated: false,
            boundary: None,
            order: self.order,
        });
        self.order += 1;
    }

    fn add_algorithm_relationships(&mut self, algorithm: &StructuralAlgorithm) {
        let segments: HashMap<_, _> = algorithm
            .segments
            .iter()
            .map(|s| (s.source.node_id.as_str(), s))
            .collect();
        let steps_by_id = steps_by_id(algorithm);
        for body in &algorithm.bodies {
            for item in &body.items {
                if let BodyItem::Step(step_id) = item {
                    if self.nodes.contains_key(step_id) {
                        let site = site_for_step(step_id, &steps_by_id);
                        self.add_edge(
                            &body.source.node_id,
                            step_id,
                            Relationship::Invoke,
                            Execution::Inline,
                            site,
                            None,
                        );
                    }
                }
            }
        }
        for step in &algorithm.steps {
            for item in &step.items {
                if let StepItem::ChildStep(child) = item {
                    let site = site_for_step(child, &steps_by_id);
                    self.add_edge(
                        &step.source.node_id,
                        child,
                        Relationship::Invoke,
                        Execution::Inline,
                        site,
                        None,
                    );
                }
            }
        }
        for invocation in &algorithm.body_invocations {
            let owner = execution_owner(algorithm, &invocation.step_id, &invocation.scope_body_id);
            if invocation.candidate_body_ids.is_empty() {
                self.add_issue(
                    &owner,
                    IssueCode::UnresolvedBodyBinding,
                    format!("no lexical body named {:?} is visible", invocation.name),
                    site_for_identity(
                        &invocation.source,
                        Some(&invocation.step_id),
                        Some(&invocation.scope_body_id),
                        None,
                        &steps_by_id,
                    ),
                );
            }
            let relation = if invocation.candidate_body_ids.len() == 1 {
                Relationship::Invoke
            } else {
                Relationship::CandidateInvoke
            };
            let execution = if relation == Relationship::Invoke {
                Execution::Inline
            } else {
                Execution::Unknown
            };
            for body in &invocation.candidate_body_ids {
                let site = site_for_identity(
                    &invocation.source,
                    Some(&invocation.step_id),
                    Some(&invocation.scope_body_id),
                    None,
                    &steps_by_id,
                );
                self.add_edge(&owner, body, relation, execution, site, None);
            }
        }
        for operation in &algorithm.operation_sites {
            if operation.role != ReferenceRole::Mention {
                if let Some(target) = &operation.target {
                    if target.spec == self.input.spec
                        && !self.anchor_index.contains_key(target.anchor.as_str())
                    {
                        self.local_missing.insert(target.anchor.clone());
                    }
                }
            }
            let owner = execution_owner(algorithm, &operation.step_id, &operation.body_id);
            let Some(segment) = segments.get(operation.segment_id.as_str()) else {
                continue;
            };
            let site = site_for_identity(
                &operation.source,
                Some(&operation.step_id),
                Some(&operation.body_id),
                Some(segment),
                &steps_by_id,
            )
            .expect("site_for_identity always yields a site");
            if operation.role == ReferenceRole::Mention {
                if let Some(target) = &operation.target {
                    match self.resolve(target) {
                        TargetResolution::Local(to) => self.add_edge(
                            &owner,
                            &to,
                            Relationship::Mention,
                            Execution::Inline,
                            Some(site),
                            None,
                        ),
                        TargetResolution::LocalMissing => {}
                        TargetResolution::Foreign => self.add_pending(
                            &owner,
                            target,
                            (Relationship::Mention, Execution::Inline),
                            true,
                            site,
                        ),
                    }
                }
                continue;
            }
            if let Some(target) = &operation.target {
                let (relation, execution) = operation_relation(segment, operation);
                match self.resolve(target) {
                    TargetResolution::Local(to) => {
                        // A link without a verb-based invocation signal, whose target
                        // is a non-algorithm section or an IDL type definition, is a
                        // concept mention rather than an unresolved invocation.
                        // Code-wrapped Wattsi links such as `DOMException` carry no
                        // data-link-type, so only the anchor's idl_kind reveals them.
                        let demote = relation == Relationship::CandidateInvoke
                            && (to.starts_with("anchor:")
                                || self.idl_type_anchors.contains(target.anchor.as_str()));
                        if demote {
                            self.add_edge(
                                &owner,
                                &to,
                                Relationship::Mention,
                                Execution::Inline,
                                Some(site.clone()),
                                None,
                            );
                        } else {
                            self.add_edge(
                                &owner,
                                &to,
                                relation,
                                execution,
                                Some(site.clone()),
                                None,
                            );
                            if relation == Relationship::CandidateInvoke {
                                self.add_issue(
                                    &owner,
                                    IssueCode::UnresolvedInvocation,
                                    "algorithm-bearing reference has unresolved invocation syntax",
                                    Some(site.clone()),
                                );
                            }
                        }
                    }
                    TargetResolution::LocalMissing => self.add_issue(
                        &owner,
                        IssueCode::MissingAnchor,
                        missing_target_message(target),
                        Some(site.clone()),
                    ),
                    TargetResolution::Foreign => {
                        self.add_pending(&owner, target, (relation, execution), false, site.clone())
                    }
                }
            } else {
                self.add_issue(
                    &owner,
                    IssueCode::UnresolvedInvocation,
                    "operation link target could not be resolved",
                    Some(site.clone()),
                );
            }
            if !operation.actual_body_ids.is_empty() {
                let modes = continuation_modes(self.input.catalog, algorithm, segment, operation);
                if modes.is_empty() {
                    for body in &operation.actual_body_ids {
                        self.add_edge(
                            &owner,
                            body,
                            Relationship::CandidateInvoke,
                            Execution::Unknown,
                            Some(site.clone()),
                            None,
                        );
                    }
                    self.add_issue(
                        &owner,
                        IssueCode::UnresolvedBodyBinding,
                        "step-body argument has no continuation timing contract",
                        Some(site.clone()),
                    );
                } else {
                    if modes.len() > 1 {
                        self.add_issue(
                            &owner,
                            IssueCode::AmbiguousMatch,
                            "matching continuation declarations disagree on execution timing",
                            Some(site.clone()),
                        );
                    }
                    for mode in modes {
                        let relation = if mode == Execution::Separate {
                            Relationship::ScheduleBody
                        } else if mode == Execution::Inline {
                            Relationship::Invoke
                        } else {
                            Relationship::CandidateInvoke
                        };
                        let boundary = Some(Boundary {
                            execution: mode,
                            operation_site_id: operation.source.node_id.clone(),
                            effect: self.boundary_effect(operation.source.node_id.as_str()),
                        });
                        for body in &operation.actual_body_ids {
                            self.add_edge(
                                &owner,
                                body,
                                relation,
                                mode,
                                Some(site.clone()),
                                boundary.clone(),
                            );
                        }
                    }
                }
            }
        }
        for continuation in &algorithm.continuations {
            let Some(remainder) = &continuation.remainder_body_id else {
                continue;
            };
            let owner = execution_owner(
                algorithm,
                &continuation.step_id,
                &continuation.enclosing_body_id,
            );
            let mode = match continuation.syntax {
                ContinuationSyntax::Inline => Execution::Inline,
                ContinuationSyntax::Queued => Execution::Separate,
                ContinuationSyntax::Unknown => Execution::Unknown,
            };
            // Queued remainders already attached to their queue operation carry
            // the stronger operation-site boundary. Keep standalone inline and
            // unresolved transfers here, and add queued only if parsing could
            // not associate it with an operation.
            let already = self
                .edge_triples
                .contains(&(owner.clone(), remainder.clone(), mode));
            if !already {
                let site = site_for_identity(
                    &continuation.source,
                    Some(&continuation.step_id),
                    Some(&continuation.enclosing_body_id),
                    segments.get(continuation.segment_id.as_str()).copied(),
                    &steps_by_id,
                );
                self.add_edge(
                    &owner,
                    remainder,
                    Relationship::Resume,
                    mode,
                    site,
                    Some(Boundary {
                        execution: mode,
                        operation_site_id: continuation.source.node_id.clone(),
                        effect: None,
                    }),
                );
            }
        }
    }

    fn attach_algorithm_context(
        &mut self,
        algorithm: &StructuralAlgorithm,
        targets_by_site_anchor: &BTreeMap<(String, String, String), Vec<ContextTarget>>,
    ) {
        let key = (
            algorithm.source.spec.clone(),
            algorithm.source.section_anchor.clone(),
            algorithm.source.snapshot_sha.clone(),
        );
        let Some(targets) = targets_by_site_anchor.get(&key) else {
            return;
        };
        let updates: Vec<_> = targets
            .iter()
            .filter_map(|target| {
                let (site_key, binding_target) = match target {
                    ContextTarget::Edge(id) => {
                        let edge = self.edges.get(id)?;
                        let binds = matches!(
                            edge.relation,
                            Relationship::Invoke
                                | Relationship::CandidateInvoke
                                | Relationship::ScheduleBody
                                | Relationship::Resume
                        );
                        (&edge.site_key, binds.then_some(edge.to.as_str()))
                    }
                    // Bindings name bodies of this algorithm, never another spec's node.
                    ContextTarget::Pending(index) => (&self.pending[*index].site_key, None),
                };
                let site = self.sites.get(site_key)?;
                let (context, truncated) = edge_context(algorithm, site, binding_target);
                Some((target, context, truncated))
            })
            .collect();
        for (target, context_sites, context_truncated) in updates {
            let refs: Vec<ContextRef> = context_sites
                .into_iter()
                .map(|(kind, site)| ContextRef {
                    kind,
                    site_key: self.intern_site(site),
                })
                .collect();
            match target {
                ContextTarget::Edge(id) => {
                    if let Some(edge) = self.edges.get_mut(id) {
                        edge.context = refs;
                        edge.context_truncated = context_truncated;
                    }
                }
                ContextTarget::Pending(index) => {
                    let pending = &mut self.pending[*index];
                    pending.context = refs;
                    pending.context_truncated = context_truncated;
                }
            }
        }
    }

    fn add_expectations(&mut self) {
        let catalog = self.input.catalog;
        for summary in catalog.summaries() {
            if summary.subject.spec == self.input.spec {
                let matches = self.subject_matches(&summary.subject, |text| {
                    summary.expect_text.regex().is_match(text)
                });
                self.expectations.insert(summary.public_id.clone(), matches);
            }
        }
        for implementation in catalog.implementations() {
            if implementation.implementation.spec == self.input.spec {
                let matches = self.subject_matches(&implementation.implementation, |text| {
                    implementation.expect_text.regex().is_match(text)
                });
                self.expectations
                    .insert(implementation.public_id.clone(), matches);
            }
        }
    }

    /// Whether the anchor's reviewed text or its algorithm's segments satisfy `test`.
    fn subject_matches(&self, anchor: &Anchor, test: impl Fn(&str) -> bool) -> bool {
        let text = self
            .anchor_index
            .get(anchor.anchor.as_str())
            .map(|&index| self.input.anchors[index].text.as_str());
        let segments = self
            .algorithms()
            .iter()
            .find(|algorithm| algorithm.source.section_anchor == anchor.anchor)
            .into_iter()
            .flat_map(|algorithm| algorithm.segments.iter().map(|s| s.text.as_str()));
        text.into_iter().chain(segments).any(test)
    }

    fn intern_site(&mut self, site: SourceSite) -> String {
        let key = site_key(&site);
        self.sites.entry(key.clone()).or_insert(site);
        key
    }

    fn add_edge(
        &mut self,
        from: &str,
        to: &str,
        relation: Relationship,
        execution: Execution,
        site: Option<SourceSite>,
        boundary: Option<Boundary>,
    ) {
        let Some(site) = site else { return };
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return;
        }
        let site_id = site.id.clone();
        let id = edge_id(from, to, relation, &site_id, execution);
        let sk = self.intern_site(site);
        self.edges.entry(id.clone()).or_insert_with(|| {
            self.edge_triples
                .insert((from.to_owned(), to.to_owned(), execution));
            ExecutionEdge {
                id,
                from: from.to_owned(),
                to: to.to_owned(),
                relation,
                execution,
                site_key: sk,
                site_id,
                context: Vec::new(),
                context_truncated: false,
                boundary,
                source_order: self.order,
            }
        });
        self.order += 1;
    }

    fn add_issue(
        &mut self,
        subject: &str,
        code: IssueCode,
        message: impl Into<String>,
        site: Option<SourceSite>,
    ) {
        let site_key = site.map(|s| self.intern_site(s));
        let issue = GraphIssue {
            code,
            message: message.into(),
            site_key,
        };
        let index = match self.issue_ids.get(&issue) {
            Some(&index) => index,
            None => {
                let index = u32::try_from(self.issues.len()).expect("issue catalog exceeded u32");
                self.issues.push(issue.clone());
                self.issue_ids.insert(issue, index);
                index
            }
        };
        let entries = self.node_issues.entry(subject.to_owned()).or_default();
        if !entries.iter().any(|&(existing, _)| existing == index) {
            entries.push((index, self.order));
        }
    }

    fn boundary_effect(&self, operation_site_id: &str) -> Option<BoundaryEffect> {
        self.occurrences_by_site
            .get(operation_site_id)?
            .iter()
            .filter_map(|occ_id| self.occurrences.get(occ_id))
            .filter(|occ| {
                self.input
                    .catalog
                    .effects
                    .get(&occ.kind)
                    .is_some_and(|definition| definition.category == "async")
            })
            .min_by(|left, right| left.id.cmp(&right.id))
            .map(|occ| BoundaryEffect {
                kind: occ.kind.clone(),
                params: occ.params.clone(),
            })
    }

    fn finish(self) -> Fragment {
        Fragment {
            format: FRAGMENT_FORMAT_VERSION,
            spec: self.input.spec.to_owned(),
            snapshot_sha: self.input.snapshot_sha.to_owned(),
            base_url: self.input.base_url.to_owned(),
            anchors: self.input.anchors.to_vec(),
            algorithm_roots: self.algorithm_roots,
            expectations: self.expectations,
            nodes: self.nodes.into_values().collect(),
            edges: self.edges.into_values().collect(),
            pending: self.pending,
            local_missing: self.local_missing,
            occurrences: self.occurrences.into_values().collect(),
            issues: self.issues,
            node_issues: self.node_issues.into_iter().collect(),
            definitions: self.definitions,
            sites: self.sites,
        }
    }
}

enum ContextTarget {
    Edge(String),
    Pending(usize),
}

pub(crate) fn edge_id(
    from: &str,
    to: &str,
    relation: Relationship,
    site_id: &str,
    execution: Execution,
) -> String {
    format!(
        "rel_{}",
        &digest_serializable(&json!({"from": from, "to": to, "relation": relation, "site": site_id, "execution": execution}))
            .expect("JSON digest")[..20]
    )
}

pub(crate) fn missing_target_message(target: &AnchorTarget) -> String {
    format!(
        "target {}#{} is not present in the immutable corpus",
        target.spec, target.anchor
    )
}

pub(crate) fn evidence_for(
    occurrence_id: &str,
    matched: &MatchedEffect,
    site: &SourceSite,
    reason: Option<&str>,
) -> Vec<Evidence> {
    matched
        .rule_ids
        .iter()
        .map(|rule_id| Evidence {
            id: format!(
                "ev_{}",
                &digest_serializable(&json!({"occurrence": occurrence_id, "rule": rule_id}))
                    .expect("JSON digest")[..20]
            ),
            basis: matched.basis,
            rule_id: rule_id.clone(),
            site: site.clone(),
            captures: (!matched.captures.is_empty()).then(|| matched.captures.clone()),
            argument_expressions: (!matched.arguments.is_empty())
                .then(|| matched.arguments.clone()),
            reason: reason.map(str::to_owned),
        })
        .collect()
}

/// Enclosing steps, branches, local body bindings and preceding exits of an edge site.
fn edge_context(
    algorithm: &StructuralAlgorithm,
    site: &SourceSite,
    binding_target: Option<&str>,
) -> (Vec<(ContextKind, SourceSite)>, bool) {
    let mut context: Vec<(ContextKind, SourceSite)> = Vec::new();
    let mut context_truncated = false;
    let mut step_chain = Vec::new();
    if let Some(step_id) = &site.subject.step_id {
        let mut current = Some(step_id.as_str());
        while let Some(id) = current {
            step_chain.push(id.to_owned());
            current = algorithm
                .steps
                .iter()
                .find(|step| step.source.node_id == id)
                .and_then(|step| step.parent_step_id.as_deref());
        }
        step_chain.reverse();
        let ancestors: BTreeSet<_> = step_chain.iter().cloned().collect();

        for parent_id in step_chain.iter().take(step_chain.len().saturating_sub(1)) {
            if let Some(segment) = first_step_segment(algorithm, parent_id) {
                let (text, clipped) = truncate_context_excerpt(&segment.text);
                context_truncated |= clipped;
                context.push((
                    ContextKind::Enclosing,
                    source_site(
                        algorithm,
                        &segment.source,
                        Some(parent_id),
                        Some(&segment.owner_body_id),
                        Some(&segment.source.node_id),
                        None,
                        Some(text),
                    ),
                ));
            }
        }
        for branch in &algorithm.branches {
            let contains = branch.items.iter().any(|item| match item {
                StepItem::ChildStep(id) => ancestors.contains(id),
                StepItem::Segment(id) => site.segment_id.as_ref() == Some(id),
                _ => false,
            });
            if contains {
                let (text, clipped) = truncate_context_excerpt(&branch.label);
                context_truncated |= clipped;
                context.push((
                    ContextKind::Branch,
                    source_site(
                        algorithm,
                        &branch.source,
                        Some(&branch.parent_step_id),
                        algorithm
                            .steps
                            .iter()
                            .find(|step| step.source.node_id == branch.parent_step_id)
                            .map(|step| step.body_id.as_str()),
                        None,
                        None,
                        Some(text),
                    ),
                ));
            }
        }
    }
    if let Some(binding) = binding_target.and_then(|to| {
        algorithm
            .body_definitions
            .iter()
            .find(|definition| definition.body_id == to)
    }) {
        let binding_text = format!(
            "{} is the locally defined body used at this call site",
            binding.name
        );
        let (text, clipped) = truncate_context_excerpt(&binding_text);
        context_truncated |= clipped;
        context.push((
            ContextKind::Binding,
            source_site(
                algorithm,
                &binding.source,
                Some(&binding.step_id),
                Some(&binding.scope_body_id),
                None,
                None,
                Some(text),
            ),
        ));
    }
    for step_id in step_chain.iter().rev() {
        if context
            .iter()
            .filter(|(kind, _)| *kind == ContextKind::PrecedingExit)
            .count()
            == 2
        {
            break;
        }
        let Some(exit) = preceding_explicit_exit(algorithm, step_id) else {
            continue;
        };
        let (text, clipped) = truncate_context_excerpt(&exit.text);
        context_truncated |= clipped;
        context.push((
            ContextKind::PrecedingExit,
            source_site(
                algorithm,
                &exit.source,
                exit.owner_step_id.as_deref(),
                Some(&exit.owner_body_id),
                Some(&exit.source.node_id),
                None,
                Some(text),
            ),
        ));
    }
    context_truncated |= context.len() > 8;
    context.truncate(8);
    (context, context_truncated)
}

fn steps_by_id(algorithm: &StructuralAlgorithm) -> HashMap<&str, &StructuralStep> {
    algorithm
        .steps
        .iter()
        .map(|s| (s.source.node_id.as_str(), s))
        .collect()
}

fn site_for_step(
    step_id: &str,
    steps_by_id: &HashMap<&str, &StructuralStep>,
) -> Option<SourceSite> {
    let step = *steps_by_id.get(step_id)?;
    site_for_identity(
        &step.source,
        Some(step_id),
        Some(&step.body_id),
        None,
        steps_by_id,
    )
}

fn site_for_identity(
    source: &SourceIdentity,
    step_id: Option<&str>,
    body_id: Option<&str>,
    segment: Option<&StructuralSegment>,
    steps_by_id: &HashMap<&str, &StructuralStep>,
) -> Option<SourceSite> {
    let step = step_id.and_then(|id| steps_by_id.get(id).copied());
    Some(SourceSite {
        id: source.node_id.clone(),
        subject: Subject {
            spec: source.spec.clone(),
            anchor: source.section_anchor.clone(),
            snapshot_sha: source.snapshot_sha.clone(),
            step_id: step_id.map(str::to_owned),
            step_path: step.map(|step| step.path.clone()),
            body_id: body_id.map(str::to_owned),
        },
        url: source.url.clone(),
        segment_id: segment.map(|segment| segment.source.node_id.clone()),
        span: None,
        step_text: segment.map(|segment| segment.text.clone()),
    })
}

fn subject_from_source(
    source: &SourceIdentity,
    step_id: Option<String>,
    step_path: Option<Vec<u32>>,
    body_id: Option<String>,
) -> Subject {
    Subject {
        spec: source.spec.clone(),
        anchor: source.section_anchor.clone(),
        snapshot_sha: source.snapshot_sha.clone(),
        step_id,
        step_path,
        body_id,
    }
}

fn source_site(
    algorithm: &StructuralAlgorithm,
    source: &SourceIdentity,
    step_id: Option<&str>,
    body_id: Option<&str>,
    segment_id: Option<&str>,
    span: Option<Span>,
    text: Option<String>,
) -> SourceSite {
    let step = step_id.and_then(|id| {
        algorithm
            .steps
            .iter()
            .find(|step| step.source.node_id == id)
    });
    SourceSite {
        id: source.node_id.clone(),
        subject: Subject {
            spec: source.spec.clone(),
            anchor: source.section_anchor.clone(),
            snapshot_sha: source.snapshot_sha.clone(),
            step_id: step_id.map(str::to_owned),
            step_path: step.map(|step| step.path.clone()),
            body_id: body_id.map(str::to_owned),
        },
        url: source.url.clone(),
        segment_id: segment_id.map(str::to_owned),
        span,
        step_text: text,
    }
}
fn execution_owner(algorithm: &StructuralAlgorithm, step_id: &str, body_id: &str) -> String {
    algorithm
        .steps
        .iter()
        .find(|step| step.source.node_id == step_id && step.body_id == body_id)
        .map(|step| step.source.node_id.clone())
        .unwrap_or_else(|| body_id.to_owned())
}

fn segment_owner(algorithm: &StructuralAlgorithm, segment: &StructuralSegment) -> String {
    segment
        .owner_step_id
        .as_deref()
        .and_then(|step_id| {
            algorithm
                .steps
                .iter()
                .find(|step| {
                    step.source.node_id == step_id && step.body_id == segment.owner_body_id
                })
                .map(|step| step.source.node_id.clone())
        })
        .unwrap_or_else(|| segment.owner_body_id.clone())
}

fn operation_relation(
    segment: &StructuralSegment,
    operation: &crate::parse::steps::OperationSite,
) -> (Relationship, Execution) {
    let visible = segment
        .text
        .get(operation.span.start..operation.span.end)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let before = segment
        .text
        .get(..operation.span.start)
        .unwrap_or_default()
        .to_ascii_lowercase();
    const VERBS: &[&str] = &[
        "abort",
        "append",
        "apply",
        "call",
        "clean",
        "continue",
        "dispatch",
        "enqueue",
        "evaluate",
        "execute",
        "fire",
        "invoke",
        "load",
        "navigate",
        "obtain",
        "parse",
        "perform",
        "prepare",
        "queue",
        "resume",
        "run",
        "schedule",
        "serialize",
        "wait",
    ];
    // These verbs identify linked actions, but are too broad to use as
    // preceding cues: "Set x to [an algorithm]" need not invoke it.
    const LINKED_VERBS: &[&str] = &[
        "attempt",
        "check",
        "commit",
        "consider",
        "create",
        "deactivate",
        "destroy",
        "disentangle",
        "finalize",
        "finish",
        "initialise",
        "initialize",
        "notify",
        "register",
        "report",
        "process",
        "reactivate",
        "reset",
        "respond",
        "scroll",
        "set",
        "stop",
        "unload",
        "update",
    ];
    let action_word = |word: &str, verbs: &[&str]| {
        let word = word.trim_matches(|ch: char| !ch.is_alphabetic());
        verbs.iter().any(|verb| {
            word == *verb
                || word
                    .strip_suffix("ing")
                    .is_some_and(|stem| stem == *verb || verb.strip_suffix('e') == Some(stem))
                || word
                    .strip_suffix("ed")
                    .is_some_and(|stem| stem == *verb || verb.strip_suffix('e') == Some(stem))
        })
    };
    let original_action = |word: &str| {
        action_word(word, VERBS) || matches!(word, "running" | "ran" | "applying" | "applied")
    };
    let linked_action = visible
        .strip_prefix("potentially ")
        .unwrap_or(&visible)
        .split_whitespace()
        .next()
        .is_some_and(|word| {
            original_action(word)
                || action_word(word, LINKED_VERBS)
                || matches!(
                    word,
                    "committing" | "committed" | "setting" | "stopping" | "stopped"
                )
        });
    let preceding_action = before
        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
        .rev()
        .take(5)
        .any(original_action);
    if linked_action || preceding_action {
        (Relationship::Invoke, Execution::Inline)
    } else {
        (Relationship::CandidateInvoke, Execution::Unknown)
    }
}

/// Returns true for IDL dfn kinds that name a type, not a member.
/// Links to type-level IDL anchors without an invocation verb are Mentions.
pub(crate) fn is_idl_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "interface"
            | "dictionary"
            | "enum"
            | "typedef"
            | "exception"
            | "callback"
            | "callback interface"
            | "namespace"
            | "mixin"
    )
}

fn owner_for_site(algorithm: &StructuralAlgorithm, site_id: &str) -> Option<String> {
    if let Some(operation) = algorithm
        .operation_sites
        .iter()
        .find(|site| site.source.node_id == site_id)
    {
        return Some(execution_owner(
            algorithm,
            &operation.step_id,
            &operation.body_id,
        ));
    }
    if let Some(invocation) = algorithm
        .body_invocations
        .iter()
        .find(|site| site.source.node_id == site_id)
    {
        return Some(execution_owner(
            algorithm,
            &invocation.step_id,
            &invocation.scope_body_id,
        ));
    }
    algorithm
        .steps
        .iter()
        .find(|step| step.source.node_id == site_id)
        .map(|step| step.source.node_id.clone())
}

fn issue_code(code: &str) -> IssueCode {
    match code {
        "unresolved_body_binding" => IssueCode::UnresolvedBodyBinding,
        "unresolved_invocation" => IssueCode::UnresolvedInvocation,
        "unresolved_argument" => IssueCode::UnresolvedArgument,
        "ambiguous_match" => IssueCode::AmbiguousMatch,
        _ => IssueCode::UnsupportedStructure,
    }
}

fn first_step_segment<'a>(
    algorithm: &'a StructuralAlgorithm,
    step_id: &str,
) -> Option<&'a StructuralSegment> {
    algorithm
        .segments
        .iter()
        .filter(|segment| segment.owner_step_id.as_deref() == Some(step_id))
        .min_by_key(|segment| segment.ordinal)
}

fn preceding_explicit_exit<'a>(
    algorithm: &'a StructuralAlgorithm,
    step_id: &str,
) -> Option<&'a StructuralSegment> {
    let step = algorithm
        .steps
        .iter()
        .find(|step| step.source.node_id == step_id)?;
    algorithm
        .steps
        .iter()
        .filter(|candidate| {
            candidate.body_id == step.body_id
                && candidate.parent_step_id == step.parent_step_id
                && candidate.ordinal < step.ordinal
        })
        .filter_map(|candidate| {
            algorithm
                .segments
                .iter()
                .filter(|segment| segment.owner_step_id.as_ref() == Some(&candidate.source.node_id))
                .filter(|segment| is_explicit_exit(&segment.text))
                .min_by_key(|segment| segment.ordinal)
                .map(|segment| (candidate.ordinal, segment))
        })
        .max_by_key(|(ordinal, _)| *ordinal)
        .map(|(_, segment)| segment)
}

fn is_explicit_exit(text: &str) -> bool {
    let text = text.trim_start().to_ascii_lowercase();
    let starts_with_verb = |verb: &str| {
        text.strip_prefix(verb).is_some_and(|rest| {
            rest.is_empty() || rest.chars().next().is_some_and(|ch| !ch.is_alphanumeric())
        })
    };
    if starts_with_verb("return") || starts_with_verb("throw") {
        return true;
    }
    // Aborting another operation (e.g. a document) does not exit this algorithm.
    ["abort", "terminate"].iter().any(|verb| {
        if !starts_with_verb(verb) {
            return false;
        }
        let rest = text[verb.len()..].trim_start();
        rest.is_empty()
            || rest == "."
            || ["these steps", "this algorithm", "the algorithm"]
                .iter()
                .any(|object| rest.starts_with(object))
    })
}

fn truncate_context_excerpt(text: &str) -> (String, bool) {
    if text.chars().count() <= 512 {
        return (text.to_string(), false);
    }
    let mut chars = text.chars();
    let excerpt: String = chars.by_ref().take(511).collect();
    (format!("{excerpt}…"), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::link::tests::{corpus, engine_fixture_catalog, input_for, multi_corpus};

    #[test]
    fn fragment_is_a_pure_function_of_its_own_spec() {
        let catalog = engine_fixture_catalog();
        let base = corpus(&[
            (
                "DOM",
                "https://dom.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/alpha.html"),
            ),
            (
                "INFRA",
                "https://infra.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/beta.html"),
            ),
        ]);
        let edited_infra = format!(
            "{}<p><dfn id=\"extra\">extra</dfn>. <dfn id=\"b-missing\">now present</dfn>.</p>",
            include_str!("../../tests/fixtures/effects/multi/beta.html")
        );
        let edited = corpus(&[
            (
                "DOM",
                "https://dom.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/alpha.html"),
            ),
            ("INFRA", "https://infra.spec.whatwg.org/", &edited_infra),
        ]);
        assert_ne!(base[1], edited[1]);
        let dom = |sources: &[SourceSpec]| {
            encode_fragment(&build_fragment(&input_for(
                &sources[0],
                &catalog,
                "generic",
            )))
        };
        assert_eq!(dom(&base), dom(&edited));
    }

    #[test]
    fn fragment_encoding_round_trips() {
        let catalog = engine_fixture_catalog();
        let sources = multi_corpus();
        let fragment = build_fragment(&input_for(&sources[0], &catalog, "generic"));
        assert!(!fragment.pending.is_empty());
        assert_eq!(
            decode_fragment(&encode_fragment(&fragment)).unwrap(),
            fragment
        );
    }

    #[test]
    fn aborting_another_operation_is_not_a_preceding_exit() {
        assert!(!is_explicit_exit(
            "abort a document and its descendants given *navigable*."
        ));
        assert!(!is_explicit_exit("Terminate the worker."));
        assert!(is_explicit_exit("Abort these steps."));
        assert!(is_explicit_exit("Terminate this algorithm."));
        assert!(is_explicit_exit("Return *result*."));
        assert!(is_explicit_exit("Throw a TypeError."));
    }
}
