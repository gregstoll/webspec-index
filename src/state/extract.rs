//! Statement sources (§7.1) from structural algorithms.
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
        for segment in &algorithm.segments {
            let step_path = segment.owner_step_id.as_ref().and_then(|id| {
                algorithm
                    .steps
                    .iter()
                    .find(|step| &step.source.node_id == id)
                    .map(|step| {
                        step.path
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    })
            });
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
