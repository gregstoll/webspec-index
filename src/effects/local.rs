//! Source-local matches, independent of reachability and caller bindings.
use super::{catalog::Catalog, engine::SourceSpec, matcher, model::*};
use crate::parse::steps::ReferenceRole;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OperationMatches {
    pub effects: Vec<matcher::MatchedEffect>,
    pub continuations: BTreeSet<Execution>,
    #[serde(default)]
    pub issues: BTreeSet<IssueCode>,
}

pub(crate) type LocalMatches = BTreeMap<String, OperationMatches>;

pub(crate) fn segment_key(segment_id: &str) -> String {
    format!("segment:{segment_id}")
}

pub(crate) fn input_key(source: &SourceSpec, catalog: &Catalog, environment: &str) -> String {
    // Include the complete immutable structural input, not just the advertised SHA.
    // This also invalidates matches when parser or registry output changes.
    digest_serializable(&serde_json::json!({
        "version": super::engine::ANALYSIS_ENGINE_VERSION,
        "source": source, "catalog": catalog.content_digest, "environment": environment
    }))
    .expect("serializable local analysis inputs")
}

pub(crate) fn prepare(source: &SourceSpec, catalog: &Catalog) -> LocalMatches {
    let mut result = BTreeMap::new();
    for algorithm in source
        .structure
        .iter()
        .flat_map(|structure| &structure.algorithms)
    {
        let segments: BTreeMap<_, _> = algorithm
            .segments
            .iter()
            .map(|segment| (segment.source.node_id.as_str(), segment))
            .collect();
        for segment in &algorithm.segments {
            let effects = matcher::match_segment(catalog, algorithm, segment);
            if !effects.is_empty() {
                result.insert(
                    segment_key(&segment.source.node_id),
                    OperationMatches {
                        effects,
                        continuations: BTreeSet::new(),
                        issues: BTreeSet::new(),
                    },
                );
            }
        }
        for operation in &algorithm.operation_sites {
            if operation.role == ReferenceRole::Mention {
                continue;
            }
            let Some(segment) = segments.get(operation.segment_id.as_str()) else {
                continue;
            };
            let matched =
                matcher::match_operation_with_issues(catalog, algorithm, segment, operation);
            result.insert(
                operation.source.node_id.clone(),
                OperationMatches {
                    effects: matched.effects,
                    continuations: matcher::continuation_modes(
                        catalog, algorithm, segment, operation,
                    ),
                    issues: matched.issues,
                },
            );
        }
    }
    result
}
