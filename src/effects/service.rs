//! Snapshot-consistent analysis orchestration, persisted summaries, and cache policy.
use super::bundled::default_catalog;
use super::catalog::Catalog;
use super::engine::{
    self, AnalysisArtifact, AnalysisInput, ArtifactSummary, IndexedAnchor, SourceSpec,
};
use super::model::*;
#[cfg(any(feature = "native", test))]
use crate::db;
use crate::db::effects as storage;
use crate::parse::steps::{StructuralSpec, STRUCTURE_VERSION};
use rusqlite::{Connection, OptionalExtension};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn failure(error: impl std::fmt::Display) -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: error.to_string(),
        details: None,
    }
}

fn unavailable(message: impl Into<String>) -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisUnavailable,
        message: message.into(),
        details: None,
    }
}

fn selector(subject: &Subject) -> SubjectSelector {
    SubjectSelector {
        spec: subject.spec.clone(),
        anchor: subject.anchor.clone(),
        step_id: subject.step_id.clone(),
        step_path: None,
        body_id: if subject.step_id.is_none() {
            subject.body_id.clone()
        } else {
            None
        },
    }
}

fn key(subject: &SubjectSelector) -> Result<String, RequestError> {
    canonical_json(&serde_json::to_value(subject).map_err(failure)?).map_err(failure)
}

fn owning_scope(subject: &SubjectSelector) -> AnalysisScope {
    AnalysisScope::Subject {
        subject: SubjectSelector {
            spec: subject.spec.clone(),
            anchor: subject.anchor.clone(),
            step_path: None,
            step_id: None,
            body_id: None,
        },
    }
}

fn resolve_subject(conn: &Connection, selected: &SubjectSelector) -> Result<Subject, RequestError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT sp.name,sn.sha FROM specs sp JOIN snapshots sn ON sn.spec_id=sp.id
         WHERE lower(sp.name)=lower(?1) AND sn.pr_number IS NULL AND sn.sha LIKE 'hash:%'
         AND (EXISTS(SELECT 1 FROM sections WHERE snapshot_id=sn.id AND anchor=?2)
           OR EXISTS(SELECT 1 FROM effect_anchors WHERE snapshot_id=sn.id AND anchor=?2)
           OR EXISTS(SELECT 1 FROM refs WHERE snapshot_id=sn.id AND from_anchor=?2)
           OR EXISTS(SELECT 1 FROM idl_defs WHERE snapshot_id=sn.id AND anchor=?2))",
            (&selected.spec, &selected.anchor),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(failure)?;
    let (spec, snapshot_sha) = row.ok_or_else(|| RequestError {
        code: RequestErrorCode::SubjectNotFound,
        message: format!(
            "{}#{} is not indexed; update the specification first",
            selected.spec, selected.anchor
        ),
        details: None,
    })?;
    Ok(Subject {
        spec,
        anchor: selected.anchor.clone(),
        snapshot_sha,
        step_id: selected.step_id.clone(),
        step_path: selected.step_path.clone(),
        body_id: selected.body_id.clone(),
    })
}

fn manifest(
    conn: &Connection,
    catalog: &Catalog,
    scope: AnalysisScope,
    options: &EffectsOptions,
) -> Result<InputManifest, RequestError> {
    let mut statement = conn
        .prepare(
            "SELECT sp.name,sn.sha FROM specs sp JOIN snapshots sn ON sn.spec_id=sp.id
        WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' ORDER BY sp.name,sn.sha",
        )
        .map_err(failure)?;
    let specs = statement
        .query_map([], |row| {
            Ok(ManifestSpec {
                spec: row.get(0)?,
                snapshot_sha: row.get(1)?,
            })
        })
        .map_err(failure)?
        .collect::<rusqlite::Result<_>>()
        .map_err(failure)?;
    Ok(InputManifest {
        specs,
        structural_representation_version: STRUCTURE_VERSION.parse().map_err(failure)?,
        registry_resolution_version: 1,
        environment: options.environment.clone(),
        catalog_digest: catalog.content_digest.clone(),
        analysis_engine_version: engine::ANALYSIS_ENGINE_VERSION,
        scope,
        budget_profile: options.budgets.clone(),
        missing_inputs: Vec::new(),
    })
}

fn semantic_key(manifest: &InputManifest, generation: i64) -> Result<String, RequestError> {
    canonical_json_sha256(&json!({"specs":manifest.specs,"generation":generation,
        "representation":manifest.structural_representation_version,"registry":manifest.registry_resolution_version,
        "engine":manifest.analysis_engine_version,"catalog":manifest.catalog_digest,"environment":manifest.environment})).map_err(failure)
}

#[cfg(feature = "native")]
pub fn input_fingerprint(options: &EffectsOptions) -> Result<String, RequestError> {
    options.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let catalog = default_catalog(&options.rule_paths)?;
    let tx = conn.unchecked_transaction().map_err(failure)?;
    let manifest = manifest(&tx, &catalog, AnalysisScope::All, options)?;
    let inputs = semantic_key(&manifest, storage::generation(&tx).map_err(failure)?)?;
    let publication: i64 = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='effects_publication'",
            [],
            |row| row.get(0),
        )
        .map_err(failure)?;
    Ok(format!("{inputs}:{publication}"))
}

fn load_sources(conn: &Connection, catalog: &Catalog) -> Result<Vec<SourceSpec>, RequestError> {
    let mut statement = conn.prepare("SELECT sp.name,sn.sha,sp.base_url,sn.id FROM specs sp JOIN snapshots sn ON sn.spec_id=sp.id
        WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' ORDER BY sp.name,sn.sha").map_err(failure)?;
    let rows: Vec<(String, String, String, i64)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(failure)?
        .collect::<rusqlite::Result<_>>()
        .map_err(failure)?;
    let reviewed: BTreeSet<_> = catalog
        .summaries()
        .map(|s| s.subject.as_identity())
        .chain(
            catalog
                .implementations()
                .map(|i| i.implementation.as_identity()),
        )
        .collect();
    let mut sources = Vec::new();
    for (spec, snapshot_sha, base_url, snapshot_id) in rows {
        let structure: Option<StructuralSpec> =
            storage::load_structure(conn, snapshot_id, STRUCTURE_VERSION)
                .map_err(failure)?
                .map(|text| serde_json::from_str(&text))
                .transpose()
                .map_err(failure)?;
        let mut statement = conn
            .prepare(
                "WITH anchors(anchor) AS (
                SELECT anchor FROM sections WHERE snapshot_id=?1
                UNION SELECT anchor FROM effect_anchors WHERE snapshot_id=?1
                UNION SELECT from_anchor FROM refs WHERE snapshot_id=?1
                UNION SELECT anchor FROM idl_defs WHERE snapshot_id=?1)
                SELECT anchors.anchor,sections.content_text FROM anchors
                LEFT JOIN sections ON sections.snapshot_id=?1 AND sections.anchor=anchors.anchor
                ORDER BY anchors.anchor",
            )
            .map_err(failure)?;
        let anchors = statement
            .query_map([snapshot_id], |row| {
                let anchor: String = row.get(0)?;
                let text = if reviewed.contains(&format!("{spec}#{anchor}")) {
                    row.get::<_, Option<String>>(1)?.unwrap_or_default()
                } else {
                    String::new()
                };
                Ok(IndexedAnchor {
                    url: format!("{}#{anchor}", base_url.trim_end_matches('#')),
                    anchor,
                    text,
                })
            })
            .map_err(failure)?
            .collect::<rusqlite::Result<_>>()
            .map_err(failure)?;
        sources.push(SourceSpec {
            spec,
            snapshot_sha,
            base_url,
            structure,
            anchors,
        });
    }
    Ok(sources)
}

fn issue_codes(issues: &[Issue]) -> Vec<IssueCode> {
    issues
        .iter()
        .map(|issue| issue.code)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn envelope(summary: ArtifactSummary, manifest: &InputManifest, id: &str) -> EffectSummaryResult {
    EffectSummaryResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        subject: summary.subject,
        effects: summary.effects,
        effects_status: EffectsStatus::ready(
            summary.coverage,
            issue_codes(&summary.issues),
            0,
            id.into(),
        ),
        defined_bodies: summary
            .defined_bodies
            .into_iter()
            .map(|body| DefinedBody {
                subject: body.subject,
                effects: body.effects,
                effects_status: EffectsStatus::ready(
                    if body.issues.is_empty() {
                        Coverage::Complete
                    } else {
                        Coverage::Partial
                    },
                    issue_codes(&body.issues),
                    0,
                    id.into(),
                ),
            })
            .collect(),
        issues: summary.issues,
        input_manifest: Some(manifest.clone()),
    }
}

#[derive(Clone, serde::Serialize)]
struct StoredSummary {
    #[serde(flatten)]
    result: EffectSummaryResult,
    #[serde(rename = "_issue_refs")]
    issue_refs: Vec<String>,
}

fn record_envelope(
    record: engine::ArtifactSummaryRecord,
    artifact: &AnalysisArtifact,
    id: &str,
) -> StoredSummary {
    let codes = |ids: &[engine::IssueId]| {
        ids.iter()
            .filter_map(|&id| artifact.issue(id))
            .map(|issue| issue.code)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    StoredSummary {
        issue_refs: record.issue_ids.iter().map(ToString::to_string).collect(),
        result: EffectSummaryResult {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: record.subject,
            effects: record.effects,
            effects_status: EffectsStatus::ready(
                record.coverage,
                codes(&record.issue_ids),
                0,
                id.into(),
            ),
            defined_bodies: record
                .defined_bodies
                .into_iter()
                .map(|body| {
                    let issues = codes(&body.issue_ids);
                    DefinedBody {
                        subject: body.subject,
                        effects: body.effects,
                        effects_status: EffectsStatus::ready(
                            if issues.is_empty() {
                                Coverage::Complete
                            } else {
                                Coverage::Partial
                            },
                            issues,
                            0,
                            id.into(),
                        ),
                    }
                })
                .collect(),
            issues: vec![],
            input_manifest: None,
        },
    }
}

fn no_result(subject: Subject, disabled: bool) -> EffectSummaryResult {
    EffectSummaryResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        subject,
        effects: Vec::new(),
        defined_bodies: Vec::new(),
        effects_status: if disabled {
            EffectsStatus::Disabled {
                semantics: Semantics::May,
                issues: Vec::new(),
                omitted: 0,
            }
        } else {
            EffectsStatus::Unavailable {
                semantics: Semantics::May,
                issues: Vec::new(),
                omitted: 0,
            }
        },
        issues: Vec::new(),
        input_manifest: None,
    }
}

fn run_manifest(run: &storage::StoredRun) -> Result<InputManifest, RequestError> {
    serde_json::from_str(&run.manifest_json).map_err(failure)
}

fn cached_run(
    conn: &Connection,
    catalog: &Catalog,
    selected: &SubjectSelector,
    options: &EffectsOptions,
) -> Result<Option<storage::StoredRun>, RequestError> {
    if let Some(id) = &options.analysis_id {
        let run = storage::load_run(conn, id)
            .map_err(failure)?
            .ok_or_else(|| unavailable(format!("analysis {id} is no longer retained")))?;
        if storage::generation(conn).map_err(failure)? != run.generation {
            return Err(unavailable(
                "the sources for this analysis have been replaced",
            ));
        }
        let manifest = run_manifest(&run)?;
        if manifest.catalog_digest != catalog.content_digest
            || manifest.environment != options.environment
            || manifest.budget_profile != options.budgets
        {
            return Err(RequestError::invalid("historical analysis requests cannot alter catalog, environment, or discovery budgets"));
        }
        return Ok(Some(run));
    }
    let scope = owning_scope(selected);
    let tx = conn.unchecked_transaction().map_err(failure)?;
    let generation = storage::generation(&tx).map_err(failure)?;
    let manifest = manifest(&tx, catalog, scope.clone(), options)?;
    let semantic = semantic_key(&manifest, generation)?;
    let scope_key = digest_serializable(&scope).map_err(failure)?;
    let budget_key = digest_serializable(&options.budgets).map_err(failure)?;
    for run in storage::matching_runs(&tx, &semantic, generation).map_err(failure)? {
        let compatible = run.reached_fixed_point
            || options.mode == EffectsMode::Cached
            || (run.scope_key == scope_key && run.budget_key == budget_key);
        if compatible
            && storage::load_subject(&tx, &run.analysis_id, &key(selected)?)
                .map_err(failure)?
                .is_some()
        {
            return Ok(Some(run));
        }
    }
    Ok(None)
}

fn materialized(
    conn: &Connection,
    run: &storage::StoredRun,
    subject: &SubjectSelector,
) -> Result<Option<EffectSummaryResult>, RequestError> {
    let Some(text) =
        storage::load_subject(conn, &run.analysis_id, &key(subject)?).map_err(failure)?
    else {
        return Ok(None);
    };
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(failure)?;
    if let Some(choices) = value.get("ambiguous_subjects") {
        return Err(RequestError {
            code: RequestErrorCode::AmbiguousSubject,
            message: "step path identifies several sites; select a step_id".into(),
            details: Some(choices.clone()),
        });
    }
    if let Some(references) = value
        .as_object_mut()
        .and_then(|object| object.remove("_issue_refs"))
    {
        let references: Vec<String> = serde_json::from_value(references).map_err(failure)?;
        let issues: Vec<serde_json::Value> =
            storage::load_issues(conn, &run.analysis_id, &references)
                .map_err(failure)?
                .iter()
                .map(|text| serde_json::from_str(text).map_err(failure))
                .collect::<Result<_, _>>()?;
        value["issues"] = serde_json::Value::Array(issues);
    }
    let mut result: EffectSummaryResult = serde_json::from_value(value).map_err(failure)?;
    result.input_manifest = Some(run_manifest(run)?);
    Ok(Some(result))
}

fn artifact(conn: &Connection, run: &storage::StoredRun) -> Result<AnalysisArtifact, RequestError> {
    let run = if run.artifact_json.is_empty() {
        storage::load_run(conn, &run.analysis_id)
            .map_err(failure)?
            .ok_or_else(|| unavailable("analysis was removed"))?
    } else {
        run.clone()
    };
    serde_json::from_str(&run.artifact_json).map_err(failure)
}

fn compute(
    conn: &Connection,
    catalog: &Catalog,
    scope: AnalysisScope,
    options: &EffectsOptions,
) -> Result<storage::StoredRun, RequestError> {
    for _ in 0..2 {
        let (generation, mut manifest, sources) = {
            let tx = conn.unchecked_transaction().map_err(failure)?;
            let generation = storage::generation(&tx).map_err(failure)?;
            let manifest = manifest(&tx, catalog, scope.clone(), options)?;
            let sources = load_sources(&tx, catalog)?;
            (generation, manifest, sources)
        };
        let mut matches = super::local::LocalMatches::new();
        for source in &sources {
            let key = super::local::input_key(source, catalog, &options.environment);
            let cached = storage::load_local_matches(conn, &key).map_err(failure)?;
            let local = match cached {
                Some(text) => serde_json::from_str(&text).map_err(failure)?,
                None => {
                    let local = super::local::prepare(source, catalog);
                    storage::store_local_matches(
                        conn,
                        &key,
                        &serde_json::to_string(&local).map_err(failure)?,
                    )
                    .map_err(failure)?;
                    local
                }
            };
            matches.extend(local);
        }
        let artifact = engine::analyze_with_local_matches(
            AnalysisInput {
                sources: &sources,
                catalog,
                environment: &options.environment,
                scope: scope.clone(),
                budgets: options.budgets.clone(),
            },
            Some(&matches),
        )?;
        let indexed: BTreeMap<_, BTreeSet<_>> = sources
            .iter()
            .map(|source| {
                (
                    source.spec.clone(),
                    source.anchors.iter().map(|a| a.anchor.clone()).collect(),
                )
            })
            .collect();
        let processed: BTreeSet<_> = artifact
            .processed_subjects
            .iter()
            .map(|s| (s.spec.clone(), s.anchor.clone()))
            .collect();
        let mut missing = BTreeSet::new();
        for source in &sources {
            for algorithm in source.structure.iter().flat_map(|s| &s.algorithms) {
                if !processed
                    .contains(&(source.spec.clone(), algorithm.source.section_anchor.clone()))
                {
                    continue;
                }
                for target in algorithm
                    .operation_sites
                    .iter()
                    .filter(|op| op.role != crate::parse::steps::ReferenceRole::Mention)
                    .filter_map(|op| op.target.as_ref())
                {
                    if !indexed
                        .get(&target.spec)
                        .is_some_and(|anchors| anchors.contains(&target.anchor))
                    {
                        missing.insert((target.spec.clone(), target.anchor.clone()));
                    }
                }
            }
        }
        manifest.missing_inputs = missing
            .into_iter()
            .map(|(spec, anchor)| MissingInput {
                spec,
                anchor: Some(anchor),
            })
            .collect();
        let id = analysis_id(&manifest, generation).map_err(failure)?;
        let mut summaries: BTreeMap<String, Vec<StoredSummary>> = BTreeMap::new();
        for summary in artifact.summary_records(None)? {
            let subject = summary.subject.clone();
            let selected = selector(&subject);
            let result = record_envelope(summary, &artifact, &id);
            summaries
                .entry(key(&selected)?)
                .or_default()
                .push(result.clone());
            if let Some(path) = &subject.step_path {
                let by_path = SubjectSelector {
                    spec: subject.spec.clone(),
                    anchor: subject.anchor.clone(),
                    step_path: Some(path.clone()),
                    step_id: None,
                    body_id: None,
                };
                summaries.entry(key(&by_path)?).or_default().push(result);
            }
        }
        let rows: Vec<_> = summaries.into_iter().map(|(key, mut values)| {
            values.dedup_by(|a,b| a.result.subject == b.result.subject);
            let text = if values.len() == 1 { serde_json::to_string(&values[0]) } else { serde_json::to_string(&json!({"ambiguous_subjects": values.iter().map(|v| &v.result.subject).collect::<Vec<_>>()})) };
            text.map(|text| (key,text)).map_err(failure)
        }).collect::<Result<_,_>>()?;
        let run = storage::StoredRun {
            analysis_id: id,
            generation,
            semantic_key: semantic_key(&manifest, generation)?,
            scope_key: digest_serializable(&scope).map_err(failure)?,
            budget_key: digest_serializable(&options.budgets).map_err(failure)?,
            reached_fixed_point: artifact.reached_fixed_point,
            manifest_json: serde_json::to_string(&manifest).map_err(failure)?,
            artifact_json: serde_json::to_string(&artifact).map_err(failure)?,
        };
        let issues = artifact
            .issue_catalog()
            .map(|(id, issue)| {
                serde_json::to_string(issue)
                    .map(|text| (id.to_string(), text))
                    .map_err(failure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut witnesses = Vec::new();
        artifact.prepare_witnesses(|subject, effect, witness| {
            witnesses.push((
                key(&selector(subject))?,
                effect.to_owned(),
                serde_json::to_string(&witness).map_err(failure)?,
            ));
            Ok(())
        })?;
        if storage::publish_run_with_witnesses(conn, &run, &rows, &issues, &witnesses)
            .map_err(failure)?
        {
            return Ok(run);
        }
    }
    Err(RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: "snapshot_changed: corpus changed during both analysis attempts".into(),
        details: Some(json!({"issue":"snapshot_changed"})),
    })
}

#[cfg(feature = "native")]
pub fn get_effect_summary(request: &EffectsRequest) -> Result<EffectSummaryResult, RequestError> {
    let conn = db::open_or_create_db().map_err(failure)?;
    get_effect_summary_on(&conn, request)
}

pub fn get_effect_summary_on(
    conn: &Connection,
    request: &EffectsRequest,
) -> Result<EffectSummaryResult, RequestError> {
    request.validate()?;
    let subject = resolve_subject(conn, &request.subject)?;
    if request.options.mode == EffectsMode::Off {
        return Ok(no_result(subject, true));
    }
    let mut selected = request.subject.clone();
    selected.spec = subject.spec.clone();
    let catalog = default_catalog(&request.options.rule_paths)?;
    let run = match cached_run(conn, &catalog, &selected, &request.options)? {
        Some(run) => run,
        None if request.options.mode == EffectsMode::Cached => {
            return Ok(no_result(subject, false))
        }
        None => compute(conn, &catalog, owning_scope(&selected), &request.options)?,
    };
    if request.filter.is_none() {
        if let Some(result) = materialized(conn, &run, &selected)? {
            return Ok(result);
        }
    }
    let summary = artifact(conn, &run)?.summary(&selected, request.filter.as_ref())?;
    Ok(envelope(summary, &run_manifest(&run)?, &run.analysis_id))
}

/// Editor details only read prepared rows. A cache miss never starts analysis.
#[cfg(feature = "native")]
pub fn prepared_effect_details(
    request: &EffectsRequest,
) -> Result<ExplainEffectsResult, RequestError> {
    let conn = db::open_or_create_db().map_err(failure)?;
    prepared_effect_details_on(&conn, request)
}

pub fn prepared_effect_details_on(
    conn: &Connection,
    request: &EffectsRequest,
) -> Result<ExplainEffectsResult, RequestError> {
    let mut request = request.clone();
    request.options.mode = EffectsMode::Cached;
    request.options.analysis_id = None;
    request.filter = None;
    let summary = get_effect_summary_on(conn, &request)?;
    let EffectsStatus::Ready { analysis_id, .. } = &summary.effects_status else {
        return Err(unavailable("Prepared effects are unavailable. Run webspec-index effects --all --summary-only after indexing."));
    };
    let subject_key = key(&selector(&summary.subject))?;
    let mut explanations = Vec::new();
    for effect in &summary.effects {
        let witness = storage::load_witness(conn, analysis_id, &subject_key, &effect.id)
            .map_err(failure)?
            .ok_or_else(|| unavailable("Prepared paths are unavailable. Run webspec-index effects --all --summary-only to prepare them."))?;
        explanations.push(EffectExplanation {
            effect_id: effect.id.clone(),
            witnesses: vec![serde_json::from_str(&witness).map_err(failure)?],
            witnesses_truncated: true,
            issues: Vec::new(),
        });
    }
    Ok(ExplainEffectsResult {
        schema_version: summary.schema_version,
        subject: summary.subject,
        effects: summary.effects,
        effects_status: summary.effects_status,
        defined_bodies: summary.defined_bodies,
        explanations,
        issues: summary.issues,
        input_manifest: summary.input_manifest,
    })
}

#[cfg(feature = "native")]
pub fn explain_effects(
    request: &ExplainEffectsRequest,
) -> Result<ExplainEffectsResult, RequestError> {
    request.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let summary = get_effect_summary_on(
        &conn,
        &EffectsRequest {
            schema_version: request.schema_version,
            subject: request.subject.clone(),
            options: request.options.clone(),
            filter: request.filter.clone(),
        },
    )?;
    let explanations = if let EffectsStatus::Ready { analysis_id, .. } = &summary.effects_status {
        let run = storage::load_run(&conn, analysis_id)
            .map_err(failure)?
            .ok_or_else(|| unavailable("analysis was removed"))?;
        let mut selected = request.subject.clone();
        selected.spec = summary.subject.spec.clone();
        artifact(&conn, &run)?
            .explain(&selected, request.filter.as_ref(), &request.explanation)?
            .explanations
    } else {
        Vec::new()
    };
    Ok(ExplainEffectsResult {
        schema_version: summary.schema_version,
        subject: summary.subject,
        effects: summary.effects,
        effects_status: summary.effects_status,
        defined_bodies: summary.defined_bodies,
        explanations,
        issues: summary.issues,
        input_manifest: summary.input_manifest,
    })
}

#[cfg(feature = "native")]
pub fn recompute_effects(
    request: &RecomputeEffectsRequest,
) -> Result<RecomputeEffectsResult, RequestError> {
    request.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let mut scope = request.scope.clone();
    if let AnalysisScope::Subject { subject } = &mut scope {
        subject.spec = resolve_subject(&conn, subject)?.spec;
    }
    let catalog = default_catalog(&request.options.rule_paths)?;
    let run = compute(&conn, &catalog, scope, &request.options)?;
    let artifact = artifact(&conn, &run)?;
    Ok(RecomputeEffectsResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        analysis_id: run.analysis_id.clone(),
        input_manifest: run_manifest(&run)?,
        processed_subjects: artifact.processed_subjects,
        unprocessed_subjects: artifact.unprocessed_subjects,
        body_count: artifact.counts.bodies,
        relationship_count: artifact.counts.relationships,
        state_count: artifact.counts.states,
        issues: artifact.issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::write;

    fn seed(conn: &Connection, spec: &str, html: &str) -> i64 {
        let url = if spec == "DOM" {
            "https://dom.spec.whatwg.org/"
        } else {
            "https://test.example/"
        };
        let id = write::insert_or_get_spec(conn, spec, url, "test").unwrap();
        let snapshot = write::insert_snapshot(conn, id, "hash:test", "2026-09-13").unwrap();
        let structure = crate::parse::steps::extract_step_structure(html, spec, url, "hash:test");
        storage::store_structure(
            conn,
            snapshot,
            STRUCTURE_VERSION,
            &serde_json::to_string(&structure).unwrap(),
        )
        .unwrap();
        snapshot
    }

    fn request(spec: &str, anchor: &str) -> EffectsRequest {
        serde_json::from_value(json!({"schema_version":1,"subject":{"spec":spec,"anchor":anchor}}))
            .unwrap()
    }

    fn setup() -> (Connection, i64) {
        let conn = db::open_test_db().unwrap();
        seed(
            &conn,
            "DOM",
            "<p>To <dfn id=concept-event-fire>fire an event</dfn>, dispatch it.</p>",
        );
        let root = seed(&conn, "TEST", "<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol><li><a href='https://dom.spec.whatwg.org/#concept-event-fire'>Fire an event</a> named <code>hello</code>.</li></ol></div>");
        (conn, root)
    }

    #[test]
    fn cache_miss_is_unavailable_and_off_does_not_compute() {
        let (conn, _) = setup();
        let mut req = request("TEST", "R");
        req.options.mode = EffectsMode::Cached;
        assert!(matches!(
            get_effect_summary_on(&conn, &req).unwrap().effects_status,
            EffectsStatus::Unavailable { .. }
        ));
        req.options.mode = EffectsMode::Off;
        assert!(matches!(
            get_effect_summary_on(&conn, &req).unwrap().effects_status,
            EffectsStatus::Disabled { .. }
        ));
        assert_eq!(
            conn.query_row("SELECT count(*) FROM effect_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn prose_endpoint_is_known_but_dangling_reference_is_not() {
        let (conn, root) = setup();
        let catalog = default_catalog(&[]).unwrap();
        conn.execute("INSERT INTO refs(snapshot_id,from_anchor,to_spec,to_anchor) VALUES (?1,'R','TEST','missing')", [root]).unwrap();
        let sources = load_sources(&conn, &catalog).unwrap();
        assert!(sources
            .iter()
            .find(|s| s.spec == "DOM")
            .unwrap()
            .anchors
            .iter()
            .any(|a| a.anchor == "concept-event-fire"));
        assert!(!sources
            .iter()
            .find(|s| s.spec == "TEST")
            .unwrap()
            .anchors
            .iter()
            .any(|a| a.anchor == "missing"));
        let result = get_effect_summary_on(&conn, &request("dom", "concept-event-fire")).unwrap();
        assert!(result.effects.iter().any(|e| e.kind == "event.fire"));
        assert_eq!(
            get_effect_summary_on(&conn, &request("TEST", "missing"))
                .unwrap_err()
                .code,
            RequestErrorCode::SubjectNotFound
        );
    }

    #[test]
    fn prepared_details_never_compute_and_survive_unreadable_graph() {
        let (conn, _) = setup();
        let req = request("TEST", "R");
        assert!(prepared_effect_details_on(&conn, &req).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM effect_runs", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        get_effect_summary_on(&conn, &req).unwrap();
        conn.execute(
            "UPDATE effect_runs SET artifact_json='invalid graph JSON'",
            [],
        )
        .unwrap();
        let details = prepared_effect_details_on(&conn, &req).unwrap();
        assert!(!details.effects.is_empty());
        assert_eq!(details.effects.len(), details.explanations.len());
        assert!(details.explanations.iter().all(|e| e.witnesses.len() == 1));
        conn.execute("DELETE FROM effect_witnesses", []).unwrap();
        assert!(
            prepared_effect_details_on(&conn, &req).is_err(),
            "missing prepared paths cannot fall back to graph search"
        );
    }

    #[test]
    fn warm_summary_reads_materialized_row_without_decoding_graph() {
        let (conn, _) = setup();
        let req = request("TEST", "R");
        let cold = get_effect_summary_on(&conn, &req).unwrap();
        assert!(cold.effects.iter().any(|e| e.kind == "event.fire"
            && e.params.get("name") == Some(&Some(EffectValue::String("hello".into())))));
        let location = cold
            .effects
            .iter()
            .find(|e| e.params.get("name") == Some(&Some(EffectValue::String("hello".into()))))
            .unwrap()
            .location
            .as_ref()
            .unwrap();
        assert_eq!(location.spec, "TEST");
        assert_eq!(location.anchor, "R");
        assert_eq!(location.step_path, Some(vec![1]));
        conn.execute(
            "UPDATE effect_runs SET artifact_json='invalid graph JSON'",
            [],
        )
        .unwrap();
        assert_eq!(get_effect_summary_on(&conn, &req).unwrap(), cold);
        let mut step = req;
        step.subject.step_path = Some(vec![1]);
        assert_eq!(
            get_effect_summary_on(&conn, &step).unwrap().effects,
            cold.effects
        );
    }

    #[test]
    fn source_replacement_removes_effects_and_invalidates_exact_run() {
        let (conn, root) = setup();
        let req = request("TEST", "R");
        let before = get_effect_summary_on(&conn, &req).unwrap();
        let EffectsStatus::Ready { analysis_id, .. } = before.effects_status else {
            panic!("analysis not ready")
        };
        let replacement = crate::parse::steps::extract_step_structure(
            "<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol><li>Return.</li></ol></div>",
            "TEST",
            "https://test.example/",
            "hash:replacement",
        );
        conn.execute(
            "UPDATE snapshots SET sha='hash:replacement' WHERE id=?1",
            [root],
        )
        .unwrap();
        storage::store_structure(
            &conn,
            root,
            STRUCTURE_VERSION,
            &serde_json::to_string(&replacement).unwrap(),
        )
        .unwrap();
        let after = get_effect_summary_on(&conn, &req).unwrap();
        assert!(after.effects.is_empty());
        let mut old = req;
        old.options.analysis_id = Some(analysis_id);
        assert_eq!(
            get_effect_summary_on(&conn, &old).unwrap_err().code,
            RequestErrorCode::AnalysisUnavailable
        );
    }
    #[cfg(feature = "native")]
    #[test]
    fn changing_rule_contents_invalidates_without_reparsing_sources() {
        let (conn, _) = setup();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.yaml");
        let yaml = "schema: 1\npackage: extra\nsummaries:\n  - id: custom\n    subject: TEST#R\n    expect_text: Fire an event\n    reason: fixture declaration\n    emit:\n      kind: event.fire\n      params: {name: first}\n";
        std::fs::write(&path, yaml).unwrap();
        let mut req = request("TEST", "R");
        req.options.rule_paths = vec![dir.path().to_string_lossy().into()];
        let before = get_effect_summary_on(&conn, &req).unwrap();
        let generation = storage::generation(&conn).unwrap();
        std::fs::write(&path, yaml.replace("first", "second")).unwrap();
        let after = get_effect_summary_on(&conn, &req).unwrap();
        assert_eq!(generation, storage::generation(&conn).unwrap());
        assert_ne!(before.effects_status, after.effects_status);
        assert!(after
            .effects
            .iter()
            .any(|effect| effect.params.get("name")
                == Some(&Some(EffectValue::String("second".into())))));
        assert!(!after
            .effects
            .iter()
            .any(|effect| effect.params.get("name")
                == Some(&Some(EffectValue::String("first".into())))));
    }
    #[test]
    fn global_recompute_reuses_scoped_local_matches() {
        let (conn, _) = setup();
        let req = request("TEST", "R");
        get_effect_summary_on(&conn, &req).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM effect_local_matches", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("CREATE TRIGGER reject_local_match_write BEFORE INSERT ON effect_local_matches BEGIN SELECT RAISE(ABORT,'local matches must be reused'); END;").unwrap();
        let catalog = default_catalog(&[]).unwrap();
        let run = compute(&conn, &catalog, AnalysisScope::All, &req.options).unwrap();
        let cached = artifact(&conn, &run).unwrap();
        let sources = load_sources(&conn, &catalog).unwrap();
        let direct = engine::analyze(AnalysisInput {
            sources: &sources,
            catalog: &catalog,
            environment: &req.options.environment,
            scope: AnalysisScope::All,
            budgets: req.options.budgets,
        })
        .unwrap();
        assert_eq!(cached, direct);
    }

    #[test]
    fn same_snapshot_structural_change_cannot_reuse_exact_run_id() {
        let (conn, root) = setup();
        let req = request("TEST", "R");
        let before = get_effect_summary_on(&conn, &req).unwrap();
        let EffectsStatus::Ready { analysis_id, .. } = before.effects_status else {
            panic!("analysis not ready")
        };
        let replacement = crate::parse::steps::extract_step_structure(
            "<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol><li>Return.</li></ol></div>",
            "TEST",
            "https://test.example/",
            "hash:test",
        );
        storage::store_structure(
            &conn,
            root,
            STRUCTURE_VERSION,
            &serde_json::to_string(&replacement).unwrap(),
        )
        .unwrap();
        let after = get_effect_summary_on(&conn, &req).unwrap();
        let EffectsStatus::Ready {
            analysis_id: new_id,
            ..
        } = after.effects_status
        else {
            panic!("analysis not ready")
        };
        assert_ne!(analysis_id, new_id);
        let mut old = req;
        old.options.analysis_id = Some(analysis_id);
        assert_eq!(
            get_effect_summary_on(&conn, &old).unwrap_err().code,
            RequestErrorCode::AnalysisUnavailable
        );
    }
    #[test]
    fn partial_global_run_does_not_suppress_subject_completion() {
        let (conn, _) = setup();
        let req = request("TEST", "R");
        let catalog = default_catalog(&[]).unwrap();
        let mut limited = req.options.clone();
        limited.budgets.max_bodies = 1;
        limited.budgets.max_relationships = 1;
        let partial = compute(&conn, &catalog, AnalysisScope::All, &limited).unwrap();
        assert!(!partial.reached_fixed_point);
        let result = get_effect_summary_on(&conn, &req).unwrap();
        let EffectsStatus::Ready {
            analysis_id,
            issues,
            ..
        } = result.effects_status
        else {
            panic!("analysis not ready")
        };
        assert_ne!(analysis_id, partial.analysis_id);
        assert!(!issues.contains(&IssueCode::AnalysisBudget));
        assert!(!result.effects.is_empty());
    }
    #[test]
    fn compact_navigation_style_summary_keeps_async_and_script_before_many_events() {
        let conn = db::open_test_db().unwrap();
        seed(
            &conn,
            "DOM",
            "<dfn id=concept-event-fire>fire an event</dfn>",
        );
        seed(&conn, "HTML", "<dfn id=queue-a-microtask>queue a microtask</dfn><dfn id=run-a-classic-script>run a classic script</dfn>");
        let mut html = String::from("<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol>");
        for index in 0..14 {
            html.push_str(&format!("<li><a href='https://dom.spec.whatwg.org/#concept-event-fire'>Fire an event</a> named <code>event-{index}</code>.</li>"));
        }
        html.push_str("<li><a href='https://html.spec.whatwg.org/#queue-a-microtask'>Queue a microtask</a>.</li><li><a href='https://html.spec.whatwg.org/#run-a-classic-script'>Run a classic script</a>.</li></ol></div>");
        seed(&conn, "TEST", &html);
        let result = get_effect_summary_on(&conn, &request("TEST", "R")).unwrap();
        assert_eq!(result.effects.len(), 16);
        let compact: Vec<_> = result
            .effects
            .iter()
            .take(super::super::query::DEFAULT_QUERY_EFFECT_LIMIT)
            .map(|effect| effect.kind.as_str())
            .collect();
        assert_eq!(&compact[..2], &["scheduling.enqueue", "script.invoke"]);
        let badges = super::super::render::compact_badges(&result.effects, 3, 120, None);
        assert!(badges.contains("queue a microtask"));
        assert!(badges.contains("run author code"));
    }
}
