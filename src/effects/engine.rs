//! Pure local matching, execution-graph propagation, and bounded explanations.
//!
//! Callers freeze source snapshots and catalog inputs before invoking this module.
//! It performs no I/O and carries no cache or run-publication policy.

use crate::effects::catalog::{Anchor, Catalog, EmitValue};
use crate::effects::graph::{ExecutionEdge, ExecutionNode};
use crate::effects::matcher::{
    continuation_modes, intrinsic_rules, match_operation_with_issues, match_segment, MatchedEffect,
};
use crate::effects::model::*;
use crate::parse::steps::{
    BodyItem, BodyKind, ContinuationSyntax, ReferenceRole, SourceIdentity, StructuralAlgorithm,
    StructuralSegment, StructuralSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

pub const ANALYSIS_ENGINE_VERSION: u32 = 11;

pub type IssueId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedAnchor {
    pub anchor: String,
    pub url: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub spec: String,
    pub snapshot_sha: String,
    pub base_url: String,
    pub structure: Option<StructuralSpec>,
    #[serde(default)]
    pub anchors: Vec<IndexedAnchor>,
}

pub struct AnalysisInput<'a> {
    pub sources: &'a [SourceSpec],
    pub catalog: &'a Catalog,
    pub environment: &'a str,
    pub scope: AnalysisScope,
    pub budgets: DiscoveryBudgets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisCounts {
    pub bodies: u64,
    pub relationships: u64,
    pub states: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalOccurrence {
    pub id: String,
    pub subject_id: String,
    pub kind: String,
    pub params: EffectParams,
    pub evidence: Vec<Evidence>,
    pub issue_codes: Vec<IssueCode>,
    pub source_order: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct StateKey {
    subject_id: String,
    occurrence_id: String,
    kind: String,
    params: EffectParams,
    execution: Execution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Derivation {
    Local,
    Edge { edge_id: String, child: StateKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PropagatedState {
    key: StateKey,
    derivations: Vec<Derivation>,
}

/// Index the stored derivations, preserving occurrence and execution identity.
/// Backward BFS shares suffixes instead of enumerating every path to them.
struct WitnessGraph<'a> {
    incoming: Vec<Vec<(usize, usize)>>,
    nodes: HashMap<&'a str, &'a ExecutionNode>,
    occurrences: HashMap<&'a str, &'a LocalOccurrence>,
}

impl<'a> WitnessGraph<'a> {
    fn new(artifact: &'a AnalysisArtifact) -> Self {
        let states: BTreeMap<_, _> = artifact
            .states
            .iter()
            .enumerate()
            .map(|(i, state)| (&state.key, i))
            .collect();
        let edges: HashMap<_, _> = artifact
            .relationships
            .iter()
            .enumerate()
            .map(|(i, edge)| (edge.id.as_str(), i))
            .collect();
        let mut incoming = vec![Vec::new(); artifact.states.len()];
        for (parent, state) in artifact.states.iter().enumerate() {
            for derivation in &state.derivations {
                if let Derivation::Edge { edge_id, child } = derivation {
                    if let (Some(&child), Some(&edge)) =
                        (states.get(child), edges.get(edge_id.as_str()))
                    {
                        incoming[child].push((parent, edge));
                    }
                }
            }
        }
        for parents in &mut incoming {
            parents
                .sort_by_key(|&(parent, edge)| (artifact.relationships[edge].source_order, parent));
        }
        Self {
            incoming,
            nodes: artifact
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node))
                .collect(),
            occurrences: artifact
                .occurrences
                .iter()
                .map(|origin| (origin.id.as_str(), origin))
                .collect(),
        }
    }

    fn witness(
        &self,
        artifact: &AnalysisArtifact,
        root: usize,
        next: &[Option<(usize, usize)>],
    ) -> Option<Witness> {
        let origin = self
            .occurrences
            .get(artifact.states[root].key.occurrence_id.as_str())?;
        let mut witness = Witness {
            hops: Vec::new(),
            terminal_evidence: origin.evidence.clone(),
            path_feasibility: PathFeasibility::Unchecked,
            issues: origin
                .issue_codes
                .iter()
                .map(|code| Issue {
                    code: *code,
                    message: "effect argument remains unresolved".into(),
                    site: origin.evidence.first().map(|e| e.site.clone()),
                })
                .collect(),
        };
        let mut state = root;
        while let Some((child, edge)) = next[state] {
            let edge = &artifact.relationships[edge];
            witness.hops.push(WitnessHop {
                from: self.nodes.get(edge.from.as_str())?.subject.clone(),
                to: self.nodes.get(edge.to.as_str())?.subject.clone(),
                relation: edge.relation,
                site: edge.site.clone(),
                context: edge.context.clone(),
                boundary: edge.boundary.clone(),
            });
            if edge.context_truncated
                && !witness
                    .issues
                    .iter()
                    .any(|issue| issue.code == IssueCode::ContextTruncated)
            {
                witness.issues.push(Issue {
                    code: IssueCode::ContextTruncated,
                    message: "witness context was truncated to its display budget".into(),
                    site: Some(edge.site.clone()),
                });
            }
            state = child;
        }
        Some(witness)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisArtifact {
    pub engine_version: u32,
    pub environment: String,
    pub catalog_digest: String,
    pub scope: AnalysisScope,
    pub reached_fixed_point: bool,
    pub counts: AnalysisCounts,
    pub processed_subjects: Vec<Subject>,
    pub unprocessed_subjects: Vec<Subject>,
    pub nodes: Vec<ExecutionNode>,
    pub relationships: Vec<ExecutionEdge>,
    pub occurrences: Vec<LocalOccurrence>,
    states: Vec<PropagatedState>,
    pub issues: Vec<Issue>,
    pub subject_issue_ids: BTreeMap<String, Vec<IssueId>>,
    pub effect_categories: BTreeMap<String, String>,
    pub defined_body_ids: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDefinedBody {
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDefinedBodyRecord {
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub issue_ids: Vec<IssueId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSummaryRecord {
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub defined_bodies: Vec<ArtifactDefinedBodyRecord>,
    pub issue_ids: Vec<IssueId>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSummary {
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub defined_bodies: Vec<ArtifactDefinedBody>,
    pub issues: Vec<Issue>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactExplanation {
    pub summary: ArtifactSummary,
    pub explanations: Vec<EffectExplanation>,
}

pub fn analyze(input: AnalysisInput<'_>) -> Result<AnalysisArtifact, RequestError> {
    analyze_with_local_matches(input, None)
}

pub(crate) fn analyze_with_local_matches(
    input: AnalysisInput<'_>,
    local_matches: Option<&super::local::LocalMatches>,
) -> Result<AnalysisArtifact, RequestError> {
    input.budgets.validate()?;
    if input.environment.is_empty() {
        return Err(RequestError::invalid("environment must be non-empty"));
    }
    let mut builder = Builder::new(&input);
    builder.local_matches = local_matches;
    builder.index_sources();
    builder.add_catalog_declarations();
    builder.add_structural_relationships();
    builder.build_edge_indices();
    builder.mark_opaque_anchors();
    builder.select_scope()?;
    builder.propagate();
    Ok(builder.finish())
}

struct Builder<'a> {
    input: &'a AnalysisInput<'a>,
    local_matches: Option<&'a super::local::LocalMatches>,
    nodes: BTreeMap<String, ExecutionNode>,
    anchor_nodes: BTreeMap<(String, String), String>,
    edges: BTreeMap<String, ExecutionEdge>,
    /// Edge IDs grouped by the node they originate from (`edge.from`).
    /// Built once by [`Self::build_edge_indices`] after all edges are added.
    edges_by_source: HashMap<String, Vec<String>>,
    /// Edge IDs grouped by their target node (`edge.to`).
    /// Built once by [`Self::build_edge_indices`] after all edges are added.
    edges_by_target: HashMap<String, Vec<String>>,
    occurrences: BTreeMap<String, LocalOccurrence>,
    issue_catalog: Vec<Issue>,
    issue_ids: BTreeMap<String, IssueId>,
    issues: BTreeMap<String, Vec<IssueId>>,
    definitions: BTreeMap<String, Vec<String>>,
    states: BTreeMap<StateKey, PropagatedState>,
    selected: BTreeSet<String>,
    active_relationships: BTreeSet<String>,
    unprocessed: BTreeSet<String>,
    fixed: bool,
    order: u64,
}

impl<'a> Builder<'a> {
    fn new(input: &'a AnalysisInput<'a>) -> Self {
        Self {
            input,
            local_matches: None,
            nodes: BTreeMap::new(),
            anchor_nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            edges_by_source: HashMap::new(),
            edges_by_target: HashMap::new(),
            occurrences: BTreeMap::new(),
            issue_catalog: Vec::new(),
            issue_ids: BTreeMap::new(),
            issues: BTreeMap::new(),
            definitions: BTreeMap::new(),
            states: BTreeMap::new(),
            selected: BTreeSet::new(),
            active_relationships: BTreeSet::new(),
            unprocessed: BTreeSet::new(),
            fixed: true,
            order: 0,
        }
    }

    /// Build reverse-lookup indices for `edges` so that callers can look up
    /// incoming or outgoing edges for a node in O(degree) instead of O(|E|).
    /// Must be called after all edges have been added (i.e. after
    /// [`Self::add_structural_relationships`] and before [`Self::select_scope`]).
    fn build_edge_indices(&mut self) {
        for (id, edge) in &self.edges {
            self.edges_by_source
                .entry(edge.from.clone())
                .or_default()
                .push(id.clone());
            self.edges_by_target
                .entry(edge.to.clone())
                .or_default()
                .push(id.clone());
        }
    }

    fn index_sources(&mut self) {
        for source in self.input.sources {
            for anchor in &source.anchors {
                let id = format!("anchor:{}#{}", source.spec, anchor.anchor);
                self.anchor_nodes
                    .entry((source.spec.clone(), anchor.anchor.clone()))
                    .or_insert_with(|| id.clone());
                self.nodes
                    .entry(id.clone())
                    .or_insert_with(|| ExecutionNode {
                        id,
                        subject: Subject {
                            spec: source.spec.clone(),
                            anchor: anchor.anchor.clone(),
                            snapshot_sha: source.snapshot_sha.clone(),
                            step_id: None,
                            step_path: None,
                            body_id: None,
                        },
                        source_order: self.order,
                        is_body: true,
                        definition_only: false,
                    });
                self.order += 1;
            }
            let Some(structure) = &source.structure else {
                continue;
            };
            for algorithm in &structure.algorithms {
                self.index_algorithm(algorithm);
            }
            for issue in &structure.issues {
                let targets: Vec<_> = issue
                    .site_id
                    .as_ref()
                    .and_then(|site| {
                        structure
                            .algorithms
                            .iter()
                            .find_map(|algorithm| owner_for_site(algorithm, site))
                    })
                    .into_iter()
                    .chain(
                        issue
                            .site_id
                            .is_none()
                            .then(|| {
                                structure
                                    .algorithms
                                    .iter()
                                    .map(|algorithm| algorithm.root_body_id.clone())
                                    .collect::<Vec<_>>()
                            })
                            .into_iter()
                            .flatten(),
                    )
                    .collect();
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
    }

    fn index_algorithm(&mut self, algorithm: &StructuralAlgorithm) {
        let root = algorithm.root_body_id.clone();
        self.anchor_nodes.insert(
            (
                algorithm.source.spec.clone(),
                algorithm.source.section_anchor.clone(),
            ),
            root.clone(),
        );
        self.nodes.remove(&format!(
            "anchor:{}#{}",
            algorithm.source.spec, algorithm.source.section_anchor
        ));
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
        for segment in &algorithm.segments {
            let owner = segment_owner(algorithm, segment);
            let matches = self
                .local_matches
                .and_then(|cache| cache.get(&super::local::segment_key(&segment.source.node_id)))
                .map(|cached| cached.effects.clone())
                .unwrap_or_else(|| match_segment(self.input.catalog, algorithm, segment));
            for matched in matches {
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
            let (matches, match_issues) = self
                .local_matches
                .and_then(|cache| cache.get(&operation.source.node_id))
                .map(|cached| (cached.effects.clone(), cached.issues.clone()))
                .unwrap_or_else(|| {
                    let matched = match_operation_with_issues(
                        self.input.catalog,
                        algorithm,
                        segment,
                        operation,
                    );
                    (matched.effects, matched.issues)
                });
            if matches.is_empty() {
                for code in match_issues {
                    self.add_issue(
                        &owner,
                        code,
                        "an anchor-text match covered multiple matching operation references",
                        self.site_for_identity(
                            algorithm,
                            &operation.source,
                            Some(&operation.step_id),
                            Some(&operation.body_id),
                            Some(segment),
                        ),
                    );
                }
            }
            for matched in matches {
                self.add_matched_occurrence(&owner, algorithm, segment, operation, matched);
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
        operation: &crate::parse::steps::OperationSite,
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
        let evidence = matched
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
                reason: None,
            })
            .collect();
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id,
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: matched.issues.iter().copied().collect(),
                source_order: self.order,
            },
        );
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
        let evidence = matched
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
                reason: None,
            })
            .collect();
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id,
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: matched.issues.iter().copied().collect(),
                source_order: self.order,
            },
        );
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

    fn add_catalog_declarations(&mut self) {
        for (rule, matched) in intrinsic_rules(self.input.catalog) {
            let anchor = rule.match_spec.anchor.as_ref().unwrap();
            let Some(node_id) = self
                .anchor_nodes
                .get(&(anchor.spec.clone(), anchor.anchor.clone()))
                .cloned()
            else {
                continue;
            };
            let site = self.anchor_site(anchor);
            self.add_declared_occurrence(&node_id, matched, site, None);
        }
        for summary in self.input.catalog.summaries() {
            let key = (summary.subject.spec.clone(), summary.subject.anchor.clone());
            let Some(node_id) = self.anchor_nodes.get(&key).cloned() else {
                continue;
            };
            let matches = self
                .subject_segments(&summary.subject)
                .iter()
                .any(|text| summary.expect_text.regex().is_match(text));
            if !matches {
                let site = self.anchor_site(&summary.subject);
                self.add_issue(
                    &node_id,
                    IssueCode::DeclarationMismatch,
                    format!(
                        "reviewed summary {} no longer matches its expectation",
                        summary.public_id
                    ),
                    site,
                );
                continue;
            }
            let params = self.emit_constants(&summary.emit);
            let matched = MatchedEffect {
                kind: summary.emit.kind.clone(),
                params,
                rule_ids: BTreeSet::from([summary.public_id.clone()]),
                basis: EvidenceBasis::Declared,
                captures: BTreeMap::new(),
                arguments: BTreeMap::new(),
                span: None,
                issues: BTreeSet::new(),
            };
            self.add_declared_occurrence(
                &node_id,
                matched,
                self.anchor_site(&summary.subject),
                Some(summary.reason.clone()),
            );
        }
    }

    fn emit_constants(&self, emit: &crate::effects::catalog::Emit) -> EffectParams {
        self.input.catalog.effects[&emit.kind]
            .parameters
            .keys()
            .map(|name| {
                let value = match emit.params.get(name).unwrap_or(&EmitValue::Unknown) {
                    EmitValue::Constant(value) => Some(value.clone()),
                    _ => None,
                };
                (name.clone(), value)
            })
            .collect()
    }

    fn add_declared_occurrence(
        &mut self,
        owner: &str,
        matched: MatchedEffect,
        site: Option<SourceSite>,
        reason: Option<String>,
    ) {
        let Some(site) = site else { return };
        let occurrence_id = format!("occ_{}", &digest_serializable(&json!({"site": site.id, "kind": matched.kind, "params": matched.params, "rules": matched.rule_ids})).expect("JSON digest")[..20]);
        let evidence = matched
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
                captures: None,
                argument_expressions: None,
                reason: reason.clone(),
            })
            .collect();
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id,
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: Vec::new(),
                source_order: self.order,
            },
        );
        self.order += 1;
    }

    fn add_structural_relationships(&mut self) {
        for source in self.input.sources {
            let Some(structure) = &source.structure else {
                continue;
            };
            for algorithm in &structure.algorithms {
                self.add_algorithm_relationships(algorithm);
            }
        }
        self.add_implementation_relationships();
    }

    fn add_algorithm_relationships(&mut self, algorithm: &StructuralAlgorithm) {
        let segments: HashMap<_, _> = algorithm
            .segments
            .iter()
            .map(|s| (s.source.node_id.as_str(), s))
            .collect();
        for body in &algorithm.bodies {
            for item in &body.items {
                if let BodyItem::Step(step_id) = item {
                    if self.nodes.contains_key(step_id) {
                        let site = self.site_for_step(algorithm, step_id);
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
                if let crate::parse::steps::StepItem::ChildStep(child) = item {
                    let site = self.site_for_step(algorithm, child);
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
                    self.site_for_identity(
                        algorithm,
                        &invocation.source,
                        Some(&invocation.step_id),
                        Some(&invocation.scope_body_id),
                        None,
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
                let site = self.site_for_identity(
                    algorithm,
                    &invocation.source,
                    Some(&invocation.step_id),
                    Some(&invocation.scope_body_id),
                    None,
                );
                self.add_edge(&owner, body, relation, execution, site, None);
            }
        }
        for operation in &algorithm.operation_sites {
            let owner = execution_owner(algorithm, &operation.step_id, &operation.body_id);
            let Some(segment) = segments.get(operation.segment_id.as_str()) else {
                continue;
            };
            let site = self.site_for_identity(
                algorithm,
                &operation.source,
                Some(&operation.step_id),
                Some(&operation.body_id),
                Some(segment),
            );
            if operation.role == ReferenceRole::Mention {
                if let Some(target) = operation
                    .target
                    .as_ref()
                    .and_then(|target| {
                        self.anchor_nodes
                            .get(&(target.spec.clone(), target.anchor.clone()))
                    })
                    .cloned()
                {
                    self.add_edge(
                        &owner,
                        &target,
                        Relationship::Mention,
                        Execution::Inline,
                        site,
                        None,
                    );
                }
                continue;
            }
            if let Some(target_anchor) = &operation.target {
                if let Some(target) = self
                    .anchor_nodes
                    .get(&(target_anchor.spec.clone(), target_anchor.anchor.clone()))
                    .cloned()
                {
                    let (relation, execution) = operation_relation(segment, operation);
                    self.add_edge(&owner, &target, relation, execution, site.clone(), None);
                    if relation == Relationship::CandidateInvoke {
                        self.add_issue(
                            &owner,
                            IssueCode::UnresolvedInvocation,
                            "algorithm-bearing reference has unresolved invocation syntax",
                            site.clone(),
                        );
                    }
                } else {
                    let code = if self
                        .input
                        .sources
                        .iter()
                        .any(|source| source.spec == target_anchor.spec)
                    {
                        IssueCode::MissingAnchor
                    } else {
                        IssueCode::MissingSpec
                    };
                    self.add_issue(
                        &owner,
                        code,
                        format!(
                            "target {}#{} is not present in the immutable corpus",
                            target_anchor.spec, target_anchor.anchor
                        ),
                        site.clone(),
                    );
                }
            } else {
                self.add_issue(
                    &owner,
                    IssueCode::UnresolvedInvocation,
                    "operation link target could not be resolved",
                    site.clone(),
                );
            }
            if !operation.actual_body_ids.is_empty() {
                let modes = self
                    .local_matches
                    .and_then(|cache| cache.get(&operation.source.node_id))
                    .map(|cached| cached.continuations.clone())
                    .unwrap_or_else(|| {
                        continuation_modes(self.input.catalog, algorithm, segment, operation)
                    });
                if modes.is_empty() {
                    for body in &operation.actual_body_ids {
                        self.add_edge(
                            &owner,
                            body,
                            Relationship::CandidateInvoke,
                            Execution::Unknown,
                            site.clone(),
                            None,
                        );
                    }
                    self.add_issue(
                        &owner,
                        IssueCode::UnresolvedBodyBinding,
                        "step-body argument has no continuation timing contract",
                        site.clone(),
                    );
                } else {
                    if modes.len() > 1 {
                        self.add_issue(
                            &owner,
                            IssueCode::AmbiguousMatch,
                            "matching continuation declarations disagree on execution timing",
                            site.clone(),
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
                                site.clone(),
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
                .edges
                .values()
                .any(|edge| edge.from == owner && edge.to == *remainder && edge.execution == mode);
            if !already {
                let site = self.site_for_identity(
                    algorithm,
                    &continuation.source,
                    Some(&continuation.step_id),
                    Some(&continuation.enclosing_body_id),
                    segments.get(continuation.segment_id.as_str()).copied(),
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
        self.attach_algorithm_context(algorithm);
    }

    fn attach_algorithm_context(&mut self, algorithm: &StructuralAlgorithm) {
        let updates: Vec<_> = self
            .edges
            .values()
            .filter(|edge| {
                edge.site.subject.spec == algorithm.source.spec
                    && edge.site.subject.anchor == algorithm.source.section_anchor
                    && edge.site.subject.snapshot_sha == algorithm.source.snapshot_sha
            })
            .map(|edge| {
                let mut context = Vec::new();
                let mut context_truncated = false;
                let mut step_chain = Vec::new();
                if let Some(step_id) = &edge.site.subject.step_id {
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

                    // The parent step carries ordinary `if`/loop prose when the
                    // parser has no explicit branch container. Keep that source
                    // context from the outside in.
                    for parent_id in step_chain.iter().take(step_chain.len().saturating_sub(1)) {
                        if let Some(segment) = first_step_segment(algorithm, parent_id) {
                            let (text, clipped) = truncate_context_excerpt(&segment.text);
                            context_truncated |= clipped;
                            context.push(ContextItem {
                                kind: ContextKind::Enclosing,
                                text: text.clone(),
                                site: source_site(
                                    algorithm,
                                    &segment.source,
                                    Some(parent_id),
                                    Some(&segment.owner_body_id),
                                    Some(&segment.source.node_id),
                                    None,
                                    Some(text),
                                ),
                            });
                        }
                    }
                    for branch in &algorithm.branches {
                        let contains = branch.items.iter().any(|item| match item {
                            crate::parse::steps::StepItem::ChildStep(id) => ancestors.contains(id),
                            crate::parse::steps::StepItem::Segment(id) => {
                                edge.site.segment_id.as_ref() == Some(id)
                            }
                            _ => false,
                        });
                        if contains {
                            let (text, clipped) = truncate_context_excerpt(&branch.label);
                            context_truncated |= clipped;
                            context.push(ContextItem {
                                kind: ContextKind::Branch,
                                text: text.clone(),
                                site: source_site(
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
                            });
                        }
                    }
                }
                if matches!(
                    edge.relation,
                    Relationship::Invoke
                        | Relationship::CandidateInvoke
                        | Relationship::ScheduleBody
                        | Relationship::Resume
                ) {
                    if let Some(binding) = algorithm
                        .body_definitions
                        .iter()
                        .find(|definition| definition.body_id == edge.to)
                    {
                        let binding_text = format!(
                            "{} is the locally defined body used at this call site",
                            binding.name
                        );
                        let (text, clipped) = truncate_context_excerpt(&binding_text);
                        context_truncated |= clipped;
                        context.push(ContextItem {
                            kind: ContextKind::Binding,
                            text: text.clone(),
                            site: source_site(
                                algorithm,
                                &binding.source,
                                Some(&binding.step_id),
                                Some(&binding.scope_body_id),
                                None,
                                None,
                                Some(text),
                            ),
                        });
                    }
                }
                for step_id in step_chain.iter().rev() {
                    if context
                        .iter()
                        .filter(|item| item.kind == ContextKind::PrecedingExit)
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
                    context.push(ContextItem {
                        kind: ContextKind::PrecedingExit,
                        text: text.clone(),
                        site: source_site(
                            algorithm,
                            &exit.source,
                            exit.owner_step_id.as_deref(),
                            Some(&exit.owner_body_id),
                            Some(&exit.source.node_id),
                            None,
                            Some(text),
                        ),
                    });
                }
                context_truncated |= context.len() > 8;
                context.truncate(8);
                (edge.id.clone(), context, context_truncated)
            })
            .collect();
        for (id, context, context_truncated) in updates {
            if let Some(edge) = self.edges.get_mut(&id) {
                edge.context = context;
                edge.context_truncated = context_truncated;
            }
        }
    }

    fn add_implementation_relationships(&mut self) {
        let declared_operations: BTreeSet<_> = self
            .input
            .catalog
            .implementations()
            .map(|item| (item.operation.spec.clone(), item.operation.anchor.clone()))
            .collect();
        if self.input.environment == "generic" {
            for key in declared_operations {
                if let Some(node) = self.anchor_nodes.get(&key).cloned() {
                    if self
                        .nodes
                        .get(&node)
                        .is_some_and(|node| node.id.starts_with("anchor:"))
                    {
                        self.add_issue(
                            &node,
                            IssueCode::UnresolvedInvocation,
                            "abstract host operation has no generic analyzable implementation",
                            None,
                        );
                    }
                }
            }
            return;
        }
        let mut active_per_operation: BTreeMap<(String, String), usize> = BTreeMap::new();
        for implementation in self
            .input
            .catalog
            .implementations()
            .filter(|item| item.environment == self.input.environment)
        {
            let operation_key = (
                implementation.operation.spec.clone(),
                implementation.operation.anchor.clone(),
            );
            let implementation_key = (
                implementation.implementation.spec.clone(),
                implementation.implementation.anchor.clone(),
            );
            let (Some(from), Some(to)) = (
                self.anchor_nodes.get(&operation_key).cloned(),
                self.anchor_nodes.get(&implementation_key).cloned(),
            ) else {
                if let Some(from) = self.anchor_nodes.get(&operation_key).cloned() {
                    self.add_issue(
                        &from,
                        IssueCode::MissingAnchor,
                        format!(
                            "implementation target {} is unavailable",
                            implementation.implementation.as_identity()
                        ),
                        self.anchor_site(&implementation.operation),
                    );
                }
                continue;
            };
            let expectation = self
                .subject_segments(&implementation.implementation)
                .iter()
                .any(|text| implementation.expect_text.regex().is_match(text));
            if !expectation {
                self.add_issue(
                    &from,
                    IssueCode::DeclarationMismatch,
                    format!(
                        "host implementation {} no longer matches its expectation",
                        implementation.public_id
                    ),
                    self.anchor_site(&implementation.implementation),
                );
                continue;
            }
            let site = self.anchor_site(&implementation.operation);
            self.add_edge(
                &from,
                &to,
                Relationship::Implements,
                Execution::Inline,
                site,
                None,
            );
            *active_per_operation.entry(operation_key).or_default() += 1;
        }
        for (key, count) in active_per_operation {
            if count > 1 {
                if let Some(node) = self.anchor_nodes.get(&key).cloned() {
                    self.add_issue(
                        &node,
                        IssueCode::AmbiguousMatch,
                        "several active host implementations are possible in this environment",
                        self.anchor_site(&Anchor {
                            spec: key.0,
                            anchor: key.1,
                        }),
                    );
                }
            }
        }
    }

    fn mark_opaque_anchors(&mut self) {
        let opaque: Vec<_> = self
            .nodes
            .values()
            .filter(|node| node.id.starts_with("anchor:"))
            .filter(|node| {
                !self
                    .occurrences
                    .values()
                    .any(|occurrence| occurrence.subject_id == node.id)
                    && !self.edges.values().any(|edge| {
                        edge.from == node.id && edge.relation == Relationship::Implements
                    })
            })
            .map(|node| node.id.clone())
            .collect();
        for node in opaque {
            self.add_issue(
                &node,
                IssueCode::UnsupportedStructure,
                "indexed anchor has no reusable algorithm body or declared terminal semantics",
                None,
            );
        }
    }

    fn select_scope(&mut self) -> Result<(), RequestError> {
        let roots: Vec<String> = match &self.input.scope {
            AnalysisScope::All => {
                // Build O(1) lookup sets to avoid O(N×(O+E)) scans per node.
                let nodes_with_occurrences: HashSet<&str> = self
                    .occurrences
                    .values()
                    .map(|occ| occ.subject_id.as_str())
                    .collect();
                let nodes_with_implements: HashSet<&str> = self
                    .edges
                    .values()
                    .filter(|edge| edge.relation == Relationship::Implements)
                    .map(|edge| edge.from.as_str())
                    .collect();
                self.nodes
                    .values()
                    .filter(|node| {
                        (node.is_body
                            && node.subject.body_id.is_none()
                            && !node.id.starts_with("anchor:"))
                            || nodes_with_occurrences.contains(node.id.as_str())
                            || nodes_with_implements.contains(node.id.as_str())
                    })
                    .map(|node| node.id.clone())
                    .collect()
            }
            AnalysisScope::Subject { subject } => vec![self.resolve_selector(subject)?],
        };
        let mut queue: VecDeque<String> = roots.into();
        let mut visited_relationships = BTreeSet::new();
        let mut selected_body_count: u64 = 0;
        while let Some(node) = queue.pop_front() {
            if self.selected.contains(&node) {
                continue;
            }
            let adding_body = self.nodes.get(&node).is_some_and(|node| node.is_body);
            if adding_body && selected_body_count >= self.input.budgets.max_bodies {
                self.fixed = false;
                self.unprocessed.insert(node);
                continue;
            }
            self.selected.insert(node.clone());
            if adding_body {
                selected_body_count += 1;
            }
            let outgoing: Vec<_> = self
                .edges_by_source
                .get(&node)
                .into_iter()
                .flatten()
                .filter_map(|id| self.edges.get(id))
                .cloned()
                .collect();
            for edge in outgoing {
                if visited_relationships.len() as u64 >= self.input.budgets.max_relationships {
                    self.fixed = false;
                    self.unprocessed.insert(edge.to);
                    continue;
                }
                visited_relationships.insert(edge.id.clone());
                self.active_relationships.insert(edge.id.clone());
                if edge.relation != Relationship::Mention {
                    queue.push_back(edge.to);
                }
            }
        }
        if !self.fixed {
            for subject in self.selected.clone() {
                self.add_issue(
                    &subject,
                    IssueCode::AnalysisBudget,
                    "analysis discovery budget was exhausted",
                    None,
                );
            }
        }
        Ok(())
    }

    fn propagate(&mut self) {
        let occurrences: Vec<_> = self
            .occurrences
            .values()
            .filter(|occ| self.selected.contains(&occ.subject_id))
            .cloned()
            .collect();
        let mut queue = VecDeque::new();
        for occurrence in occurrences {
            let key = StateKey {
                subject_id: occurrence.subject_id.clone(),
                occurrence_id: occurrence.id.clone(),
                kind: occurrence.kind.clone(),
                params: occurrence.params.clone(),
                execution: Execution::Inline,
            };
            self.states.insert(
                key.clone(),
                PropagatedState {
                    key: key.clone(),
                    derivations: vec![Derivation::Local],
                },
            );
            queue.push_back(key);
        }
        if self.states.len() as u64 > self.input.budgets.max_states {
            self.fixed = false;
            for subject in self.selected.clone() {
                self.add_issue(
                    &subject,
                    IssueCode::AnalysisBudget,
                    "local effects exceeded the propagation state budget; direct effects were retained",
                    None,
                );
            }
        }
        while let Some(child) = queue.pop_front() {
            let incoming: Vec<_> = self
                .edges_by_target
                .get(&child.subject_id)
                .into_iter()
                .flatten()
                .filter_map(|id| self.edges.get(id))
                .filter(|edge| {
                    edge.relation != Relationship::Mention
                        && self.selected.contains(&edge.from)
                        && self.active_relationships.contains(&edge.id)
                })
                .cloned()
                .collect();
            for edge in incoming {
                if self.same_site_specializes_intrinsic(&edge, &child) {
                    continue;
                }
                let key = StateKey {
                    subject_id: edge.from.clone(),
                    occurrence_id: child.occurrence_id.clone(),
                    kind: child.kind.clone(),
                    params: child.params.clone(),
                    execution: edge.execution.compose(child.execution),
                };
                let derivation = Derivation::Edge {
                    edge_id: edge.id.clone(),
                    child: child.clone(),
                };
                if let Some(existing) = self.states.get_mut(&key) {
                    if !existing.derivations.contains(&derivation) {
                        existing.derivations.push(derivation);
                    }
                    continue;
                }
                let containment = edge.relation == Relationship::Invoke && edge.site.id == edge.to;
                if !containment && self.states.len() as u64 >= self.input.budgets.max_states {
                    self.fixed = false;
                    self.add_issue(
                        &edge.from,
                        IssueCode::AnalysisBudget,
                        "propagation state budget was exhausted",
                        None,
                    );
                    continue;
                }
                self.states.insert(
                    key.clone(),
                    PropagatedState {
                        key: key.clone(),
                        derivations: vec![derivation],
                    },
                );
                queue.push_back(key);
            }
        }
        // Issues follow the same possible-execution edges to callers.
        // Pre-filter active edges once; the fixed-point loop body is then O(active_edges).
        let issue_edges: Vec<_> = self
            .active_relationships
            .iter()
            .filter_map(|id| self.edges.get(id))
            .filter(|edge| {
                edge.relation != Relationship::Mention
                    && self.selected.contains(&edge.from)
                    && self.selected.contains(&edge.to)
            })
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for edge in &issue_edges {
                let inherited = self.issues.get(&edge.to).cloned().unwrap_or_default();
                let target = self.issues.entry(edge.from.clone()).or_default();
                for issue in inherited {
                    if !target.contains(&issue) {
                        target.push(issue);
                        changed = true;
                    }
                }
            }
        }
        if !self.fixed {
            for subject in self.selected.clone() {
                self.add_issue(
                    &subject,
                    IssueCode::AnalysisBudget,
                    "analysis stopped before every selected subject reached a fixed point",
                    None,
                );
            }
        }
    }

    fn same_site_specializes_intrinsic(&self, edge: &ExecutionEdge, child: &StateKey) -> bool {
        let Some(origin) = self.occurrences.get(&child.occurrence_id) else {
            return false;
        };
        if origin.subject_id != edge.to {
            return false;
        }
        self.occurrences.values().any(|local| {
            let shares_rule = local.evidence.iter().any(|left| {
                origin
                    .evidence
                    .iter()
                    .any(|right| left.rule_id == right.rule_id)
            });
            local.subject_id == edge.from
                && local.kind == child.kind
                && local
                    .evidence
                    .iter()
                    .any(|evidence| evidence.site.id == edge.site.id)
                && shares_rule
                && (local.params == child.params || params_dominate(&local.params, &child.params))
        })
    }

    fn finish(self) -> AnalysisArtifact {
        let processed_subjects = self
            .selected
            .iter()
            .filter_map(|id| self.nodes.get(id).map(|node| node.subject.clone()))
            .collect();
        let unprocessed_subjects = self
            .unprocessed
            .iter()
            .filter_map(|id| self.nodes.get(id).map(|node| node.subject.clone()))
            .collect();
        let relationship_count = self.active_relationships.len() as u64;
        let body_count = self
            .selected
            .iter()
            .filter(|id| self.nodes.get(*id).is_some_and(|node| node.is_body))
            .count() as u64;
        AnalysisArtifact {
            engine_version: ANALYSIS_ENGINE_VERSION,
            environment: self.input.environment.to_owned(),
            catalog_digest: self.input.catalog.content_digest.clone(),
            scope: self.input.scope.clone(),
            reached_fixed_point: self.fixed,
            counts: AnalysisCounts {
                bodies: body_count,
                relationships: relationship_count,
                states: self.states.len() as u64,
            },
            processed_subjects,
            unprocessed_subjects,
            nodes: self.nodes.into_values().collect(),
            relationships: self
                .edges
                .into_values()
                .filter(|edge| self.active_relationships.contains(&edge.id))
                .collect(),
            occurrences: self.occurrences.into_values().collect(),
            states: self.states.into_values().collect(),
            issues: self.issue_catalog,
            subject_issue_ids: self.issues,
            effect_categories: self
                .input
                .catalog
                .effects
                .iter()
                .map(|(kind, definition)| (kind.clone(), definition.category.clone()))
                .collect(),
            defined_body_ids: self.definitions,
        }
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
        let id = format!("rel_{}", &digest_serializable(&json!({"from": from, "to": to, "relation": relation, "site": site.id, "execution": execution})).expect("JSON digest")[..20]);
        self.edges.entry(id.clone()).or_insert(ExecutionEdge {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            relation,
            execution,
            site,
            context: Vec::new(),
            context_truncated: false,
            boundary,
            source_order: self.order,
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
        let issue = Issue {
            code,
            message: message.into(),
            site,
        };
        let digest = digest_serializable(&issue).expect("serializable issue");
        let issue_id = if let Some(issue_id) = self.issue_ids.get(&digest).copied() {
            assert_eq!(
                self.issue_catalog.get(issue_id as usize),
                Some(&issue),
                "issue digest collision"
            );
            issue_id
        } else {
            let issue_id = IssueId::try_from(self.issue_catalog.len())
                .expect("issue catalog exceeded u32 address space");
            self.issue_catalog.push(issue);
            self.issue_ids.insert(digest, issue_id);
            issue_id
        };
        let issues = self.issues.entry(subject.to_owned()).or_default();
        if !issues.contains(&issue_id) {
            issues.push(issue_id);
        }
    }

    fn boundary_effect(&self, operation_site_id: &str) -> Option<BoundaryEffect> {
        self.occurrences
            .values()
            .find(|occ| {
                occ.evidence
                    .iter()
                    .any(|evidence| evidence.site.id == operation_site_id)
                    && self
                        .input
                        .catalog
                        .effects
                        .get(&occ.kind)
                        .is_some_and(|definition| definition.category == "async")
            })
            .map(|occ| BoundaryEffect {
                kind: occ.kind.clone(),
                params: occ.params.clone(),
            })
    }

    fn resolve_selector(&self, selector: &SubjectSelector) -> Result<String, RequestError> {
        let candidates: Vec<_> = self
            .nodes
            .values()
            .filter(|node| {
                node.subject.spec == selector.spec
                    && node.subject.anchor == selector.anchor
                    && selector
                        .step_id
                        .as_ref()
                        .is_none_or(|id| node.subject.step_id.as_ref() == Some(id))
                    && selector
                        .step_path
                        .as_ref()
                        .is_none_or(|path| node.subject.step_path.as_ref() == Some(path))
                    && selector
                        .body_id
                        .as_ref()
                        .is_none_or(|id| node.subject.body_id.as_ref() == Some(id))
                    && (selector.step_id.is_some()
                        || selector.step_path.is_some()
                        || selector.body_id.is_some()
                        || node.subject.step_id.is_none() && node.subject.body_id.is_none())
            })
            .collect();
        match candidates.as_slice() {
            [node] => Ok(node.id.clone()),
            [] => Err(request_error(
                RequestErrorCode::SubjectNotFound,
                format!(
                    "subject {}#{} was not found",
                    selector.spec, selector.anchor
                ),
            )),
            _ => Err(request_error(
                RequestErrorCode::AmbiguousSubject,
                format!("subject {}#{} is ambiguous", selector.spec, selector.anchor),
            )),
        }
    }

    fn anchor_site(&self, anchor: &Anchor) -> Option<SourceSite> {
        let node_id = self
            .anchor_nodes
            .get(&(anchor.spec.clone(), anchor.anchor.clone()))?;
        let node = self.nodes.get(node_id)?;
        let source = self
            .input
            .sources
            .iter()
            .find(|source| source.spec == anchor.spec)?;
        let metadata = source
            .anchors
            .iter()
            .find(|item| item.anchor == anchor.anchor);
        Some(SourceSite {
            id: node_id.clone(),
            subject: node.subject.clone(),
            url: metadata
                .map(|item| item.url.clone())
                .unwrap_or_else(|| format!("{}#{}", source.base_url, anchor.anchor)),
            segment_id: None,
            span: None,
            step_text: metadata
                .map(|item| item.text.clone())
                .filter(|text| !text.is_empty()),
        })
    }

    fn subject_segments(&self, anchor: &Anchor) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(source) = self
            .input
            .sources
            .iter()
            .find(|source| source.spec == anchor.spec)
        {
            if let Some(metadata) = source
                .anchors
                .iter()
                .find(|item| item.anchor == anchor.anchor)
            {
                result.push(metadata.text.clone());
            }
            if let Some(structure) = &source.structure {
                if let Some(algorithm) = structure
                    .algorithms
                    .iter()
                    .find(|algorithm| algorithm.source.section_anchor == anchor.anchor)
                {
                    result.extend(
                        algorithm
                            .segments
                            .iter()
                            .map(|segment| segment.text.clone()),
                    );
                }
            }
        }
        result
    }

    fn site_for_step(&self, algorithm: &StructuralAlgorithm, step_id: &str) -> Option<SourceSite> {
        let step = algorithm
            .steps
            .iter()
            .find(|step| step.source.node_id == step_id)?;
        self.site_for_identity(
            algorithm,
            &step.source,
            Some(step_id),
            Some(&step.body_id),
            None,
        )
    }

    fn site_for_identity(
        &self,
        algorithm: &StructuralAlgorithm,
        source: &SourceIdentity,
        step_id: Option<&str>,
        body_id: Option<&str>,
        segment: Option<&StructuralSegment>,
    ) -> Option<SourceSite> {
        let step = step_id.and_then(|id| {
            algorithm
                .steps
                .iter()
                .find(|step| step.source.node_id == id)
        });
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
}

impl AnalysisArtifact {
    pub fn issue(&self, id: IssueId) -> Option<&Issue> {
        self.issues.get(id as usize)
    }

    pub fn issue_catalog(&self) -> impl ExactSizeIterator<Item = (IssueId, &Issue)> {
        self.issues
            .iter()
            .enumerate()
            .map(|(id, issue)| (id as IssueId, issue))
    }

    /// Build compact records for every processed subject in one pass over propagated states.
    /// Persistence should retain their issue IDs and store [`Self::issue_catalog`] once.
    pub fn summary_records(
        &self,
        filter: Option<&EffectFilter>,
    ) -> Result<Vec<ArtifactSummaryRecord>, RequestError> {
        let occurrence_index: HashMap<_, _> = self
            .occurrences
            .iter()
            .map(|occurrence| (occurrence.id.as_str(), occurrence))
            .collect();
        let mut grouped: BTreeMap<String, BTreeMap<(String, EffectParams), Group>> =
            BTreeMap::new();
        for state in &self.states {
            let Some(occurrence) = occurrence_index.get(state.key.occurrence_id.as_str()) else {
                continue;
            };
            let group = grouped
                .entry(state.key.subject_id.clone())
                .or_default()
                .entry((state.key.kind.clone(), state.key.params.clone()))
                .or_default();
            group.execution.insert(state.key.execution);
            group.occurrences.insert(occurrence.id.clone());
            group.rules.extend(
                occurrence
                    .evidence
                    .iter()
                    .map(|evidence| evidence.rule_id.clone()),
            );
        }
        let handles = self.run_effect_handles();
        let processed: BTreeSet<_> = self
            .processed_subjects
            .iter()
            .map(|subject| digest_serializable(subject).expect("serializable subject"))
            .collect();
        let mut summaries: BTreeMap<String, ArtifactSummaryRecord> = BTreeMap::new();
        for node in self.nodes.iter().filter(|node| {
            processed.contains(&digest_serializable(&node.subject).expect("serializable subject"))
        }) {
            let mut effects = Vec::new();
            for ((kind, params), group) in grouped.remove(&node.id).unwrap_or_default() {
                let digest = effect_digest(&kind, &params).expect("serializable effect identity");
                let locations: BTreeSet<_> = group
                    .occurrences
                    .iter()
                    .filter(|id| self.origin_matches_filter(id, filter))
                    .filter_map(|id| occurrence_index.get(id.as_str()))
                    .flat_map(|occurrence| &occurrence.evidence)
                    .map(|evidence| {
                        let subject = &evidence.site.subject;
                        (
                            subject.step_path.is_none(),
                            EffectLocation {
                                spec: subject.spec.clone(),
                                anchor: subject.anchor.clone(),
                                step_path: subject.step_path.clone(),
                                url: evidence.site.url.clone(),
                            },
                        )
                    })
                    .collect();
                let additional_locations = locations.len().saturating_sub(1) as u64;
                let mut locations = locations.into_iter().map(|(_, location)| location);
                let location = locations.next();
                let other_locations = locations.take(MAX_SUMMARY_LOCATIONS - 1).collect();
                let summary = EffectSummary {
                    id: handles[&digest].clone(),
                    kind,
                    params,
                    execution: group.execution.into_iter().collect(),
                    location,
                    other_locations,
                    additional_locations,
                };
                if self.filter_group(&summary, &group.rules, &group.occurrences, filter) {
                    effects.push(summary);
                }
            }
            effects.sort_by(|left, right| {
                category_order(self.effect_categories.get(&left.kind).map(String::as_str))
                    .cmp(&category_order(
                        self.effect_categories.get(&right.kind).map(String::as_str),
                    ))
                    .then_with(|| compare_effect_summaries(left, right))
            });
            let issue_ids = self
                .subject_issue_ids
                .get(&node.id)
                .cloned()
                .unwrap_or_default();
            let coverage = self.coverage_for_ids(&issue_ids);
            summaries.insert(
                node.id.clone(),
                ArtifactSummaryRecord {
                    subject: node.subject.clone(),
                    effects,
                    defined_bodies: Vec::new(),
                    issue_ids,
                    coverage,
                },
            );
        }
        let snapshots = summaries.clone();
        for (step_id, body_ids) in &self.defined_body_ids {
            let Some(summary) = summaries.get_mut(step_id) else {
                continue;
            };
            summary.defined_bodies = body_ids
                .iter()
                .filter_map(|body_id| {
                    let body = snapshots.get(body_id)?;
                    Some(ArtifactDefinedBodyRecord {
                        subject: body.subject.clone(),
                        effects: body.effects.clone(),
                        issue_ids: body.issue_ids.clone(),
                    })
                })
                .collect();
        }
        Ok(summaries.into_values().collect())
    }

    /// Materialize the public summaries, including their issue payloads.
    pub fn summaries(
        &self,
        filter: Option<&EffectFilter>,
    ) -> Result<Vec<ArtifactSummary>, RequestError> {
        self.summary_records(filter)?
            .into_iter()
            .map(|record| self.materialize_record(record))
            .collect()
    }

    pub fn summary(
        &self,
        selector: &SubjectSelector,
        filter: Option<&EffectFilter>,
    ) -> Result<ArtifactSummary, RequestError> {
        selector.validate()?;
        let node_id = self.resolve_selector(selector)?;
        self.summary_records(filter)?
            .into_iter()
            .find(|record| {
                self.nodes
                    .iter()
                    .find(|node| node.id == node_id)
                    .is_some_and(|node| node.subject == record.subject)
            })
            .map(|record| self.materialize_record(record))
            .transpose()?
            .ok_or_else(|| {
                request_error(
                    RequestErrorCode::AnalysisUnavailable,
                    "subject was not materialized in this analysis",
                )
            })
    }

    fn coverage_for_ids(&self, issue_ids: &[IssueId]) -> Coverage {
        if issue_ids
            .iter()
            .filter_map(|id| self.issue(*id))
            .any(|issue| {
                !matches!(
                    issue.code,
                    IssueCode::WitnessBudget | IssueCode::ContextTruncated
                )
            })
        {
            Coverage::Partial
        } else {
            Coverage::Complete
        }
    }

    fn materialize_record(
        &self,
        record: ArtifactSummaryRecord,
    ) -> Result<ArtifactSummary, RequestError> {
        let issues = self.materialize_issues(&record.issue_ids)?;
        let defined_bodies = record
            .defined_bodies
            .into_iter()
            .map(|body| {
                Ok(ArtifactDefinedBody {
                    subject: body.subject,
                    effects: body.effects,
                    issues: self.materialize_issues(&body.issue_ids)?,
                })
            })
            .collect::<Result<_, RequestError>>()?;
        Ok(ArtifactSummary {
            subject: record.subject,
            effects: record.effects,
            defined_bodies,
            issues,
            coverage: record.coverage,
        })
    }

    fn materialize_issues(&self, issue_ids: &[IssueId]) -> Result<Vec<Issue>, RequestError> {
        issue_ids
            .iter()
            .map(|id| {
                self.issue(*id).cloned().ok_or_else(|| {
                    request_error(
                        RequestErrorCode::AnalysisUnavailable,
                        format!("analysis artifact references missing issue {id}"),
                    )
                })
            })
            .collect()
    }

    /// Prepare one shortest representative per subject/effect in one shared
    /// backward BFS. Each propagated state is visited once, including cycles.
    pub(crate) fn prepare_witnesses(
        &self,
        mut emit: impl FnMut(&Subject, &str, Witness) -> Result<(), RequestError>,
    ) -> Result<(), RequestError> {
        let graph = WitnessGraph::new(self);
        let handles = self.run_effect_handles();
        let mut seeds: Vec<_> = self
            .states
            .iter()
            .enumerate()
            .filter(|(_, state)| state.derivations.contains(&Derivation::Local))
            .map(|(i, state)| {
                (
                    graph.occurrences[state.key.occurrence_id.as_str()].source_order,
                    i,
                )
            })
            .collect();
        seeds.sort_unstable();
        let mut queue = VecDeque::new();
        let mut seen = vec![false; self.states.len()];
        let mut next = vec![None; self.states.len()];
        for (_, i) in seeds {
            seen[i] = true;
            queue.push_back(i);
        }
        let mut emitted = BTreeSet::new();
        while let Some(i) = queue.pop_front() {
            let state = &self.states[i];
            let effect = effect_digest(&state.key.kind, &state.key.params)
                .expect("serializable effect identity");
            if emitted.insert((&state.key.subject_id, effect.clone())) {
                if let Some(witness) = graph.witness(self, i, &next) {
                    emit(
                        &graph.nodes[state.key.subject_id.as_str()].subject,
                        &handles[&effect],
                        witness,
                    )?;
                }
            }
            for &(parent, edge) in &graph.incoming[i] {
                if !seen[parent] {
                    seen[parent] = true;
                    next[parent] = Some((i, edge));
                    queue.push_back(parent);
                }
            }
        }
        Ok(())
    }

    pub fn explain(
        &self,
        selector: &SubjectSelector,
        filter: Option<&EffectFilter>,
        options: &ExplanationOptions,
    ) -> Result<ArtifactExplanation, RequestError> {
        options.validate()?;
        let summary = self.summary(selector, filter)?;
        let node_id = self.resolve_selector(selector)?;
        let graph = WitnessGraph::new(self);
        let group_by_effect: BTreeMap<_, _> = summary
            .effects
            .iter()
            .enumerate()
            .map(|(i, effect)| ((&effect.kind, &effect.params), i))
            .collect();
        let mut root_groups = vec![None; self.states.len()];
        let mut origin_groups = HashMap::new();
        let mut root_counts = vec![0usize; summary.effects.len()];
        for (index, state) in self.states.iter().enumerate() {
            if state.key.subject_id == node_id
                && self.origin_matches_filter(&state.key.occurrence_id, filter)
            {
                if let Some(&group) = group_by_effect.get(&(&state.key.kind, &state.key.params)) {
                    root_groups[index] = Some(group);
                    origin_groups.insert(state.key.occurrence_id.as_str(), group);
                    root_counts[group] += 1;
                }
            }
        }
        let mut frontiers = vec![VecDeque::new(); summary.effects.len()];
        let mut distance = vec![None; self.states.len()];
        let mut next = vec![None; self.states.len()];
        let mut seeds: Vec<_> = self
            .states
            .iter()
            .enumerate()
            .filter(|(_, state)| {
                state.derivations.contains(&Derivation::Local)
                    && origin_groups.contains_key(state.key.occurrence_id.as_str())
            })
            .collect();
        seeds.sort_by_key(|(index, state)| {
            (
                graph
                    .occurrences
                    .get(state.key.occurrence_id.as_str())
                    .map_or(u64::MAX, |o| o.source_order),
                *index,
            )
        });
        for (index, state) in seeds {
            let group = origin_groups[state.key.occurrence_id.as_str()];
            distance[index] = Some(0u64);
            frontiers[group].push_back(index);
        }
        let mut witnesses = vec![Vec::new(); summary.effects.len()];
        let mut depth_limited = vec![false; summary.effects.len()];
        let mut remaining = options.max_states;
        // Round-robin across effects keeps a small explicit work budget fair.
        // Each state is enqueued once; next[] is a shared shortest-path tree.
        while remaining > 0 {
            let mut spent = false;
            let filling_first = frontiers
                .iter()
                .enumerate()
                .any(|(group, queue)| !queue.is_empty() && witnesses[group].is_empty());
            for group in 0..frontiers.len() {
                if filling_first && !witnesses[group].is_empty() {
                    continue;
                }
                if remaining == 0 {
                    break;
                }
                if witnesses[group].len() >= (root_counts[group] as u64).min(options.limit) as usize
                {
                    continue;
                }
                let Some(state) = frontiers[group].pop_front() else {
                    continue;
                };
                remaining -= 1;
                spent = true;
                if root_groups[state] == Some(group) {
                    if let Some(witness) = graph.witness(self, state, &next) {
                        witnesses[group].push(witness);
                    }
                    if witnesses[group].len()
                        >= (root_counts[group] as u64).min(options.limit) as usize
                    {
                        continue;
                    }
                }
                let depth = distance[state].expect("queued states have a distance");
                for &(parent, edge) in &graph.incoming[state] {
                    if distance[parent].is_some() {
                        continue;
                    }
                    if depth >= options.max_depth {
                        depth_limited[group] = true;
                        continue;
                    }
                    distance[parent] = Some(depth + 1);
                    next[parent] = Some((state, edge));
                    frontiers[group].push_back(parent);
                }
            }
            if !spent {
                break;
            }
        }
        let mut explanations = Vec::new();
        for (group, effect) in summary.effects.iter().enumerate() {
            let shown = witnesses[group].len();
            let wanted = (root_counts[group] as u64).min(options.limit) as usize;
            let mut issues = Vec::new();
            // An output cap is not a failed graph search. Keep the machine-readable
            // omission flag, without classifying normal sampling as a budget issue.
            if shown < wanted {
                if depth_limited[group] {
                    issues.push(Issue { code: IssueCode::WitnessBudget,
                        message: format!("Trace depth limit ({} graph edges) prevented finding all requested examples.", options.max_depth), site: None });
                }
                if remaining == 0 && !frontiers[group].is_empty() {
                    issues.push(Issue { code: IssueCode::WitnessBudget,
                        message: format!("Trace work limit ({} distinct states shared across selected effects) prevented finding all requested examples.", options.max_states), site: None });
                }
                if issues.is_empty() {
                    issues.push(Issue {
                        code: IssueCode::WitnessBudget,
                        message: "No complete derivation was available for all requested examples."
                            .into(),
                        site: None,
                    });
                }
            }
            explanations.push(EffectExplanation {
                effect_id: effect.id.clone(),
                witnesses: std::mem::take(&mut witnesses[group]),
                witnesses_truncated: shown < root_counts[group],
                issues,
            });
        }
        Ok(ArtifactExplanation {
            summary,
            explanations,
        })
    }

    fn filter_group(
        &self,
        summary: &EffectSummary,
        rules: &BTreeSet<String>,
        occurrences: &BTreeSet<String>,
        filter: Option<&EffectFilter>,
    ) -> bool {
        let Some(filter) = filter else { return true };
        filter
            .kind
            .as_ref()
            .is_none_or(|kind| kind == &summary.kind)
            && filter
                .category
                .as_ref()
                .is_none_or(|category| self.effect_categories.get(&summary.kind) == Some(category))
            && filter
                .rule_id
                .as_ref()
                .is_none_or(|rule| rules.contains(rule))
            && filter.effect_id.as_ref().is_none_or(|id| id == &summary.id)
            && filter
                .occurrence_id
                .as_ref()
                .is_none_or(|id| occurrences.contains(id))
    }

    fn origin_matches_filter(&self, occurrence_id: &str, filter: Option<&EffectFilter>) -> bool {
        let Some(filter) = filter else { return true };
        let Some(occurrence) = self.occurrence(occurrence_id) else {
            return false;
        };
        filter.rule_id.as_ref().is_none_or(|rule| {
            occurrence
                .evidence
                .iter()
                .any(|evidence| &evidence.rule_id == rule)
        }) && filter
            .occurrence_id
            .as_ref()
            .is_none_or(|id| id == occurrence_id)
    }

    fn resolve_selector(&self, selector: &SubjectSelector) -> Result<String, RequestError> {
        let candidates: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.subject.spec == selector.spec
                    && node.subject.anchor == selector.anchor
                    && selector
                        .step_id
                        .as_ref()
                        .is_none_or(|id| node.subject.step_id.as_ref() == Some(id))
                    && selector
                        .step_path
                        .as_ref()
                        .is_none_or(|path| node.subject.step_path.as_ref() == Some(path))
                    && selector
                        .body_id
                        .as_ref()
                        .is_none_or(|id| node.subject.body_id.as_ref() == Some(id))
                    && (selector.step_id.is_some()
                        || selector.step_path.is_some()
                        || selector.body_id.is_some()
                        || node.subject.step_id.is_none() && node.subject.body_id.is_none())
            })
            .collect();
        match candidates.as_slice() {
            [node] if self.processed_subjects.contains(&node.subject) => Ok(node.id.clone()),
            [..] if candidates.is_empty() => Err(request_error(
                RequestErrorCode::SubjectNotFound,
                format!(
                    "subject {}#{} was not found",
                    selector.spec, selector.anchor
                ),
            )),
            [_] => Err(request_error(
                RequestErrorCode::AnalysisUnavailable,
                "subject was not processed before the analysis budget was exhausted",
            )),
            _ => Err(request_error(
                RequestErrorCode::AmbiguousSubject,
                format!("subject {}#{} is ambiguous", selector.spec, selector.anchor),
            )),
        }
    }

    fn occurrence(&self, id: &str) -> Option<&LocalOccurrence> {
        self.occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
    }

    fn run_effect_handles(&self) -> BTreeMap<String, String> {
        let digests: Vec<_> = self
            .states
            .iter()
            .map(|state| {
                effect_digest(&state.key.kind, &state.key.params)
                    .expect("serializable effect identity")
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        digests
            .iter()
            .cloned()
            .zip(effect_handles(&digests))
            .collect()
    }
}

#[derive(Default)]
struct Group {
    execution: BTreeSet<Execution>,
    occurrences: BTreeSet<String>,
    rules: BTreeSet<String>,
}

fn compare_effect_summaries(left: &EffectSummary, right: &EffectSummary) -> std::cmp::Ordering {
    left.kind
        .cmp(&right.kind)
        .then_with(|| compare_params(&left.params, &right.params))
}

fn category_order(category: Option<&str>) -> (u8, &str) {
    match category {
        Some("async") => (0, ""),
        Some("script") => (1, ""),
        Some("events") => (2, ""),
        Some(category) => (3, category),
        None => (4, ""),
    }
}

fn compare_params(left: &EffectParams, right: &EffectParams) -> std::cmp::Ordering {
    for ((left_name, left_value), (right_name, right_value)) in left.iter().zip(right) {
        let order = left_name
            .cmp(right_name)
            .then_with(|| match (left_value, right_value) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
        if !order.is_eq() {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

fn params_dominate(more: &EffectParams, less: &EffectParams) -> bool {
    if more.len() != less.len() {
        return false;
    }
    let mut added = false;
    for (name, less_value) in less {
        match (more.get(name), less_value) {
            (Some(Some(left)), Some(right)) if left != right => return false,
            (Some(None), Some(_)) | (None, _) => return false,
            (Some(Some(_)), None) => added = true,
            _ => {}
        }
    }
    added
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
        "abort", "append", "apply", "call", "clean", "continue", "dispatch", "enqueue", "evaluate",
        "execute", "fire", "invoke", "load", "navigate", "perform", "prepare", "queue", "resume",
        "run", "schedule", "wait",
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

fn request_error(code: RequestErrorCode, message: impl Into<String>) -> RequestError {
    RequestError {
        code,
        message: message.into(),
        details: None,
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
    use crate::effects::catalog::{load_catalog, load_package_files};
    use crate::parse::steps::extract_step_structure;

    #[test]
    fn queued_remainder_has_no_unknown_shortcut_through_window_argument() {
        let html = r##"<div class="algorithm"><p>To <dfn id="root">run root</dfn>:</p><ol>
            <li><a href="#queue">Queue a task</a> on the <a href="#source">task source</a>
                given the <a href="#window">active window</a> to continue these steps.</li>
            <li><a href="#fire">Fire an event</a> named <code>"a"</code>.</li></ol></div>
            <p><dfn id="queue">queue a task</dfn>. <dfn id="fire">fire an event</dfn>.
            <dfn id="source">task source</dfn>. <dfn id="window">active window</dfn>.</p>"##;
        let artifact = artifact_from_html("root", "generic", DiscoveryBudgets::default(), html);
        let result = artifact
            .explain(&selector("root"), None, &ExplanationOptions::default())
            .unwrap();
        let effect = result
            .summary
            .effects
            .iter()
            .find(|effect| effect.kind == "event.fire")
            .unwrap();
        assert_eq!(effect.execution, vec![Execution::Separate]);
        let explanation = result
            .explanations
            .iter()
            .find(|e| e.effect_id == effect.id)
            .unwrap();
        assert!(explanation.witnesses[0]
            .hops
            .iter()
            .any(|hop| hop
                .boundary
                .as_ref()
                .is_some_and(|b| b.execution == Execution::Separate
                    && b.effect
                        .as_ref()
                        .is_some_and(|e| e.kind == "scheduling.enqueue"))));
        assert!(!explanation.witnesses[0]
            .hops
            .iter()
            .any(|hop| hop.to.body_id.is_some() && hop.relation == Relationship::CandidateInvoke));
    }

    #[test]
    fn queued_nested_step_variants_preserve_the_scheduling_boundary() {
        for intro in [
            "run the steps",
            "run these steps",
            "run the following substeps",
        ] {
            let html = format!(
                r##"<div class="algorithm"><p>To <dfn id="root">run root</dfn>:</p><ol>
                <li><a href="#queue">Queue a task</a> given the <a href="#window">active window</a> to {intro}:
                    <ol><li><a href="#fire">Fire an event</a> named <code>"a"</code>.</li></ol>
                </li></ol></div><p><dfn id="queue">queue a task</dfn>.
                <dfn id="fire">fire an event</dfn>. <dfn id="window">active window</dfn>.</p>"##
            );
            let artifact =
                artifact_from_html("root", "generic", DiscoveryBudgets::default(), &html);
            let result = artifact
                .explain(&selector("root"), None, &ExplanationOptions::default())
                .unwrap();
            let effect = result
                .summary
                .effects
                .iter()
                .find(|effect| effect.kind == "event.fire")
                .unwrap();
            assert_eq!(effect.execution, vec![Execution::Separate], "{intro}");
            let explanation = result
                .explanations
                .iter()
                .find(|e| e.effect_id == effect.id)
                .unwrap();
            assert!(
                explanation.witnesses[0].hops.iter().any(|hop| {
                    hop.boundary
                        .as_ref()
                        .is_some_and(|boundary| boundary.execution == Execution::Separate)
                }),
                "{intro}"
            );
        }
    }

    #[test]
    fn ordinary_inflected_call_syntax_is_recognized() {
        for action in [
            "firing a navigation event",
            "evaluating a javascript URL",
            "loading an HTML document",
            "performing an algorithm",
            "commit a navigate event",
            "set the URL",
            "respond to base URL changes",
            "consider speculative loads",
            "update document for history step application",
            "creating and initializing a Document object",
            "process a header",
            "attempt to populate the history entry",
            "stopped parsing",
            "scroll to the fragment",
            "finalize a cross-document navigation",
            "deactivate a document",
            "unload a document",
            "destroy a document",
            "disentangle a port",
            "reactivate a document",
            "checking if unloading is canceled",
            "potentially process scroll behavior",
            "potentially reset the focus",
            "finish an event",
            "report an exception",
            "notify about rejected promises",
            "register speculation rules",
        ] {
            let html = format!(
                r##"<div class="algorithm"><p>To <dfn id="root">run root</dfn>:</p><ol>
                <li>Let result be the result of <a href="#child">{action}</a> given arg.</li></ol></div>
                <div class="algorithm"><p>To <dfn id="child">run child</dfn>:</p><ol><li>Return.</li></ol></div>"##
            );
            let artifact =
                artifact_from_html("root", "generic", DiscoveryBudgets::default(), &html);
            let child = artifact
                .nodes
                .iter()
                .find(|n| {
                    n.subject.anchor == "child"
                        && n.subject.step_id.is_none()
                        && n.subject.body_id.is_none()
                })
                .unwrap();
            assert!(
                artifact
                    .relationships
                    .iter()
                    .any(|edge| edge.to == child.id && edge.relation == Relationship::Invoke),
                "{action}"
            );
        }
    }

    #[test]
    fn assignment_to_an_algorithm_is_not_recognized_as_a_call() {
        let html = r##"<div class="algorithm"><p>To <dfn id="root">run root</dfn>:</p><ol>
            <li>Set <var>callback</var> to <a href="#child">the child algorithm</a>.</li></ol></div>
            <div class="algorithm"><p>To <dfn id="child">run child</dfn>:</p><ol><li>Return.</li></ol></div>"##;
        let artifact = artifact_from_html("root", "generic", DiscoveryBudgets::default(), html);
        let child = artifact
            .nodes
            .iter()
            .find(|node| {
                node.subject.anchor == "child"
                    && node.subject.step_id.is_none()
                    && node.subject.body_id.is_none()
            })
            .unwrap();
        assert!(artifact
            .relationships
            .iter()
            .any(|edge| { edge.to == child.id && edge.relation == Relationship::CandidateInvoke }));
        assert!(!artifact
            .relationships
            .iter()
            .any(|edge| { edge.to == child.id && edge.relation == Relationship::Invoke }));
    }

    #[test]
    fn converging_paths_need_at_most_one_visit_per_effect_state() {
        let mut html = String::new();
        for level in 0..12 {
            html.push_str(&format!(r##"<div class="algorithm"><p>To <dfn id="level-{level}">run level</dfn>:</p>
                <ol><li><a href="#left-{level}">Run left</a>.</li><li><a href="#right-{level}">Run right</a>.</li></ol></div>"##));
            for side in ["left", "right"] {
                html.push_str(&format!(
                    r##"<div class="algorithm"><p>To <dfn id="{side}-{level}">run branch</dfn>:</p>
                    <ol><li><a href="#level-{}">Run next</a>.</li></ol></div>"##,
                    level + 1
                ));
            }
        }
        html.push_str(r##"<div class="algorithm"><p>To <dfn id="level-12">run endpoint</dfn>:</p><ol>
            <li><a href="#fire">Fire an event</a> named <code>"a"</code>.</li></ol></div><p><dfn id="fire">fire an event</dfn>.</p>"##);
        let artifact = artifact_from_html("level-0", "generic", DiscoveryBudgets::default(), &html);
        let explained = artifact
            .explain(
                &selector("level-0"),
                None,
                &ExplanationOptions {
                    max_depth: 256,
                    max_states: artifact.states.len() as u64,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(explained.explanations[0].witnesses.len(), 1);
        assert!(!explained.explanations[0]
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::WitnessBudget));
    }

    #[test]
    fn default_explanation_reaches_deep_effect_without_a_shallow_cutoff() {
        let mut html = String::new();
        for level in 0..40 {
            html.push_str(&format!(
                r##"<div class="algorithm"><p>To <dfn id="level-{level}">run level</dfn>:</p>
                <ol><li><a href="#level-{}">Run the next algorithm</a>.</li></ol></div>"##,
                level + 1
            ));
        }
        html.push_str(
            r##"<div class="algorithm"><p>To <dfn id="level-40">run level</dfn>:</p>
            <ol><li><a href="#fire">Fire an event</a> named <code>"a"</code>.</li></ol></div>
            <dfn id="fire">fire an event</dfn>"##,
        );
        let artifact = artifact_from_html("level-0", "generic", DiscoveryBudgets::default(), &html);
        let result = artifact
            .explain(&selector("level-0"), None, &ExplanationOptions::default())
            .unwrap();
        let effect = result
            .summary
            .effects
            .iter()
            .find(|effect| effect.kind == "event.fire")
            .unwrap();
        let explanation = result
            .explanations
            .iter()
            .find(|e| e.effect_id == effect.id)
            .unwrap();
        assert_eq!(explanation.witnesses.len(), 1);
        assert!(!explanation
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::WitnessBudget));
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

    #[test]
    fn compact_summary_points_to_transitive_endpoint_steps() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="root">run root</dfn>:</p>
          <ol><li><a href="#child">Run child</a>.</li></ol>
        </div><div class="algorithm">
          <p>To <dfn id="child">run child</dfn>:</p>
          <ol start="3"><li>If needed:<ol><li>Continue:<ol start="3">
            <li><a href="#fire">Fire an event</a> named <code>"WASSUP"</code>.</li>
          </ol></li></ol></li>
          <li><a href="#fire">Fire an event</a> named <code>"WASSUP"</code>.</li>
          <li><a href="#fire">Fire an event</a> named <code>"WASSUP"</code>.</li>
          <li><a href="#fire">Fire an event</a> named <code>"WASSUP"</code>.</li>
          <li><a href="#fire">Fire an event</a> named <code>"WASSUP"</code>.</li></ol>
        </div><p><dfn id="fire">fire an event</dfn>.</p>"##;
        let artifact = artifact_from_html("root", "generic", DiscoveryBudgets::default(), html);
        let summary = artifact.summary(&selector("root"), None).unwrap();
        let effect = summary
            .effects
            .iter()
            .find(|effect| {
                effect.params.get("name") == Some(&Some(EffectValue::String("WASSUP".into())))
            })
            .unwrap();
        let value = serde_json::to_value(effect).unwrap();
        assert_eq!(value["location"]["anchor"], "child");
        assert_eq!(value["location"]["step_path"], serde_json::json!([3, 1, 3]));
        assert_eq!(value["additional_locations"], 4);
        assert_eq!(effect.other_locations.len(), MAX_SUMMARY_LOCATIONS - 1);
        assert_eq!(effect.other_locations[0].step_path, Some(vec![4]));
        assert_eq!(effect.other_locations[1].step_path, Some(vec![5]));
    }

    fn artifact(scope_anchor: &str) -> AnalysisArtifact {
        artifact_with(scope_anchor, "generic", DiscoveryBudgets::default())
    }

    fn artifact_with(
        scope_anchor: &str,
        environment: &str,
        budgets: DiscoveryBudgets,
    ) -> AnalysisArtifact {
        artifact_from_html(
            scope_anchor,
            environment,
            budgets,
            include_str!("../../tests/fixtures/effects/engine/acceptance.html"),
        )
    }

    fn artifact_from_html(
        scope_anchor: &str,
        environment: &str,
        budgets: DiscoveryBudgets,
        html: &str,
    ) -> AnalysisArtifact {
        artifact_from_html_and_catalog(
            scope_anchor,
            environment,
            budgets,
            html,
            include_str!("../../tests/fixtures/effects/engine/catalog.yaml"),
        )
    }

    fn artifact_from_html_and_catalog(
        scope_anchor: &str,
        environment: &str,
        budgets: DiscoveryBudgets,
        html: &str,
        catalog_yaml: &str,
    ) -> AnalysisArtifact {
        let package = load_package_files(&[("catalog.yaml", catalog_yaml)]).unwrap();
        let catalog = load_catalog([package]).unwrap();
        let structure =
            extract_step_structure(html, "TEST", "https://example.test/spec", &"a".repeat(64));
        let anchors = structure
            .algorithms
            .iter()
            .map(|algorithm| IndexedAnchor {
                anchor: algorithm.source.section_anchor.clone(),
                url: algorithm.source.url.clone(),
                text: algorithm
                    .segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .chain(
                ["fire", "queue", "consume", "host-operation"]
                    .into_iter()
                    .map(|anchor| IndexedAnchor {
                        anchor: anchor.to_string(),
                        url: format!("https://example.test/spec#{anchor}"),
                        text: anchor.to_string(),
                    }),
            )
            .collect();
        let sources = [SourceSpec {
            spec: "TEST".to_string(),
            snapshot_sha: "a".repeat(64),
            base_url: "https://example.test/spec".to_string(),
            structure: Some(structure),
            anchors,
        }];
        let local_matches = super::super::local::prepare(&sources[0], &catalog);
        analyze_with_local_matches(
            AnalysisInput {
                sources: &sources,
                catalog: &catalog,
                environment,
                scope: AnalysisScope::Subject {
                    subject: SubjectSelector {
                        spec: "TEST".into(),
                        anchor: scope_anchor.into(),
                        step_path: None,
                        step_id: None,
                        body_id: None,
                    },
                },
                budgets,
            },
            Some(&local_matches),
        )
        .unwrap()
    }

    fn selector(anchor: &str) -> SubjectSelector {
        SubjectSelector {
            spec: "TEST".into(),
            anchor: anchor.into(),
            step_path: None,
            step_id: None,
            body_id: None,
        }
    }

    #[test]
    fn explanation_budget_attempts_one_witness_per_group_before_alternatives() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="fair">run fair</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>;
          then <a href="#fire">fire an event</a> named <code>"a"</code>;
          then <a href="#fire">fire an event</a> named <code>"b"</code>.</p></li></ol>
        </div><p><dfn id="fire">fire an event</dfn>.</p>"##;
        let artifact = artifact_from_html("fair", "generic", DiscoveryBudgets::default(), html);
        let explanation = artifact
            .explain(
                &selector("fair"),
                None,
                &ExplanationOptions {
                    max_depth: 4,
                    max_states: 5,
                    limit: 2,
                },
            )
            .unwrap();
        assert_eq!(explanation.explanations.len(), 2);
        assert_eq!(
            explanation
                .explanations
                .iter()
                .map(|effect| effect.witnesses.len())
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
    }

    #[test]
    fn explanation_prefers_the_shortest_route_across_origins() {
        let html = r##"
        <div class="algorithm">
          <p>To <dfn id="ranked">run ranked</dfn>:</p>
          <ol><li><p><a href="#long">Run long</a>; then <a href="#short">run short</a>.</p></li></ol>
        </div>
        <div class="algorithm">
          <p>To <dfn id="long">run long</dfn>:</p>
          <ol><li><p><a href="#middle">Run middle</a>.</p></li></ol>
        </div>
        <div class="algorithm">
          <p>To <dfn id="middle">run middle</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>.</p></li></ol>
        </div>
        <div class="algorithm">
          <p>To <dfn id="short">run short</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>.</p></li></ol>
        </div>
        <p><dfn id="fire">fire an event</dfn>.</p>
        "##;
        let artifact = artifact_from_html("ranked", "generic", DiscoveryBudgets::default(), html);
        let explanation = artifact
            .explain(
                &selector("ranked"),
                None,
                &ExplanationOptions {
                    max_depth: 12,
                    max_states: 100,
                    limit: 1,
                },
            )
            .unwrap();
        let witness = &explanation.explanations[0].witnesses[0];
        assert!(witness.hops.iter().any(|hop| hop.to.anchor == "short"));
        assert!(!witness.hops.iter().any(|hop| hop.to.anchor == "long"));
        assert!(explanation.explanations[0].witnesses_truncated);
        assert!(explanation.explanations[0].issues.is_empty());
        let mut prepared = Vec::new();
        artifact
            .prepare_witnesses(|subject, _, witness| {
                if subject.anchor == "ranked"
                    && subject.step_id.is_none()
                    && subject.body_id.is_none()
                {
                    prepared.push(witness);
                }
                Ok(())
            })
            .unwrap();
        assert!(!prepared.is_empty());
        assert!(prepared
            .iter()
            .all(|w| w.hops.iter().any(|hop| hop.to.anchor == "short")));
        assert!(prepared
            .iter()
            .all(|w| !w.hops.iter().any(|hop| hop.to.anchor == "long")));
    }

    #[test]
    fn depth_pruned_explanation_reports_truncation() {
        let artifact = artifact("cycle-r");
        let explanation = artifact
            .explain(
                &selector("cycle-r"),
                None,
                &ExplanationOptions {
                    max_depth: 1,
                    max_states: 100,
                    limit: 1,
                },
            )
            .unwrap();
        assert!(explanation.explanations[0].witnesses.is_empty());
        assert!(explanation.explanations[0].witnesses_truncated);
        assert!(explanation.explanations[0]
            .issues
            .iter()
            .any(|issue| issue.message.contains("Trace depth limit (1 graph edges)")));
        assert!(explanation.explanations[0]
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::WitnessBudget));
    }

    #[test]
    fn ambiguous_match_without_fallback_makes_the_subject_partial() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="ambiguous">run ambiguous</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>;
          then <a href="#fire">fire an event</a> named <code>"b"</code>.</p></li></ol>
        </div><p><dfn id="fire">fire an event</dfn>.</p>"##;
        let catalog = r#"schema: 1
package: test
effects:
  event.fire:
    category: events
    label: fire an event
    parameters:
      name: {type: string}
rules:
  - id: broad
    match:
      anchor: TEST#fire
      text: '(?i)fire an event named `(?P<name>[^`]+)`.*fire an event'
    emit:
      kind: event.fire
      params:
        name: {capture: name, from: literal}
"#;
        let artifact = artifact_from_html_and_catalog(
            "ambiguous",
            "generic",
            DiscoveryBudgets::default(),
            html,
            catalog,
        );
        let summary = artifact.summary(&selector("ambiguous"), None).unwrap();
        assert!(summary.effects.is_empty());
        assert_eq!(summary.coverage, Coverage::Partial);
        assert!(summary
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::AmbiguousMatch));
        assert!(summary
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::UnresolvedArgument));
    }

    #[test]
    fn operation_scoped_captures_keep_two_direct_occurrences_distinct() {
        let artifact = artifact("direct");
        let result = artifact.summary(&selector("direct"), None).unwrap();
        assert_eq!(
            result
                .effects
                .iter()
                .map(|effect| (&effect.kind, effect.params["name"].as_ref()))
                .collect::<Vec<_>>(),
            vec![
                (
                    &"event.fire".to_string(),
                    Some(&EffectValue::String("a".into()))
                ),
                (
                    &"event.fire".to_string(),
                    Some(&EffectValue::String("b".into()))
                ),
            ]
        );
        assert_eq!(
            artifact
                .occurrences
                .iter()
                .filter(|item| {
                    item.kind == "event.fire"
                        && item
                            .evidence
                            .first()
                            .is_some_and(|evidence| evidence.site.subject.anchor == "direct")
                })
                .count(),
            2
        );
    }

    #[test]
    fn variable_capture_keeps_generic_effect_and_original_expression() {
        let artifact = artifact("variable");
        let result = artifact.summary(&selector("variable"), None).unwrap();
        assert_eq!(result.effects.len(), 1);
        assert_eq!(result.effects[0].params["name"], None);
        assert_eq!(result.coverage, Coverage::Partial);
        let explanation = artifact
            .explain(&selector("variable"), None, &ExplanationOptions::default())
            .unwrap();
        assert_eq!(
            explanation.explanations[0].witnesses[0].terminal_evidence[0]
                .argument_expressions
                .as_ref()
                .unwrap()["name"],
            "*name*"
        );
    }

    #[test]
    fn anchorless_text_rules_match_segments_once_per_regex_occurrence() {
        let text_artifact = artifact("text-only");
        let result = text_artifact.summary(&selector("text-only"), None).unwrap();
        assert_eq!(result.effects.len(), 1);
        assert_eq!(result.effects[0].kind, "script.opportunity");
        assert_eq!(result.effects[0].execution, vec![Execution::Inline]);
        let occurrences: Vec<_> = text_artifact
            .occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.kind == "script.opportunity"
                    && occurrence.evidence[0].site.subject.anchor == "text-only"
            })
            .collect();
        assert_eq!(occurrences.len(), 2);
        assert_ne!(occurrences[0].id, occurrences[1].id);
        assert_ne!(
            occurrences[0].evidence[0].site.span,
            occurrences[1].evidence[0].site.span
        );

        let other = artifact("text-other");
        let other_result = other.summary(&selector("text-other"), None).unwrap();
        assert!(other_result.effects.is_empty());
        assert!(!other
            .occurrences
            .iter()
            .any(|occurrence| occurrence.kind == "script.opportunity"
                && occurrence.evidence[0].site.subject.anchor == "text-other"));
    }

    #[test]
    fn anchorless_text_rule_stays_owned_by_its_defined_body() {
        let artifact = artifact("text-definition");
        let definition = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![1]),
                    ..selector("text-definition")
                },
                None,
            )
            .unwrap();
        assert!(definition.effects.is_empty());
        assert_eq!(definition.defined_bodies.len(), 1);
        assert!(definition.defined_bodies[0]
            .effects
            .iter()
            .any(|effect| effect.kind == "script.opportunity"
                && effect.execution == vec![Execution::Inline]));

        let invocation = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![2]),
                    ..selector("text-definition")
                },
                None,
            )
            .unwrap();
        assert!(invocation.effects.iter().any(|effect| {
            effect.kind == "script.opportunity" && effect.execution == vec![Execution::Separate]
        }));
    }

    #[test]
    fn witness_retains_ordinary_enclosing_step_and_preceding_exit_context() {
        let artifact = artifact("nested-context");
        let explanation = artifact
            .explain(
                &selector("nested-context"),
                Some(&EffectFilter {
                    kind: Some("event.fire".to_string()),
                    ..EffectFilter::default()
                }),
                &ExplanationOptions::default(),
            )
            .unwrap();
        let witness = &explanation.explanations[0].witnesses[0];
        let context: Vec<_> = witness.hops.iter().flat_map(|hop| &hop.context).collect();
        assert!(context.iter().any(|item| {
            item.kind == ContextKind::Enclosing && item.text == "If the document is active, then:"
        }));
        assert!(context
            .iter()
            .any(|item| item.kind == ContextKind::PrecedingExit && item.text == "Return."));
        assert_eq!(witness.path_feasibility, PathFeasibility::Unchecked);
    }

    #[test]
    fn clipped_witness_context_is_bounded_and_reports_an_issue_only_on_the_witness() {
        let clipped_artifact = artifact("clipped-context");
        let summary = clipped_artifact
            .summary(&selector("clipped-context"), None)
            .unwrap();
        assert_eq!(summary.coverage, Coverage::Complete);
        assert!(!summary
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::ContextTruncated));

        let explanation = clipped_artifact
            .explain(
                &selector("clipped-context"),
                Some(&EffectFilter {
                    kind: Some("event.fire".to_string()),
                    ..EffectFilter::default()
                }),
                &ExplanationOptions::default(),
            )
            .unwrap();
        let witness = &explanation.explanations[0].witnesses[0];
        let enclosing = witness
            .hops
            .iter()
            .flat_map(|hop| &hop.context)
            .find(|item| item.kind == ContextKind::Enclosing)
            .unwrap();
        assert_eq!(enclosing.text.chars().count(), 512);
        assert!(enclosing.text.ends_with('…'));
        assert!(witness
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::ContextTruncated));

        let deep = artifact("item-clipped-context")
            .explain(
                &selector("item-clipped-context"),
                Some(&EffectFilter {
                    kind: Some("event.fire".to_string()),
                    ..EffectFilter::default()
                }),
                &ExplanationOptions::default(),
            )
            .unwrap();
        let deep_witness = &deep.explanations[0].witnesses[0];
        assert!(deep_witness.hops.iter().all(|hop| hop.context.len() <= 8));
        assert!(deep_witness.hops.iter().any(|hop| hop.context.len() == 8));
        assert_eq!(
            deep_witness
                .issues
                .iter()
                .filter(|issue| issue.code == IssueCode::ContextTruncated)
                .count(),
            1
        );
    }

    #[test]
    fn definition_step_is_empty_but_defined_body_and_invocation_are_inspectable() {
        let artifact = artifact("caller-r");
        let definition = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![1]),
                    ..selector("caller-r")
                },
                None,
            )
            .unwrap();
        assert!(definition.effects.is_empty());
        assert_eq!(definition.defined_bodies.len(), 1);
        assert_eq!(definition.defined_bodies[0].effects.len(), 1);
        assert_eq!(
            definition.defined_bodies[0].effects[0].execution,
            vec![Execution::Inline]
        );

        let invocation = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![2]),
                    ..selector("caller-r")
                },
                None,
            )
            .unwrap();
        assert!(invocation.effects.iter().any(|effect| {
            effect.kind == "event.fire" && effect.execution == vec![Execution::Separate]
        }));
    }

    #[test]
    fn same_segment_queue_has_distinct_operation_and_body_scope() {
        let artifact = artifact("same-segment");
        let root = artifact.summary(&selector("same-segment"), None).unwrap();
        assert!(root.effects.iter().any(|effect| {
            effect.kind == "scheduling.enqueue" && effect.execution == vec![Execution::Inline]
        }));
        assert!(root.effects.iter().any(|effect| {
            effect.kind == "event.fire" && effect.execution == vec![Execution::Separate]
        }));
        let body = artifact
            .nodes
            .iter()
            .find(|node| {
                node.subject.anchor == "same-segment"
                    && node.subject.body_id.is_some()
                    && node.definition_only
            })
            .unwrap();
        let body_result = artifact
            .summary(
                &SubjectSelector {
                    spec: "TEST".into(),
                    anchor: "same-segment".into(),
                    step_path: None,
                    step_id: None,
                    body_id: body.subject.body_id.clone(),
                },
                None,
            )
            .unwrap();
        assert_eq!(body_result.effects.len(), 1);
        assert_eq!(body_result.effects[0].kind, "event.fire");
        assert_eq!(body_result.effects[0].execution, vec![Execution::Inline]);
    }

    #[test]
    fn conditional_remainder_has_only_inline_and_queued_resume_routes() {
        let html =
            include_str!("../../tests/fixtures/effects/structure/conditional_remainder.html");
        let package = load_package_files(&[(
            "remainder.yaml",
            r#"
schema: 1
package: remainder-test
effects:
  event.fire:
    category: events
    label: fire
    parameters: {name: {type: string}}
  scheduling.enqueue:
    category: async
    label: enqueue
    parameters: {queue: {type: string}}
rules:
  - id: fire
    match:
      anchor: TEST#fire
      text: '(?i)fire an event named (?P<name>`[^`]+`)'
    emit:
      kind: event.fire
      params: {name: {capture: name, from: literal}}
  - id: queue
    match: {anchor: TEST#queue-task}
    emit:
      kind: scheduling.enqueue
      params: {queue: task}
    continuations: separate
"#,
        )])
        .unwrap();
        let catalog = load_catalog([package]).unwrap();
        let structure =
            extract_step_structure(html, "TEST", "https://example.test/spec", &"b".repeat(64));
        let sources = [SourceSpec {
            spec: "TEST".into(),
            snapshot_sha: "b".repeat(64),
            base_url: "https://example.test/spec".into(),
            anchors: ["conditional-remainder", "fire", "queue-task"]
                .into_iter()
                .map(|anchor| IndexedAnchor {
                    anchor: anchor.into(),
                    url: format!("https://example.test/spec#{anchor}"),
                    text: anchor.into(),
                })
                .collect(),
            structure: Some(structure),
        }];
        let artifact = analyze(AnalysisInput {
            sources: &sources,
            catalog: &catalog,
            environment: "generic",
            scope: AnalysisScope::Subject {
                subject: selector("conditional-remainder"),
            },
            budgets: DiscoveryBudgets::default(),
        })
        .unwrap();
        let root = artifact
            .summary(&selector("conditional-remainder"), None)
            .unwrap();
        let event = root
            .effects
            .iter()
            .find(|effect| effect.kind == "event.fire")
            .unwrap();
        assert_eq!(
            event.execution,
            vec![Execution::Inline, Execution::Separate]
        );
        let step_one = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![1]),
                    ..selector("conditional-remainder")
                },
                None,
            )
            .unwrap();
        assert_eq!(
            step_one
                .effects
                .iter()
                .find(|effect| effect.kind == "event.fire")
                .unwrap()
                .execution,
            vec![Execution::Inline, Execution::Separate]
        );
        let step_two = artifact
            .summary(
                &SubjectSelector {
                    step_path: Some(vec![2]),
                    ..selector("conditional-remainder")
                },
                None,
            )
            .unwrap();
        assert_eq!(step_two.effects[0].execution, vec![Execution::Inline]);
    }

    #[test]
    fn call_site_bodies_do_not_pollute_other_callers_or_primitive() {
        let r = artifact("caller-r");
        let r_summary = r.summary(&selector("caller-r"), None).unwrap();
        assert!(r_summary
            .effects
            .iter()
            .any(|effect| effect.kind == "event.fire"
                && effect.params["name"] == Some(EffectValue::String("a".into()))
                && effect.execution == vec![Execution::Separate]));
        assert!(!r_summary.effects.iter().any(
            |effect| effect.params.get("name") == Some(&Some(EffectValue::String("b".into())))
        ));

        let s = artifact("caller-s");
        let s_summary = s.summary(&selector("caller-s"), None).unwrap();
        assert!(s_summary
            .effects
            .iter()
            .any(|effect| effect.kind == "event.fire"
                && effect.params["name"] == Some(EffectValue::String("b".into()))));
        assert!(!s_summary.effects.iter().any(
            |effect| effect.params.get("name") == Some(&Some(EffectValue::String("a".into())))
        ));

        let queue = artifact("queue");
        let queue_summary = queue.summary(&selector("queue"), None).unwrap();
        assert_eq!(queue_summary.effects.len(), 1);
        assert_eq!(queue_summary.effects[0].kind, "scheduling.enqueue");
    }

    #[test]
    fn occurrence_aware_propagation_converges_on_cycles_and_explanation_is_bounded() {
        let artifact = artifact("cycle-r");
        assert!(artifact.reached_fixed_point);
        let result = artifact.summary(&selector("cycle-r"), None).unwrap();
        assert_eq!(result.effects.len(), 1);
        assert_eq!(
            result.effects[0].params["name"],
            Some(EffectValue::String("a".into()))
        );
        let explanation = artifact
            .explain(
                &selector("cycle-r"),
                None,
                &ExplanationOptions {
                    max_depth: 8,
                    max_states: 20,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(explanation.explanations.len(), 1);
        assert_eq!(explanation.explanations[0].witnesses.len(), 1);
        assert!(explanation.explanations[0].witnesses[0].hops.len() <= 4);
    }

    #[test]
    fn unknown_body_timing_is_candidate_partial_and_call_site_local() {
        let artifact = artifact("unknown-timing");
        let result = artifact.summary(&selector("unknown-timing"), None).unwrap();
        let event = result
            .effects
            .iter()
            .find(|effect| effect.kind == "event.fire")
            .unwrap();
        assert_eq!(event.params["name"], Some(EffectValue::String("a".into())));
        assert_eq!(event.execution, vec![Execution::Unknown]);
        assert_eq!(result.coverage, Coverage::Partial);
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::UnresolvedBodyBinding));
    }

    #[test]
    fn opaque_indexed_anchor_is_partial_instead_of_confidently_empty() {
        let artifact = artifact("consume");
        let result = artifact.summary(&selector("consume"), None).unwrap();
        assert!(result.effects.is_empty());
        assert_eq!(result.coverage, Coverage::Partial);
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::UnsupportedStructure));
    }

    #[test]
    fn shared_issue_catalog_keeps_fanout_compact_and_roundtrips() {
        let mut artifact = artifact("unknown-timing");
        let issue_id = artifact
            .issue_catalog()
            .find(|(_, issue)| issue.code == IssueCode::UnresolvedBodyBinding)
            .map(|(id, _)| id)
            .unwrap();
        artifact.issues[issue_id as usize].message = "x".repeat(4096);

        for index in 0..500 {
            artifact
                .subject_issue_ids
                .insert(format!("synthetic-{index}"), vec![issue_id]);
        }

        let compact = serde_json::to_vec(&artifact).unwrap();
        let mut legacy = serde_json::to_value(&artifact).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("issues");
        object.remove("subject_issue_ids");
        object.insert(
            "subject_issues".into(),
            serde_json::Value::Object(
                artifact
                    .subject_issue_ids
                    .iter()
                    .map(|(subject, ids)| {
                        (
                            subject.clone(),
                            serde_json::to_value(
                                ids.iter()
                                    .map(|id| artifact.issue(*id).unwrap())
                                    .collect::<Vec<_>>(),
                            )
                            .unwrap(),
                        )
                    })
                    .collect(),
            ),
        );
        let duplicated = serde_json::to_vec(&legacy).unwrap();
        assert!(
            compact.len() * 8 < duplicated.len(),
            "compact={} duplicate={}",
            compact.len(),
            duplicated.len()
        );

        let roundtripped: AnalysisArtifact = serde_json::from_slice(&compact).unwrap();
        assert_eq!(roundtripped, artifact);
        assert_eq!(
            roundtripped
                .summary(&selector("unknown-timing"), None)
                .unwrap(),
            artifact.summary(&selector("unknown-timing"), None).unwrap()
        );
    }

    #[test]
    fn direct_effects_survive_discovery_and_explanation_caps() {
        let artifact = artifact_with(
            "direct",
            "generic",
            DiscoveryBudgets {
                max_bodies: 1,
                max_relationships: 1,
                max_states: 1,
            },
        );
        let result = artifact.summary(&selector("direct"), None).unwrap();
        assert_eq!(result.effects.len(), 2);
        assert_eq!(result.coverage, Coverage::Partial);
        let explanation = artifact
            .explain(
                &selector("direct"),
                None,
                &ExplanationOptions {
                    max_depth: 1,
                    max_states: 1,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(explanation.summary.effects.len(), 2);
        assert!(explanation
            .explanations
            .iter()
            .any(|item| item.witnesses_truncated));
    }

    #[test]
    fn environment_mapping_is_explicit_and_mentions_do_not_propagate() {
        let generic = artifact("host-caller");
        let generic_result = generic.summary(&selector("host-caller"), None).unwrap();
        assert!(generic_result.effects.is_empty());
        assert_eq!(generic_result.coverage, Coverage::Partial);

        let web = artifact_with("host-caller", "web", DiscoveryBudgets::default());
        let web_result = web.summary(&selector("host-caller"), None).unwrap();
        assert!(web_result
            .effects
            .iter()
            .any(|effect| effect.kind == "scheduling.enqueue"));
        let explanation = web
            .explain(
                &selector("host-caller"),
                None,
                &ExplanationOptions::default(),
            )
            .unwrap();
        assert!(explanation
            .explanations
            .iter()
            .any(|effect| effect.witnesses.iter().any(|witness| witness
                .hops
                .iter()
                .any(|hop| hop.relation == Relationship::Implements))));

        let mention = artifact("mention");
        let mention_result = mention.summary(&selector("mention"), None).unwrap();
        assert!(mention_result.effects.is_empty());
        assert_eq!(mention_result.coverage, Coverage::Complete);
        assert!(mention
            .relationships
            .iter()
            .any(|edge| edge.relation == Relationship::Mention));
    }
}
