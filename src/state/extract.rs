//! Statement sources (§7.1) from structural algorithms.
use std::collections::BTreeMap;

use crate::parse::steps::{AnchorTarget, StructuralSpec};
use crate::state::ir::{SourceContext, StatementSource};

/// One source per structural segment, in algorithm and segment order.
#[allow(dead_code)]
pub(crate) fn algorithm_sources(structure: &StructuralSpec) -> Vec<StatementSource> {
    let mut sources = Vec::new();
    for algorithm in &structure.algorithms {
        let subject = AnchorTarget {
            spec: structure.spec.clone(),
            anchor: algorithm.source.section_anchor.clone(),
        };
        let step_paths: BTreeMap<&str, String> = algorithm
            .steps
            .iter()
            .map(|step| {
                let path = step
                    .path
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(".");
                (step.source.node_id.as_str(), path)
            })
            .collect();
        for segment in &algorithm.segments {
            let step_path = segment
                .owner_step_id
                .as_deref()
                .and_then(|id| step_paths.get(id).cloned());
            sources.push(StatementSource {
                id: segment.source.node_id.clone(),
                subject: subject.clone(),
                context: SourceContext::Algorithm {
                    segment_id: segment.source.node_id.clone(),
                    step_id: segment.owner_step_id.clone(),
                    step_path,
                    body_id: segment.owner_body_id.clone(),
                },
                text: segment.text.clone(),
                tokens: segment.tokens.clone(),
                links: segment.links.clone(),
            });
        }
    }
    sources
}
