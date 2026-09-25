//! Deterministic link step: per-spec fragments in, one execution graph out.
//!
//! Fragments are combined in spec-name order, so the graph depends only on the
//! fragment set. Each item's `source_order` is `phase << 62 | rank << 48 | local`:
//! phase 0 holds fragment items, phase 1 catalog declarations and host
//! implementations; rank is the spec's position in sorted order.

use crate::effects::catalog::{Anchor, Catalog, EmitValue};
use crate::effects::engine::{Graph, IssueId, LocalOccurrence, ANALYSIS_ENGINE_VERSION};
use crate::effects::fragment::{
    edge_id, evidence_for, is_idl_type_kind, missing_target_message, Fragment, PendingSite,
};
use crate::effects::graph::{site_key, ExecutionEdge, ExecutionNode};
use crate::effects::matcher::{intrinsic_rules, MatchedEffect};
use crate::effects::model::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct Linked {
    pub graph: Graph,
    pub missing_inputs: Vec<MissingInput>,
    /// Spec -> other spec -> number of non-Mention edges between their nodes.
    pub spec_deps: BTreeMap<String, BTreeMap<String, u64>>,
}

pub fn link(
    fragments: &[Fragment],
    catalog: &Catalog,
    environment: &str,
) -> Result<Linked, RequestError> {
    if environment.is_empty() {
        return Err(RequestError::invalid("environment must be non-empty"));
    }
    let mut ordered: Vec<&Fragment> = fragments.iter().collect();
    ordered.sort_by(|left, right| left.spec.cmp(&right.spec));
    let mut linker = Linker::new(&ordered, catalog, environment);
    linker.copy_fragments();
    linker.resolve_pending();
    linker.add_catalog_declarations();
    linker.add_implementation_relationships();
    linker.mark_opaque_anchors();
    Ok(linker.finish())
}

const FRAGMENT_PHASE: u64 = 0;
const CATALOG_PHASE: u64 = 1;

fn source_order(phase: u64, rank: u64, local: u64) -> u64 {
    debug_assert!(rank < 1 << 14 && local < 1 << 48);
    phase << 62 | rank << 48 | local
}

/// What the linker knows about one indexed spec.
struct SpecSymbols<'f> {
    fragment: &'f Fragment,
    /// First position of each anchor in the anchor list.
    anchor_index: HashMap<&'f str, usize>,
    idl_type_anchors: HashSet<&'f str>,
}

impl SpecSymbols<'_> {
    /// The node an anchor of this spec resolves to: its algorithm root or its anchor node.
    fn resolve(&self, anchor: &str) -> Option<String> {
        if let Some(root) = self.fragment.algorithm_roots.get(anchor) {
            return Some(root.clone());
        }
        self.anchor_index
            .contains_key(anchor)
            .then(|| format!("anchor:{}#{anchor}", self.fragment.spec))
    }

    fn metadata(&self, anchor: &str) -> Option<&crate::effects::engine::IndexedAnchor> {
        self.anchor_index
            .get(anchor)
            .map(|&index| &self.fragment.anchors[index])
    }
}

struct Linker<'f> {
    catalog: &'f Catalog,
    environment: &'f str,
    fragments: &'f [&'f Fragment],
    specs: BTreeMap<&'f str, SpecSymbols<'f>>,
    nodes: BTreeMap<String, ExecutionNode>,
    anchor_nodes: BTreeMap<(String, String), String>,
    edges: BTreeMap<String, ExecutionEdge>,
    occurrences: BTreeMap<String, LocalOccurrence>,
    issue_catalog: Vec<GraphIssue>,
    issue_ids: HashMap<GraphIssue, IssueId>,
    /// Node -> (source order key, issue id), in attach order.
    node_issues: BTreeMap<String, Vec<(u64, IssueId)>>,
    definitions: BTreeMap<String, Vec<String>>,
    sites: BTreeMap<String, SourceSite>,
    missing_inputs: BTreeSet<(String, String)>,
    catalog_order: u64,
}

impl<'f> Linker<'f> {
    fn new(fragments: &'f [&'f Fragment], catalog: &'f Catalog, environment: &'f str) -> Self {
        let specs = fragments
            .iter()
            .map(|fragment| {
                let mut anchor_index = HashMap::new();
                let mut idl_type_anchors = HashSet::new();
                for (index, anchor) in fragment.anchors.iter().enumerate() {
                    anchor_index.entry(anchor.anchor.as_str()).or_insert(index);
                    if anchor.idl_kind.as_deref().is_some_and(is_idl_type_kind) {
                        idl_type_anchors.insert(anchor.anchor.as_str());
                    }
                }
                (
                    fragment.spec.as_str(),
                    SpecSymbols {
                        fragment,
                        anchor_index,
                        idl_type_anchors,
                    },
                )
            })
            .collect();
        Self {
            catalog,
            environment,
            fragments,
            specs,
            nodes: BTreeMap::new(),
            anchor_nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            issue_catalog: Vec::new(),
            issue_ids: HashMap::new(),
            node_issues: BTreeMap::new(),
            definitions: BTreeMap::new(),
            sites: BTreeMap::new(),
            missing_inputs: BTreeSet::new(),
            catalog_order: 0,
        }
    }

    fn copy_fragments(&mut self) {
        for (rank, fragment) in self.fragments.iter().enumerate() {
            let order = |local| source_order(FRAGMENT_PHASE, rank as u64, local);
            for (index, anchor) in fragment.anchors.iter().enumerate() {
                if fragment.algorithm_roots.contains_key(&anchor.anchor) {
                    continue;
                }
                let id = format!("anchor:{}#{}", fragment.spec, anchor.anchor);
                self.anchor_nodes
                    .entry((fragment.spec.clone(), anchor.anchor.clone()))
                    .or_insert_with(|| id.clone());
                self.nodes
                    .entry(id.clone())
                    .or_insert_with(|| ExecutionNode {
                        id,
                        subject: Subject {
                            spec: fragment.spec.clone(),
                            anchor: anchor.anchor.clone(),
                            snapshot_sha: fragment.snapshot_sha.clone(),
                            step_id: None,
                            step_path: None,
                            body_id: None,
                        },
                        source_order: order(index as u64),
                        is_body: true,
                        definition_only: false,
                    });
            }
            for (anchor, root) in &fragment.algorithm_roots {
                self.anchor_nodes
                    .insert((fragment.spec.clone(), anchor.clone()), root.clone());
            }
            for node in &fragment.nodes {
                let mut node = node.clone();
                node.source_order = order(node.source_order);
                self.nodes.insert(node.id.clone(), node);
            }
            for edge in &fragment.edges {
                let mut edge = edge.clone();
                edge.source_order = order(edge.source_order);
                self.edges.entry(edge.id.clone()).or_insert(edge);
            }
            for occurrence in &fragment.occurrences {
                let mut occurrence = occurrence.clone();
                occurrence.source_order = order(occurrence.source_order);
                self.occurrences.insert(occurrence.id.clone(), occurrence);
            }
            for (key, site) in &fragment.sites {
                self.sites
                    .entry(key.clone())
                    .or_insert_with(|| site.clone());
            }
            for (step, bodies) in &fragment.definitions {
                self.definitions
                    .entry(step.clone())
                    .or_default()
                    .extend(bodies.iter().cloned());
            }
            let ids: Vec<IssueId> = fragment
                .issues
                .iter()
                .map(|issue| self.intern_issue(issue.clone()))
                .collect();
            for (node, entries) in &fragment.node_issues {
                let target = self.node_issues.entry(node.clone()).or_default();
                target.extend(
                    entries
                        .iter()
                        .map(|&(index, local)| (order(local), ids[index as usize])),
                );
            }
            self.missing_inputs.extend(
                fragment
                    .local_missing
                    .iter()
                    .map(|anchor| (fragment.spec.clone(), anchor.clone())),
            );
        }
    }

    fn resolve_pending(&mut self) {
        for (rank, fragment) in self.fragments.iter().enumerate() {
            for pending in &fragment.pending {
                self.resolve_site(rank as u64, pending);
            }
        }
    }

    fn resolve_site(&mut self, rank: u64, pending: &PendingSite) {
        let order = source_order(FRAGMENT_PHASE, rank, pending.order);
        let target = &pending.target;
        let symbols = self.specs.get(target.spec.as_str());
        if !pending.mention
            && !symbols
                .is_some_and(|symbols| symbols.anchor_index.contains_key(target.anchor.as_str()))
        {
            self.missing_inputs
                .insert((target.spec.clone(), target.anchor.clone()));
        }
        let resolved = symbols.map(|symbols| {
            (
                symbols.resolve(&target.anchor),
                symbols.idl_type_anchors.contains(target.anchor.as_str()),
            )
        });
        match resolved {
            Some((Some(to), is_idl_type)) => {
                // Without a verb-based invocation signal, a link to a non-algorithm
                // section or an IDL type definition is a concept mention.
                let demote = pending.relation == Relationship::CandidateInvoke
                    && (to.starts_with("anchor:") || is_idl_type);
                let (relation, execution) = if pending.mention || demote {
                    (Relationship::Mention, Execution::Inline)
                } else {
                    (pending.relation, pending.execution)
                };
                self.add_pending_edge(pending, &to, relation, execution, order);
                if relation == Relationship::CandidateInvoke {
                    self.add_issue(
                        &pending.owner,
                        IssueCode::UnresolvedInvocation,
                        "algorithm-bearing reference has unresolved invocation syntax",
                        Some(pending.site_key.clone()),
                        order,
                    );
                }
            }
            _ if pending.mention => {}
            Some((None, _)) => self.add_issue(
                &pending.owner,
                IssueCode::MissingAnchor,
                missing_target_message(target),
                Some(pending.site_key.clone()),
                order,
            ),
            None => self.add_issue(
                &pending.owner,
                IssueCode::MissingSpec,
                missing_target_message(target),
                Some(pending.site_key.clone()),
                order,
            ),
        }
    }

    fn add_pending_edge(
        &mut self,
        pending: &PendingSite,
        to: &str,
        relation: Relationship,
        execution: Execution,
        order: u64,
    ) {
        if !self.nodes.contains_key(&pending.owner) || !self.nodes.contains_key(to) {
            return;
        }
        let id = edge_id(&pending.owner, to, relation, &pending.site_id, execution);
        self.edges
            .entry(id.clone())
            .or_insert_with(|| ExecutionEdge {
                id,
                from: pending.owner.clone(),
                to: to.to_owned(),
                relation,
                execution,
                site_key: pending.site_key.clone(),
                site_id: pending.site_id.clone(),
                context: pending.context.clone(),
                context_truncated: pending.context_truncated,
                boundary: pending.boundary.clone(),
                source_order: order,
            });
    }

    fn add_catalog_declarations(&mut self) {
        for (rule, matched) in intrinsic_rules(self.catalog) {
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
        for summary in self.catalog.summaries() {
            let key = (summary.subject.spec.clone(), summary.subject.anchor.clone());
            let Some(node_id) = self.anchor_nodes.get(&key).cloned() else {
                continue;
            };
            if !self.expectation(&summary.subject.spec, &summary.public_id) {
                let site = self.anchor_site(&summary.subject);
                self.add_catalog_issue(
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
            let site = self.anchor_site(&summary.subject);
            self.add_declared_occurrence(&node_id, matched, site, Some(&summary.reason));
        }
    }

    /// Whether catalog item `public_id` matched its expectation in `spec`'s fragment.
    fn expectation(&self, spec: &str, public_id: &str) -> bool {
        self.specs
            .get(spec)
            .and_then(|symbols| symbols.fragment.expectations.get(public_id))
            .copied()
            .unwrap_or(false)
    }

    fn emit_constants(&self, emit: &crate::effects::catalog::Emit) -> EffectParams {
        self.catalog.effects[&emit.kind]
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
        reason: Option<&str>,
    ) {
        let Some(site) = site else { return };
        let occurrence_id = format!(
            "occ_{}",
            &digest_serializable(&json!({"site": site.id, "kind": matched.kind, "params": matched.params, "rules": matched.rule_ids}))
                .expect("JSON digest")[..20]
        );
        let evidence = evidence_for(&occurrence_id, &matched, &site, reason);
        self.occurrences.insert(
            occurrence_id.clone(),
            LocalOccurrence {
                id: occurrence_id,
                subject_id: owner.to_owned(),
                kind: matched.kind,
                params: matched.params,
                evidence,
                issue_codes: Vec::new(),
                source_order: source_order(CATALOG_PHASE, 0, self.catalog_order),
            },
        );
        self.catalog_order += 1;
    }

    fn add_implementation_relationships(&mut self) {
        let catalog = self.catalog;
        let environment = self.environment;
        let declared_operations: BTreeSet<_> = catalog
            .implementations()
            .map(|item| (item.operation.spec.clone(), item.operation.anchor.clone()))
            .collect();
        if environment == "generic" {
            for key in declared_operations {
                if let Some(node) = self.anchor_nodes.get(&key).cloned() {
                    if node.starts_with("anchor:") && self.nodes.contains_key(&node) {
                        self.add_catalog_issue(
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
        for implementation in catalog
            .implementations()
            .filter(|item| item.environment == environment)
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
                    let site = self.anchor_site(&implementation.operation);
                    self.add_catalog_issue(
                        &from,
                        IssueCode::MissingAnchor,
                        format!(
                            "implementation target {} is unavailable",
                            implementation.implementation.as_identity()
                        ),
                        site,
                    );
                }
                continue;
            };
            if !self.expectation(
                &implementation.implementation.spec,
                &implementation.public_id,
            ) {
                let site = self.anchor_site(&implementation.implementation);
                self.add_catalog_issue(
                    &from,
                    IssueCode::DeclarationMismatch,
                    format!(
                        "host implementation {} no longer matches its expectation",
                        implementation.public_id
                    ),
                    site,
                );
                continue;
            }
            let site = self.anchor_site(&implementation.operation);
            self.add_catalog_edge(&from, &to, site);
            *active_per_operation.entry(operation_key).or_default() += 1;
        }
        for (key, count) in active_per_operation {
            if count > 1 {
                if let Some(node) = self.anchor_nodes.get(&key).cloned() {
                    let site = self.anchor_site(&Anchor {
                        spec: key.0,
                        anchor: key.1,
                    });
                    self.add_catalog_issue(
                        &node,
                        IssueCode::AmbiguousMatch,
                        "several active host implementations are possible in this environment",
                        site,
                    );
                }
            }
        }
    }

    fn add_catalog_edge(&mut self, from: &str, to: &str, site: Option<SourceSite>) {
        let Some(site) = site else { return };
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return;
        }
        let site_id = site.id.clone();
        let relation = Relationship::Implements;
        let execution = Execution::Inline;
        let id = edge_id(from, to, relation, &site_id, execution);
        let site_key = self.intern_site(site);
        let order = source_order(CATALOG_PHASE, 0, self.catalog_order);
        self.edges
            .entry(id.clone())
            .or_insert_with(|| ExecutionEdge {
                id,
                from: from.to_owned(),
                to: to.to_owned(),
                relation,
                execution,
                site_key,
                site_id,
                context: Vec::new(),
                context_truncated: false,
                boundary: None,
                source_order: order,
            });
        self.catalog_order += 1;
    }

    fn mark_opaque_anchors(&mut self) {
        let with_occurrences: HashSet<&str> = self
            .occurrences
            .values()
            .map(|o| o.subject_id.as_str())
            .collect();
        let implements_sources: HashSet<&str> = self
            .edges
            .values()
            .filter(|e| e.relation == Relationship::Implements)
            .map(|e| e.from.as_str())
            .collect();
        let opaque: Vec<_> = self
            .nodes
            .keys()
            .filter(|id| id.starts_with("anchor:"))
            .filter(|id| {
                !with_occurrences.contains(id.as_str()) && !implements_sources.contains(id.as_str())
            })
            .cloned()
            .collect();
        for node in opaque {
            self.add_catalog_issue(
                &node,
                IssueCode::UnsupportedStructure,
                "indexed anchor has no reusable algorithm body or declared terminal semantics",
                None,
            );
        }
    }

    fn anchor_site(&self, anchor: &Anchor) -> Option<SourceSite> {
        let node_id = self
            .anchor_nodes
            .get(&(anchor.spec.clone(), anchor.anchor.clone()))?;
        let node = self.nodes.get(node_id)?;
        let symbols = self.specs.get(anchor.spec.as_str())?;
        let metadata = symbols.metadata(&anchor.anchor);
        Some(SourceSite {
            id: node_id.clone(),
            subject: node.subject.clone(),
            url: metadata
                .map(|item| item.url.clone())
                .unwrap_or_else(|| format!("{}#{}", symbols.fragment.base_url, anchor.anchor)),
            segment_id: None,
            span: None,
            step_text: metadata
                .map(|item| item.text.clone())
                .filter(|text| !text.is_empty()),
        })
    }

    fn intern_site(&mut self, site: SourceSite) -> String {
        let key = site_key(&site);
        self.sites.entry(key.clone()).or_insert(site);
        key
    }

    fn intern_issue(&mut self, issue: GraphIssue) -> IssueId {
        if let Some(&id) = self.issue_ids.get(&issue) {
            return id;
        }
        let id = IssueId::try_from(self.issue_catalog.len())
            .expect("issue catalog exceeded u32 address space");
        self.issue_catalog.push(issue.clone());
        self.issue_ids.insert(issue, id);
        id
    }

    fn add_issue(
        &mut self,
        subject: &str,
        code: IssueCode,
        message: impl Into<String>,
        site_key: Option<String>,
        order: u64,
    ) {
        let id = self.intern_issue(GraphIssue {
            code,
            message: message.into(),
            site_key,
        });
        self.node_issues
            .entry(subject.to_owned())
            .or_default()
            .push((order, id));
    }

    fn add_catalog_issue(
        &mut self,
        subject: &str,
        code: IssueCode,
        message: impl Into<String>,
        site: Option<SourceSite>,
    ) {
        let site_key = site.map(|site| self.intern_site(site));
        let order = source_order(CATALOG_PHASE, 0, self.catalog_order);
        self.add_issue(subject, code, message, site_key, order);
    }

    fn finish(self) -> Linked {
        let issues: BTreeMap<String, Vec<IssueId>> = self
            .node_issues
            .into_iter()
            .map(|(node, mut entries)| {
                entries.sort_by_key(|&(order, _)| order);
                let mut ids: Vec<IssueId> = Vec::with_capacity(entries.len());
                for (_, id) in entries {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                (node, ids)
            })
            .collect();
        let mut referenced: HashSet<&str> = HashSet::new();
        for edge in self.edges.values() {
            referenced.insert(&edge.site_key);
            referenced.extend(edge.context.iter().map(|c| c.site_key.as_str()));
        }
        referenced.extend(
            self.issue_catalog
                .iter()
                .filter_map(|issue| issue.site_key.as_deref()),
        );
        let sites: BTreeMap<String, SourceSite> = self
            .sites
            .iter()
            .filter(|(key, _)| referenced.contains(key.as_str()))
            .map(|(key, site)| (key.clone(), site.clone()))
            .collect();
        let mut spec_deps: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for edge in self
            .edges
            .values()
            .filter(|edge| edge.relation != Relationship::Mention)
        {
            let (Some(from), Some(to)) = (self.nodes.get(&edge.from), self.nodes.get(&edge.to))
            else {
                continue;
            };
            if from.subject.spec != to.subject.spec {
                *spec_deps
                    .entry(from.subject.spec.clone())
                    .or_default()
                    .entry(to.subject.spec.clone())
                    .or_default() += 1;
            }
        }
        let effect_categories = self
            .catalog
            .effects
            .iter()
            .map(|(kind, definition)| (kind.clone(), definition.category.clone()))
            .collect();
        let mut graph = Graph {
            engine_version: ANALYSIS_ENGINE_VERSION,
            environment: self.environment.to_owned(),
            catalog_digest: self.catalog.content_digest.clone(),
            nodes: self.nodes,
            anchor_nodes: self.anchor_nodes,
            edges: self.edges,
            occurrences: self.occurrences,
            issue_catalog: self.issue_catalog,
            issues,
            definitions: self.definitions,
            sites,
            effect_categories,
            edges_by_source: HashMap::new(),
            edges_by_target: HashMap::new(),
        };
        graph.index_edges();
        Linked {
            graph,
            missing_inputs: self
                .missing_inputs
                .into_iter()
                .map(|(spec, anchor)| MissingInput {
                    spec,
                    anchor: Some(anchor),
                })
                .collect(),
            spec_deps,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::effects::catalog::{load_catalog, load_package_files, Catalog};
    use crate::effects::engine::{build_graph, Graph, GraphInput, IndexedAnchor, SourceSpec};
    use crate::effects::fragment::{build_fragment, Fragment, FragmentInput};
    use crate::effects::model::MissingInput;
    use crate::parse::steps::{extract_step_structure, ReferenceRole};
    use std::collections::{BTreeMap, BTreeSet};

    pub(crate) fn input_for<'a>(
        source: &'a SourceSpec,
        catalog: &'a Catalog,
        environment: &'a str,
    ) -> FragmentInput<'a> {
        FragmentInput {
            spec: &source.spec,
            snapshot_sha: &source.snapshot_sha,
            base_url: &source.base_url,
            structure: source.structure.as_ref(),
            anchors: &source.anchors,
            catalog,
            environment,
        }
    }

    pub(crate) fn engine_fixture_catalog() -> Catalog {
        let package = load_package_files(&[(
            "catalog.yaml",
            include_str!("../../tests/fixtures/effects/engine/catalog.yaml"),
        )])
        .unwrap();
        load_catalog([package]).unwrap()
    }

    pub(crate) fn corpus(names: &[(&str, &str, &str)]) -> Vec<SourceSpec> {
        names
            .iter()
            .map(|(spec, base, html)| {
                let structure = extract_step_structure(html, spec, base, &format!("hash:{spec}"));
                let document = scraper::Html::parse_document(html);
                let parsed = crate::parse::parse_spec_document(&document, spec, base).unwrap();
                let mut anchors: Vec<String> = structure.anchors.clone();
                anchors.extend(parsed.sections.iter().map(|s| s.anchor.clone()));
                anchors.sort();
                anchors.dedup();
                SourceSpec {
                    spec: spec.to_string(),
                    snapshot_sha: format!("hash:{spec}"),
                    base_url: base.to_string(),
                    anchors: anchors
                        .into_iter()
                        .map(|a| IndexedAnchor {
                            url: format!("{base}#{a}"),
                            anchor: a,
                            text: String::new(),
                            idl_kind: None,
                        })
                        .collect(),
                    structure: Some(structure),
                }
            })
            .collect()
    }

    /// DOM, INFRA and URL; INFRA's `b-typed` algorithm is an IDL interface.
    pub(crate) fn multi_corpus() -> Vec<SourceSpec> {
        let mut sources = corpus(&[
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
            (
                "URL",
                "https://url.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/gamma.html"),
            ),
        ]);
        for anchor in &mut sources[1].anchors {
            if anchor.anchor == "b-typed" {
                anchor.idl_kind = Some("interface".into());
            }
        }
        sources
    }

    /// The graph without `source_order` and with issue numbering replaced by values.
    fn normalized(graph: &Graph) -> serde_json::Value {
        let mut value = serde_json::to_value(graph).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in ["nodes", "edges", "occurrences"] {
            for item in object[key].as_object_mut().unwrap().values_mut() {
                item.as_object_mut().unwrap().remove("source_order");
            }
        }
        let issues: serde_json::Map<String, serde_json::Value> = graph
            .issues
            .iter()
            .map(|(node, ids)| {
                let mut values: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        let issue = &graph.issue_catalog[*id as usize];
                        (
                            serde_json::to_string(&issue.code).unwrap(),
                            issue.message.clone(),
                            issue.site_key.clone(),
                        )
                    })
                    .collect();
                values.sort();
                (node.clone(), serde_json::to_value(values).unwrap())
            })
            .collect();
        serde_json::json!({
            "nodes": object["nodes"],
            "anchor_nodes": object["anchor_nodes"],
            "edges": object["edges"],
            "occurrences": object["occurrences"],
            "issues": issues,
            "definitions": object["definitions"],
            "sites": object["sites"],
            "effect_categories": object["effect_categories"],
        })
    }

    fn check_golden(name: &str, value: &serde_json::Value) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/effects/link")
            .join(format!("{name}.json"));
        let text = serde_json::to_string_pretty(value).unwrap();
        if std::env::var_os("WEBSPEC_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            return;
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "{name}: linked graph differs from the characterized build"
        );
    }

    #[test]
    fn linked_graph_matches_characterized_build() {
        let catalog = engine_fixture_catalog();
        let multi = multi_corpus();
        for environment in ["generic", "browser"] {
            let graph = build_graph(GraphInput {
                sources: &multi,
                catalog: &catalog,
                environment,
            })
            .unwrap();
            check_golden(&format!("multi-{environment}"), &normalized(&graph));
        }
        let single = corpus(&[(
            "TEST",
            "https://example.test/spec",
            include_str!("../../tests/fixtures/effects/engine/acceptance.html"),
        )]);
        for environment in ["generic", "web"] {
            let graph = build_graph(GraphInput {
                sources: &single,
                catalog: &catalog,
                environment,
            })
            .unwrap();
            check_golden(&format!("acceptance-{environment}"), &normalized(&graph));
        }
    }

    fn fragments(sources: &[SourceSpec], catalog: &Catalog, environment: &str) -> Vec<Fragment> {
        sources
            .iter()
            .map(|source| build_fragment(&input_for(source, catalog, environment)))
            .collect()
    }

    #[test]
    fn link_is_deterministic_and_order_independent() {
        let catalog = engine_fixture_catalog();
        let sources = multi_corpus();
        let mut fragments = fragments(&sources, &catalog, "browser");
        let first =
            serde_json::to_string(&link(&fragments, &catalog, "browser").unwrap().graph).unwrap();
        fragments.reverse();
        let second =
            serde_json::to_string(&link(&fragments, &catalog, "browser").unwrap().graph).unwrap();
        assert_eq!(first, second);
    }

    /// Non-Mention operation targets that are absent from the indexed anchor lists.
    fn unindexed_targets(sources: &[SourceSpec]) -> Vec<MissingInput> {
        let indexed: BTreeMap<&str, BTreeSet<&str>> = sources
            .iter()
            .map(|source| {
                (
                    source.spec.as_str(),
                    source.anchors.iter().map(|a| a.anchor.as_str()).collect(),
                )
            })
            .collect();
        let missing: BTreeSet<(String, String)> = sources
            .iter()
            .flat_map(|source| source.structure.iter().flat_map(|s| &s.algorithms))
            .flat_map(|algorithm| &algorithm.operation_sites)
            .filter(|operation| operation.role != ReferenceRole::Mention)
            .filter_map(|operation| operation.target.as_ref())
            .filter(|target| {
                !indexed
                    .get(target.spec.as_str())
                    .is_some_and(|anchors| anchors.contains(target.anchor.as_str()))
            })
            .map(|target| (target.spec.clone(), target.anchor.clone()))
            .collect();
        missing
            .into_iter()
            .map(|(spec, anchor)| MissingInput {
                spec,
                anchor: Some(anchor),
            })
            .collect()
    }

    #[test]
    fn link_reports_missing_inputs_and_cross_spec_dependencies() {
        let catalog = engine_fixture_catalog();
        let sources = multi_corpus();
        let generic = link(
            &fragments(&sources, &catalog, "generic"),
            &catalog,
            "generic",
        )
        .unwrap();
        assert_eq!(generic.missing_inputs, unindexed_targets(&sources));
        assert_eq!(
            generic
                .missing_inputs
                .iter()
                .map(|input| format!("{}#{}", input.spec, input.anchor.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            ["DOM#a-missing", "FETCH#x", "INFRA#b-missing"]
        );
        let deps = |linked: &Linked| {
            linked
                .spec_deps
                .iter()
                .flat_map(|(spec, deps)| {
                    deps.iter()
                        .map(move |(dep, edges)| format!("{spec}->{dep}:{edges}"))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            deps(&generic),
            ["DOM->INFRA:3", "DOM->URL:1", "INFRA->DOM:1", "INFRA->URL:1"]
        );
        let browser = link(
            &fragments(&sources, &catalog, "browser"),
            &catalog,
            "browser",
        )
        .unwrap();
        assert_eq!(
            deps(&browser),
            ["DOM->INFRA:4", "DOM->URL:1", "INFRA->DOM:1", "INFRA->URL:1"]
        );
    }

    #[test]
    fn link_rejects_an_empty_environment() {
        let catalog = engine_fixture_catalog();
        assert!(link(&[], &catalog, "").is_err());
    }
}
