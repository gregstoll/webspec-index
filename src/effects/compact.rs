//! Whole-graph propagation on interned ids, and the summary rows published from it.
//!
//! The global publication has no discovery budgets. For every node, the propagated
//! states equal that node's states under an unbudgeted `Analysis`, whether the
//! analysis covers the whole graph or only the subject.

use super::engine::{
    category_order, compare_effect_summaries, handles_by_digest, params_dominate, Graph,
    LocalOccurrence,
};
use super::graph::{ExecutionEdge, ExecutionNode};
use super::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Serialize, Deserialize)]
pub(crate) struct CompactSummary {
    pub(crate) subject: Subject,
    pub(crate) effects: Vec<EffectSummary>,
    pub(crate) coverage: Coverage,
    pub(crate) issue_codes: Vec<IssueCode>,
    pub(crate) defined_bodies: Vec<CompactBody>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CompactBody {
    pub(crate) subject: Subject,
    pub(crate) effects: Vec<EffectSummary>,
    pub(crate) issue_codes: Vec<IssueCode>,
}

pub(crate) struct SummaryRow {
    pub spec: String,
    pub subject_key: String,
    pub payload: String,
}

pub(crate) struct Propagated {
    /// Interned node ids in `graph.nodes` order; the index is the node's `u32` id.
    pub(crate) ids: Vec<String>,
    /// Per node, sorted and deduplicated `occurrence_index << 2 | execution`, where
    /// the occurrence index follows `graph.occurrences` order.
    pub(crate) states: Vec<Vec<u64>>,
    /// Per node, one bit per [`IssueCode`] in declaration order.
    pub(crate) issue_mask: Vec<u16>,
}

fn failure(error: impl std::fmt::Display) -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: error.to_string(),
        details: None,
    }
}

pub(crate) fn selector_key(subject: &SubjectSelector) -> Result<String, RequestError> {
    canonical_json(
        &serde_json::to_value((
            &subject.spec,
            &subject.anchor,
            &subject.step_id,
            &subject.step_path,
            &subject.body_id,
        ))
        .map_err(failure)?,
    )
    .map_err(failure)
}

pub(crate) fn subject_selector(subject: &Subject) -> SubjectSelector {
    SubjectSelector {
        spec: subject.spec.clone(),
        anchor: subject.anchor.clone(),
        step_id: subject.step_id.clone(),
        step_path: subject.step_path.clone(),
        body_id: subject.body_id.clone(),
    }
}

const ISSUE_CODES: [IssueCode; 13] = [
    IssueCode::MissingSpec,
    IssueCode::MissingAnchor,
    IssueCode::UnsupportedStructure,
    IssueCode::UnresolvedInvocation,
    IssueCode::UnresolvedBodyBinding,
    IssueCode::UnresolvedArgument,
    IssueCode::AmbiguousMatch,
    IssueCode::DeclarationMismatch,
    IssueCode::AnalysisBudget,
    IssueCode::SnapshotChanged,
    IssueCode::UnsupportedPreview,
    IssueCode::WitnessBudget,
    IssueCode::ContextTruncated,
];

fn issue_bit(code: IssueCode) -> u16 {
    1 << code as u16
}

fn issue_codes(mask: u16) -> Vec<IssueCode> {
    ISSUE_CODES
        .into_iter()
        .filter(|code| mask & issue_bit(*code) != 0)
        .collect()
}

fn coverage(mask: u16) -> Coverage {
    let tolerated = issue_bit(IssueCode::WitnessBudget) | issue_bit(IssueCode::ContextTruncated);
    if mask & !tolerated != 0 {
        Coverage::Partial
    } else {
        Coverage::Complete
    }
}

fn execution_bits(execution: Execution) -> u64 {
    match execution {
        Execution::Inline => 0,
        Execution::Separate => 1,
        Execution::Unknown => 2,
    }
}

fn execution_from_bits(bits: u64) -> Execution {
    match bits & 3 {
        0 => Execution::Inline,
        1 => Execution::Separate,
        _ => Execution::Unknown,
    }
}

/// Non-Mention edges between known nodes, as CSR adjacency from parent to child.
struct Topology<'g> {
    index: HashMap<&'g str, u32>,
    /// `(from, to, edge)` in `graph.edges` order.
    edges: Vec<(u32, u32, &'g ExecutionEdge)>,
    offsets: Vec<usize>,
    /// Edge positions grouped by parent.
    outgoing: Vec<u32>,
}

impl<'g> Topology<'g> {
    fn new(graph: &'g Graph) -> Self {
        let index: HashMap<&str, u32> = graph
            .nodes
            .keys()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i as u32))
            .collect();
        let edges: Vec<_> = graph
            .edges
            .values()
            .filter(|edge| edge.relation != Relationship::Mention)
            .filter_map(|edge| {
                Some((
                    *index.get(edge.from.as_str())?,
                    *index.get(edge.to.as_str())?,
                    edge,
                ))
            })
            .collect();
        let mut offsets = vec![0usize; graph.nodes.len() + 1];
        for &(from, _, _) in &edges {
            offsets[from as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut fill = offsets.clone();
        let mut outgoing = vec![0u32; edges.len()];
        for (position, &(from, _, _)) in edges.iter().enumerate() {
            outgoing[fill[from as usize]] = position as u32;
            fill[from as usize] += 1;
        }
        Self {
            index,
            edges,
            offsets,
            outgoing,
        }
    }

    fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    fn outgoing(&self, node: u32) -> &[u32] {
        &self.outgoing[self.offsets[node as usize]..self.offsets[node as usize + 1]]
    }

    fn reach(&self, roots: impl IntoIterator<Item = u32>) -> Vec<bool> {
        let mut reached = vec![false; self.len()];
        let mut stack: Vec<u32> = Vec::new();
        for root in roots {
            if !reached[root as usize] {
                reached[root as usize] = true;
                stack.push(root);
            }
        }
        while let Some(node) = stack.pop() {
            for &position in self.outgoing(node) {
                let child = self.edges[position as usize].1;
                if !reached[child as usize] {
                    reached[child as usize] = true;
                    stack.push(child);
                }
            }
        }
        reached
    }

    /// Tarjan's strongly connected components, children before parents.
    fn components(&self) -> Vec<Vec<u32>> {
        const UNSEEN: u32 = u32::MAX;
        let mut order = vec![UNSEEN; self.len()];
        let mut low = vec![0u32; self.len()];
        let mut on_stack = vec![false; self.len()];
        let mut stack: Vec<u32> = Vec::new();
        let mut frames: Vec<(u32, usize)> = Vec::new();
        let mut next = 0u32;
        let mut components = Vec::new();
        for root in 0..self.len() as u32 {
            if order[root as usize] != UNSEEN {
                continue;
            }
            order[root as usize] = next;
            low[root as usize] = next;
            next += 1;
            stack.push(root);
            on_stack[root as usize] = true;
            frames.push((root, 0));
            while let Some(frame) = frames.last_mut() {
                let node = frame.0;
                let outgoing = self.outgoing(node);
                if frame.1 < outgoing.len() {
                    let child = self.edges[outgoing[frame.1] as usize].1;
                    frame.1 += 1;
                    if order[child as usize] == UNSEEN {
                        order[child as usize] = next;
                        low[child as usize] = next;
                        next += 1;
                        stack.push(child);
                        on_stack[child as usize] = true;
                        frames.push((child, 0));
                    } else if on_stack[child as usize] {
                        low[node as usize] = low[node as usize].min(order[child as usize]);
                    }
                    continue;
                }
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    low[parent as usize] = low[parent as usize].min(low[node as usize]);
                }
                if low[node as usize] == order[node as usize] {
                    let mut component = Vec::new();
                    loop {
                        let member = stack.pop().expect("Tarjan stack holds the component");
                        on_stack[member as usize] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    components.push(component);
                }
            }
        }
        components
    }
}

/// Whether two occurrences come from at least one common rule.
fn share_rule(left: &LocalOccurrence, right: &LocalOccurrence) -> bool {
    left.evidence
        .iter()
        .any(|l| right.evidence.iter().any(|r| l.rule_id == r.rule_id))
}

/// Per edge, the sorted occurrences of its child that the parent's own occurrence at
/// the same site specializes, so they do not propagate across the edge.
fn blocked_origins(
    topology: &Topology<'_>,
    occurrences: &[&LocalOccurrence],
    local: &[Vec<u32>],
) -> Vec<Vec<u32>> {
    let mut by_site: HashMap<&str, Vec<u32>> = HashMap::new();
    for (index, occurrence) in occurrences.iter().enumerate() {
        for evidence in &occurrence.evidence {
            by_site
                .entry(evidence.site.id.as_str())
                .or_default()
                .push(index as u32);
        }
    }
    topology
        .edges
        .iter()
        .map(|&(_, to, edge)| {
            let Some(candidates) = by_site.get(edge.site_id.as_str()) else {
                return Vec::new();
            };
            local[to as usize]
                .iter()
                .copied()
                .filter(|&origin| {
                    let origin = occurrences[origin as usize];
                    candidates.iter().any(|&candidate| {
                        let candidate = occurrences[candidate as usize];
                        candidate.subject_id == edge.from
                            && candidate.kind == origin.kind
                            && share_rule(candidate, origin)
                            && (candidate.params == origin.params
                                || params_dominate(&candidate.params, &origin.params))
                    })
                })
                .collect()
        })
        .collect()
}

/// Propagate every occurrence and issue code to all its ancestors over non-Mention
/// edges, composing execution and dropping same-site specialized origins.
pub(crate) fn propagate_compact(graph: &Graph) -> Propagated {
    let topology = Topology::new(graph);
    let occurrences: Vec<&LocalOccurrence> = graph.occurrences.values().collect();
    let mut local: Vec<Vec<u32>> = vec![Vec::new(); topology.len()];
    for (index, occurrence) in occurrences.iter().enumerate() {
        if let Some(&node) = topology.index.get(occurrence.subject_id.as_str()) {
            local[node as usize].push(index as u32);
        }
    }
    let blocked = blocked_origins(&topology, &occurrences, &local);
    let mut states: Vec<Vec<u64>> = local
        .iter()
        .map(|origins| {
            origins
                .iter()
                .map(|&origin| u64::from(origin) << 2 | execution_bits(Execution::Inline))
                .collect()
        })
        .collect();
    let mut issue_mask = vec![0u16; topology.len()];
    for (node, ids) in &graph.issues {
        if let Some(&node) = topology.index.get(node.as_str()) {
            for &id in ids {
                if let Some(issue) = graph.issue_catalog.get(id as usize) {
                    issue_mask[node as usize] |= issue_bit(issue.code);
                }
            }
        }
    }
    for component in topology.components() {
        let cyclic = component.len() > 1
            || topology
                .outgoing(component[0])
                .iter()
                .any(|&position| topology.edges[position as usize].1 == component[0]);
        loop {
            let mut changed = false;
            for &node in &component {
                let mut derived = states[node as usize].clone();
                let mut mask = issue_mask[node as usize];
                for &position in topology.outgoing(node) {
                    let (_, child, edge) = topology.edges[position as usize];
                    let blocked = &blocked[position as usize];
                    mask |= issue_mask[child as usize];
                    for &state in &states[child as usize] {
                        let origin = state >> 2;
                        if blocked.binary_search(&(origin as u32)).is_ok() {
                            continue;
                        }
                        let execution = edge.execution.compose(execution_from_bits(state));
                        derived.push(origin << 2 | execution_bits(execution));
                    }
                }
                derived.sort_unstable();
                derived.dedup();
                // Derivation only adds states, so a longer set is a changed one.
                if derived.len() != states[node as usize].len() {
                    states[node as usize] = derived;
                    changed = true;
                }
                if mask != issue_mask[node as usize] {
                    issue_mask[node as usize] = mask;
                    changed = true;
                }
            }
            if !cyclic || !changed {
                break;
            }
        }
    }
    Propagated {
        ids: graph.nodes.keys().cloned().collect(),
        states,
        issue_mask,
    }
}

type SubjectKey<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a [u32]>,
    Option<&'a str>,
);

fn subject_key(subject: &Subject) -> SubjectKey<'_> {
    (
        subject.spec.as_str(),
        subject.anchor.as_str(),
        subject.snapshot_sha.as_str(),
        subject.step_id.as_deref(),
        subject.step_path.as_deref(),
        subject.body_id.as_deref(),
    )
}

struct Effect<'g> {
    kind: &'g str,
    params: &'g EffectParams,
    handle: String,
}

struct RowBuilder<'g> {
    graph: &'g Graph,
    propagated: &'g Propagated,
    topology: Topology<'g>,
    nodes: Vec<&'g ExecutionNode>,
    occurrences: Vec<&'g LocalOccurrence>,
    /// Index into `effects` per occurrence; occurrences outside the graph never occur in states.
    occurrence_effect: Vec<u32>,
    effects: Vec<Effect<'g>>,
}

impl<'g> RowBuilder<'g> {
    fn new(graph: &'g Graph, propagated: &'g Propagated) -> Result<Self, RequestError> {
        let topology = Topology::new(graph);
        let occurrences: Vec<&LocalOccurrence> = graph.occurrences.values().collect();
        let mut interned: BTreeMap<(&str, &EffectParams), u32> = BTreeMap::new();
        let mut occurrence_effect = vec![u32::MAX; occurrences.len()];
        for (index, occurrence) in occurrences.iter().enumerate() {
            if topology.index.contains_key(occurrence.subject_id.as_str()) {
                let next = interned.len() as u32;
                occurrence_effect[index] = *interned
                    .entry((occurrence.kind.as_str(), &occurrence.params))
                    .or_insert(next);
            }
        }
        let mut digests = vec![String::new(); interned.len()];
        for (&(kind, params), &effect) in &interned {
            digests[effect as usize] = effect_digest(kind, params).map_err(failure)?;
        }
        let handles = handles_by_digest(digests.iter().cloned().collect());
        let mut effects: Vec<Option<Effect<'g>>> = (0..interned.len()).map(|_| None).collect();
        for ((kind, params), effect) in interned {
            effects[effect as usize] = Some(Effect {
                kind,
                params,
                handle: handles[&digests[effect as usize]].clone(),
            });
        }
        Ok(Self {
            graph,
            propagated,
            topology,
            nodes: graph.nodes.values().collect(),
            occurrences,
            occurrence_effect,
            effects: effects.into_iter().map(Option::unwrap).collect(),
        })
    }

    /// Nodes of the unbudgeted whole-graph selection: the analysis roots and
    /// everything they reach.
    fn all_selection(&self) -> Vec<bool> {
        let mut root = vec![false; self.nodes.len()];
        for occurrence in &self.occurrences {
            if let Some(&node) = self.topology.index.get(occurrence.subject_id.as_str()) {
                root[node as usize] = true;
            }
        }
        for edge in self.graph.edges.values() {
            if edge.relation == Relationship::Implements {
                if let Some(&node) = self.topology.index.get(edge.from.as_str()) {
                    root[node as usize] = true;
                }
            }
        }
        for (flag, node) in root.iter_mut().zip(&self.nodes) {
            *flag |=
                node.is_body && node.subject.body_id.is_none() && !node.id.starts_with("anchor:");
        }
        self.topology.reach(
            root.iter()
                .enumerate()
                .filter(|(_, root)| **root)
                .map(|(node, _)| node as u32),
        )
    }

    fn processed(&self, selected: &[bool]) -> HashSet<SubjectKey<'g>> {
        self.nodes
            .iter()
            .zip(selected)
            .filter(|(_, selected)| **selected)
            .map(|(node, _)| subject_key(&node.subject))
            .collect()
    }

    fn effects(&self, node: u32) -> Vec<EffectSummary> {
        let mut groups: BTreeMap<u32, (BTreeSet<Execution>, BTreeSet<u32>)> = BTreeMap::new();
        for &state in &self.propagated.states[node as usize] {
            let origin = (state >> 2) as usize;
            let group = groups.entry(self.occurrence_effect[origin]).or_default();
            group.0.insert(execution_from_bits(state));
            group.1.insert(origin as u32);
        }
        let mut effects: Vec<EffectSummary> = groups
            .into_iter()
            .map(|(effect, (executions, origins))| {
                let effect = &self.effects[effect as usize];
                let locations: BTreeSet<_> = origins
                    .iter()
                    .flat_map(|&origin| &self.occurrences[origin as usize].evidence)
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
                EffectSummary {
                    id: effect.handle.clone(),
                    kind: effect.kind.to_owned(),
                    params: effect.params.clone(),
                    execution: executions.into_iter().collect(),
                    location,
                    other_locations: locations.take(MAX_SUMMARY_LOCATIONS - 1).collect(),
                    additional_locations,
                }
            })
            .collect();
        let category =
            |kind: &str| category_order(self.graph.effect_categories.get(kind).map(String::as_str));
        effects.sort_by(|left, right| {
            category(&left.kind)
                .cmp(&category(&right.kind))
                .then_with(|| compare_effect_summaries(left, right))
        });
        effects
    }

    /// The row of `node`, whose defined bodies are those `processed` by its analysis.
    fn row(
        &self,
        node: u32,
        processed: &HashSet<SubjectKey<'_>>,
    ) -> Result<SummaryRow, RequestError> {
        let subject = &self.nodes[node as usize].subject;
        let mask = self.propagated.issue_mask[node as usize];
        let defined_bodies = self
            .graph
            .definitions
            .get(&self.propagated.ids[node as usize])
            .into_iter()
            .flatten()
            .filter_map(|body| self.topology.index.get(body.as_str()).copied())
            .filter(|&body| processed.contains(&subject_key(&self.nodes[body as usize].subject)))
            .map(|body| CompactBody {
                subject: self.nodes[body as usize].subject.clone(),
                effects: self.effects(body),
                issue_codes: issue_codes(self.propagated.issue_mask[body as usize]),
            })
            .collect();
        let summary = CompactSummary {
            subject: subject.clone(),
            effects: self.effects(node),
            coverage: coverage(mask),
            issue_codes: issue_codes(mask),
            defined_bodies,
        };
        Ok(SummaryRow {
            spec: subject.spec.clone(),
            subject_key: selector_key(&subject_selector(subject))?,
            payload: serde_json::to_string(&summary).map_err(failure)?,
        })
    }
}

/// Summary rows for the unbudgeted whole-graph selection plus every anchor node;
/// an anchor outside that selection gets the summary of its own analysis.
pub(crate) fn summary_rows(
    graph: &Graph,
    propagated: &Propagated,
) -> Result<Vec<SummaryRow>, RequestError> {
    let builder = RowBuilder::new(graph, propagated)?;
    let selected = builder.all_selection();
    let processed = builder.processed(&selected);
    let mut rows = Vec::new();
    for (node, entry) in builder.nodes.iter().enumerate() {
        if processed.contains(&subject_key(&entry.subject)) {
            rows.push(builder.row(node as u32, &processed)?);
        }
    }
    let mut keys: HashSet<String> = rows.iter().map(|row| row.subject_key.clone()).collect();
    for ((spec, anchor), id) in &graph.anchor_nodes {
        let Some(&node) = builder.topology.index.get(id.as_str()) else {
            continue;
        };
        let key = selector_key(&SubjectSelector {
            spec: spec.clone(),
            anchor: anchor.clone(),
            step_id: None,
            step_path: None,
            body_id: None,
        })?;
        if keys.contains(&key) {
            continue;
        }
        let scoped = if graph.definitions.contains_key(id) {
            builder.processed(&builder.topology.reach([node]))
        } else {
            HashSet::new()
        };
        let row = builder.row(node, &scoped)?;
        keys.insert(row.subject_key.clone());
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::engine::{
        analyze_graph, build_graph, AnalysisArtifact, ArtifactSummaryRecord, GraphInput, IssueId,
    };
    use crate::effects::link::tests::{corpus, engine_fixture_catalog, multi_corpus};
    use std::collections::{BTreeMap, BTreeSet};

    fn unbounded() -> DiscoveryBudgets {
        DiscoveryBudgets {
            max_bodies: u64::MAX / 4,
            max_relationships: u64::MAX / 4,
            max_states: u64::MAX / 4,
        }
    }

    fn fixture_graphs() -> Vec<Graph> {
        let catalog = engine_fixture_catalog();
        let multi = multi_corpus();
        let acceptance = corpus(&[(
            "TEST",
            "https://example.test/spec",
            include_str!("../../tests/fixtures/effects/engine/acceptance.html"),
        )]);
        let mut inputs = vec![
            (multi.clone(), "generic"),
            (multi, "browser"),
            (acceptance.clone(), "generic"),
            (acceptance, "web"),
        ];
        for html in [
            include_str!("../../tests/fixtures/effects/structure/wattsi.html"),
            include_str!("../../tests/fixtures/effects/structure/bikeshed.html"),
            include_str!("../../tests/fixtures/effects/structure/ecmarkup.html"),
            include_str!("../../tests/fixtures/effects/structure/conditional_remainder.html"),
        ] {
            inputs.push((
                corpus(&[("TEST", "https://test.example/spec", html)]),
                "browser",
            ));
        }
        inputs
            .into_iter()
            .map(|(sources, environment)| {
                build_graph(GraphInput {
                    sources: &sources,
                    catalog: &catalog,
                    environment,
                })
                .unwrap()
            })
            .collect()
    }

    fn codes_for_ids(artifact: &AnalysisArtifact, ids: &[IssueId]) -> Vec<IssueCode> {
        ids.iter()
            .filter_map(|id| artifact.issue(*id).map(|issue| issue.code))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn compact_row(artifact: &AnalysisArtifact, record: ArtifactSummaryRecord) -> (String, String) {
        let compact = CompactSummary {
            subject: record.subject.clone(),
            effects: record.effects,
            coverage: record.coverage,
            issue_codes: codes_for_ids(artifact, &record.issue_ids),
            defined_bodies: record
                .defined_bodies
                .into_iter()
                .map(|body| CompactBody {
                    subject: body.subject,
                    effects: body.effects,
                    issue_codes: codes_for_ids(artifact, &body.issue_ids),
                })
                .collect(),
        };
        (
            selector_key(&subject_selector(&compact.subject)).unwrap(),
            serde_json::to_string(&compact).unwrap(),
        )
    }

    fn anchor_selector(spec: &str, anchor: &str) -> SubjectSelector {
        SubjectSelector {
            spec: spec.to_owned(),
            anchor: anchor.to_owned(),
            step_id: None,
            step_path: None,
            body_id: None,
        }
    }

    fn reference_rows(graph: &Graph) -> BTreeMap<String, String> {
        let artifact = analyze_graph(graph, AnalysisScope::All, unbounded()).unwrap();
        let mut rows: BTreeMap<String, String> = artifact
            .summary_records(None)
            .unwrap()
            .into_iter()
            .map(|record| compact_row(&artifact, record))
            .collect();
        for (spec, anchor) in graph.anchor_nodes.keys() {
            let selector = anchor_selector(spec, anchor);
            let key = selector_key(&selector).unwrap();
            if rows.contains_key(&key) {
                continue;
            }
            let scoped = analyze_graph(
                graph,
                AnalysisScope::Subject {
                    subject: selector.clone(),
                },
                unbounded(),
            )
            .unwrap();
            let record = scoped
                .summary_records(None)
                .unwrap()
                .into_iter()
                .find(|r| {
                    r.subject.anchor == *anchor
                        && r.subject.step_id.is_none()
                        && r.subject.body_id.is_none()
                })
                .unwrap();
            let (key2, payload) = compact_row(&scoped, record);
            assert_eq!(key, key2);
            rows.insert(key, payload);
        }
        rows
    }

    #[test]
    fn compact_rows_equal_the_reference_analysis() {
        for graph in fixture_graphs() {
            let propagated = propagate_compact(&graph);
            let rows: BTreeMap<String, String> = summary_rows(&graph, &propagated)
                .unwrap()
                .into_iter()
                .map(|row| (row.subject_key, row.payload))
                .collect();
            assert_eq!(rows, reference_rows(&graph));
        }
    }

    #[test]
    fn former_null_anchors_get_real_rows() {
        let graph = fixture_graphs().into_iter().next().unwrap();
        let reference = analyze_graph(&graph, AnalysisScope::All, unbounded()).unwrap();
        let all_keys: BTreeSet<String> = reference
            .summary_records(None)
            .unwrap()
            .iter()
            .map(|record| selector_key(&subject_selector(&record.subject)).unwrap())
            .collect();
        let rows = summary_rows(&graph, &propagate_compact(&graph)).unwrap();
        let keys: BTreeSet<_> = rows.iter().map(|r| r.subject_key.clone()).collect();
        let mut former_null = 0;
        for (spec, anchor) in graph.anchor_nodes.keys() {
            let key = selector_key(&anchor_selector(spec, anchor)).unwrap();
            assert!(keys.contains(&key), "{spec}#{anchor} has no row");
            former_null += usize::from(!all_keys.contains(&key));
        }
        assert!(
            former_null > 0,
            "the fixture has no anchor outside the All selection"
        );
        assert!(rows.iter().all(|r| r.payload != "null"));
    }

    #[test]
    fn propagated_states_are_sorted_and_deduplicated() {
        for graph in fixture_graphs() {
            let propagated = propagate_compact(&graph);
            assert_eq!(
                propagated.ids,
                graph.nodes.keys().cloned().collect::<Vec<_>>()
            );
            assert_eq!(propagated.states.len(), propagated.ids.len());
            assert_eq!(propagated.issue_mask.len(), propagated.ids.len());
            for states in &propagated.states {
                assert!(states.windows(2).all(|pair| pair[0] < pair[1]));
            }
        }
    }

    #[test]
    fn issue_code_bits_follow_declaration_order() {
        for (bit, code) in ISSUE_CODES.iter().enumerate() {
            assert_eq!(*code as usize, bit);
        }
        assert!(ISSUE_CODES.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
