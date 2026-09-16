//! Additive effects adapter for ordinary section queries.
//!
//! The existing `query_section` API deliberately remains effects-disabled. CLI and
//! other opt-in consumers use this wrapper so a semantic-analysis failure never
//! discards successfully retrieved specification text.

use serde::Serialize;

#[cfg(feature = "native")]
use crate::model::PrOpts;
use crate::model::QueryResult;

use super::model::{
    EffectSummary, EffectSummaryResult, EffectsStatus, IssueCode, Semantics, Subject,
    EFFECTS_SCHEMA_VERSION,
};
#[cfg(feature = "native")]
use super::model::{EffectsMode, EffectsOptions, EffectsRequest, SubjectSelector};

pub const DEFAULT_QUERY_EFFECT_LIMIT: usize = 12;

/// The original query wire shape plus the optional effects extension.
#[derive(Debug, Clone, Serialize)]
pub struct QueryWithEffects {
    #[serde(flatten)]
    pub query: QueryResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<EffectSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects_status: Option<EffectsStatus>,
}

impl QueryWithEffects {
    pub fn effects_result(&self) -> Option<EffectSummaryResult> {
        Some(EffectSummaryResult {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: Subject {
                spec: self.query.spec.clone(),
                anchor: self.query.anchor.clone(),
                snapshot_sha: self.query.sha.clone(),
                step_id: None,
                step_path: None,
                body_id: None,
            },
            effects: self.effects.clone()?,
            effects_status: self.effects_status.clone()?,
            defined_bodies: Vec::new(),
            issues: Vec::new(),
            input_manifest: None,
        })
    }
}

fn empty_status(state: fn(Semantics, Vec<IssueCode>, u64) -> EffectsStatus) -> EffectsStatus {
    state(Semantics::May, Vec::new(), 0)
}

pub(crate) fn error_status() -> EffectsStatus {
    empty_status(|semantics, issues, omitted| EffectsStatus::Error {
        semantics,
        issues,
        omitted,
    })
}

#[cfg(feature = "native")]
fn preview_status() -> EffectsStatus {
    EffectsStatus::Unavailable {
        semantics: Semantics::May,
        issues: vec![IssueCode::UnsupportedPreview],
        omitted: 0,
    }
}

fn snapshot_changed_status() -> EffectsStatus {
    EffectsStatus::Unavailable {
        semantics: Semantics::May,
        issues: vec![IssueCode::SnapshotChanged],
        omitted: 0,
    }
}

fn truncate_summary(mut result: EffectSummaryResult, limit: usize) -> EffectSummaryResult {
    if result.effects.len() <= limit {
        return result;
    }
    let newly_omitted = (result.effects.len() - limit) as u64;
    result.effects.truncate(limit);
    if let EffectsStatus::Ready { omitted, .. } = &mut result.effects_status {
        *omitted += newly_omitted;
    }
    result
}

/// The short effects preview shared by `query` and `effects --compact`.
pub fn compact_summary(result: EffectSummaryResult) -> EffectSummaryResult {
    let mut result = truncate_summary(result, DEFAULT_QUERY_EFFECT_LIMIT);
    result.defined_bodies.clear();
    result.issues.clear();
    result.input_manifest = None;
    result
}

pub fn attach_summary(query: QueryResult, result: EffectSummaryResult) -> QueryWithEffects {
    if result.subject.snapshot_sha != query.sha {
        return QueryWithEffects {
            query,
            effects: Some(Vec::new()),
            effects_status: Some(snapshot_changed_status()),
        };
    }
    let result = compact_summary(result);
    QueryWithEffects {
        query,
        effects: Some(result.effects),
        effects_status: Some(result.effects_status),
    }
}

/// Query section content and optionally attach a compact possible-effects summary.
#[cfg(feature = "native")]
pub async fn query_section_with_effects(
    spec_anchor: &str,
    pr: Option<&PrOpts>,
    options: EffectsOptions,
) -> anyhow::Result<QueryWithEffects> {
    let query = crate::query_section(spec_anchor, pr).await?;
    if options.mode == EffectsMode::Off {
        return Ok(QueryWithEffects {
            query,
            effects: None,
            effects_status: None,
        });
    }

    if pr.is_some() {
        return Ok(QueryWithEffects {
            query,
            effects: Some(Vec::new()),
            effects_status: Some(preview_status()),
        });
    }

    let request = EffectsRequest {
        schema_version: EFFECTS_SCHEMA_VERSION,
        subject: SubjectSelector {
            spec: query.spec.clone(),
            anchor: query.anchor.clone(),
            step_path: None,
            step_id: None,
            body_id: None,
        },
        options,
        filter: None,
    };

    match super::get_effect_summary(&request) {
        Ok(result) => Ok(attach_summary(query, result)),
        Err(_) => Ok(QueryWithEffects {
            query,
            effects: Some(Vec::new()),
            effects_status: Some(error_status()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::Coverage;
    use super::*;
    use crate::model::Navigation;

    fn query() -> QueryResult {
        QueryResult {
            spec: "TEST".into(),
            sha: "abc".into(),
            anchor: "algorithm".into(),
            url: "https://example.test/#algorithm".into(),
            title: None,
            number: None,
            section_type: "Algorithm".into(),
            content: Some("Do the thing.".into()),
            navigation: Navigation {
                parent: None,
                prev: None,
                next: None,
                children: Vec::new(),
            },
            outgoing_refs: Vec::new(),
            incoming_refs: Vec::new(),
        }
    }

    #[test]
    fn disabled_extension_preserves_existing_json_shape() {
        let wrapped = QueryWithEffects {
            query: query(),
            effects: None,
            effects_status: None,
        };
        let wrapped = serde_json::to_value(wrapped).unwrap();
        let original = serde_json::to_value(query()).unwrap();
        assert_eq!(wrapped, original);
    }

    #[test]
    fn query_limit_updates_ready_omission_count() {
        let result = EffectSummaryResult {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: Subject {
                spec: "TEST".into(),
                anchor: "algorithm".into(),
                snapshot_sha: "abc".into(),
                step_id: None,
                step_path: None,
                body_id: None,
            },
            effects: (0..14)
                .map(|i| EffectSummary {
                    id: format!("ef_{i:016x}"),
                    kind: "test.effect".into(),
                    params: Default::default(),
                    execution: vec![super::super::model::Execution::Inline],
                    location: None,
                    other_locations: Vec::new(),
                    additional_locations: 0,
                })
                .collect(),
            effects_status: EffectsStatus::ready(
                Coverage::Complete,
                Vec::new(),
                2,
                "an_test".into(),
            ),
            defined_bodies: Vec::new(),
            issues: Vec::new(),
            input_manifest: None,
        };
        let result = compact_summary(result);
        assert_eq!(result.effects.len(), DEFAULT_QUERY_EFFECT_LIMIT);
        assert!(matches!(
            result.effects_status,
            EffectsStatus::Ready { omitted: 4, .. }
        ));
    }

    #[test]
    fn snapshot_change_has_no_stale_effects() {
        let result = EffectSummaryResult {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: Subject {
                spec: "TEST".into(),
                anchor: "algorithm".into(),
                snapshot_sha: "newer".into(),
                step_id: None,
                step_path: None,
                body_id: None,
            },
            effects: vec![EffectSummary {
                id: "ef_0123456789abcdef".into(),
                kind: "test.effect".into(),
                params: Default::default(),
                execution: vec![super::super::model::Execution::Inline],
                location: None,
                other_locations: Vec::new(),
                additional_locations: 0,
            }],
            effects_status: EffectsStatus::ready(
                Coverage::Complete,
                Vec::new(),
                0,
                "an_test".into(),
            ),
            defined_bodies: Vec::new(),
            issues: Vec::new(),
            input_manifest: None,
        };
        let wrapped = attach_summary(query(), result);
        assert!(wrapped.effects.as_ref().unwrap().is_empty());
        assert!(matches!(
            wrapped.effects_status,
            Some(EffectsStatus::Unavailable { ref issues, .. })
                if issues == &[IssueCode::SnapshotChanged]
        ));
    }
}
