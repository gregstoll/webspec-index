//! Snapshot-consistent analysis orchestration, persisted summaries, and cache policy.
use super::bundled::default_catalog;
use super::catalog::Catalog;
#[cfg(any(feature = "native", test))]
use super::engine::IssueId;
use super::engine::{
    self, AnalysisArtifact, ArtifactSummary, GraphInput, IndexedAnchor, SourceSpec,
};
use super::graph::ExecutionNode;
use super::model::*;
#[cfg(any(feature = "native", test))]
use crate::db;
use crate::db::effects as storage;
use crate::parse::steps::{StructuralSpec, STRUCTURE_VERSION};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
#[cfg(any(feature = "native", test))]
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

fn failure(error: impl std::fmt::Display) -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: error.to_string(),
        details: None,
    }
}

const COMPLETE_CACHE_KEY: &str = "__effect_summary_complete__";

fn unavailable(message: impl Into<String>) -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisUnavailable,
        message: message.into(),
        details: None,
    }
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

pub fn graph_semantic_key(
    manifest: &InputManifest,
    generation: i64,
) -> Result<String, RequestError> {
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
    let generation = storage::generation(&tx).map_err(failure)?;
    // A stored graph and a missing one must fingerprint differently: an LSP that
    // cached an unavailable result has to notice when the graph gets built.
    match storage::load_graph_meta(&tx).map_err(failure)? {
        Some(meta) => Ok(format!("{}:{}:graph", meta.semantic_key, meta.generation)),
        None => {
            let m = manifest(&tx, &catalog, AnalysisScope::All, options)?;
            let sk = graph_semantic_key(&m, generation)?;
            Ok(format!("{sk}:{generation}:none"))
        }
    }
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

#[derive(Serialize, Deserialize)]
struct CompactSummary {
    subject: Subject,
    effects: Vec<EffectSummary>,
    coverage: Coverage,
    issue_codes: Vec<IssueCode>,
    defined_bodies: Vec<CompactBody>,
}

#[derive(Serialize, Deserialize)]
struct CompactBody {
    subject: Subject,
    effects: Vec<EffectSummary>,
    issue_codes: Vec<IssueCode>,
}

fn selector_key(subject: &SubjectSelector) -> Result<String, RequestError> {
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

#[cfg(any(feature = "native", test))]
fn record_selector(subject: &Subject) -> SubjectSelector {
    SubjectSelector {
        spec: subject.spec.clone(),
        anchor: subject.anchor.clone(),
        step_id: subject.step_id.clone(),
        step_path: subject.step_path.clone(),
        body_id: subject.body_id.clone(),
    }
}

#[cfg(any(feature = "native", test))]
fn codes_for_ids(artifact: &AnalysisArtifact, ids: &[IssueId]) -> Vec<IssueCode> {
    ids.iter()
        .filter_map(|id| artifact.issue(*id).map(|issue| issue.code))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(any(feature = "native", test))]
fn store_compact_summaries(
    conn: &Connection,
    graph: &engine::Graph,
    artifact: &AnalysisArtifact,
    meta: &storage::StoredGraphMeta,
    budgets: &DiscoveryBudgets,
) -> Result<(), RequestError> {
    // A bounded global run is not interchangeable with a scoped run: the latter
    // may reach a subject that the former did not. Publish only a complete run.
    if !artifact.reached_fixed_point || !artifact.unprocessed_subjects.is_empty() {
        return Ok(());
    }
    let budget_key =
        canonical_json(&serde_json::to_value(budgets).map_err(failure)?).map_err(failure)?;
    let mut rows = artifact
        .summary_records(None)?
        .into_iter()
        .map(|record| {
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
            Ok((
                compact.subject.spec.clone(),
                selector_key(&record_selector(&compact.subject))?,
                serde_json::to_string(&compact).map_err(failure)?,
            ))
        })
        .collect::<Result<Vec<_>, RequestError>>()?;
    let mut stored_keys: HashSet<String> = rows.iter().map(|(_, key, _)| key.clone()).collect();
    // A missing top-level row can be synthesized only if the graph has no
    // matching anchor. Keep tiny miss markers for graph anchors not emitted by
    // summary_records, so the lookup can distinguish those cases.
    for (spec, anchor) in graph.anchor_nodes.keys() {
        let key = selector_key(&SubjectSelector {
            spec: spec.clone(),
            anchor: anchor.clone(),
            step_id: None,
            step_path: None,
            body_id: None,
        })?;
        if stored_keys.insert(key.clone()) {
            rows.push((spec.clone(), key, "null".into()));
        }
    }
    // This row certifies that an absent key means an ordinary indexed anchor
    // outside the graph, rather than an incompletely populated cache.
    rows.push((String::new(), COMPLETE_CACHE_KEY.into(), "{}".into()));
    let tx = conn.unchecked_transaction().map_err(failure)?;
    if storage::generation(&tx).map_err(failure)? != meta.generation {
        return Err(unavailable(
            "snapshot changed while preparing effect summaries",
        ));
    }
    storage::store_summary_cache(&tx, &meta.semantic_key, &budget_key, rows).map_err(failure)?;
    tx.commit().map_err(failure)
}

fn compact_envelope(
    summary: CompactSummary,
    manifest: InputManifest,
    id: &str,
) -> EffectSummaryResult {
    EffectSummaryResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        subject: summary.subject,
        effects: summary.effects,
        effects_status: EffectsStatus::ready(summary.coverage, summary.issue_codes, 0, id.into()),
        defined_bodies: summary
            .defined_bodies
            .into_iter()
            .map(|body| DefinedBody {
                subject: body.subject,
                effects: body.effects,
                effects_status: EffectsStatus::ready(
                    if body.issue_codes.is_empty() {
                        Coverage::Complete
                    } else {
                        Coverage::Partial
                    },
                    body.issue_codes,
                    0,
                    id.into(),
                ),
            })
            .collect(),
        issues: Vec::new(),
        input_manifest: Some(manifest),
    }
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

/// Compute local matches for the given uncached sources.
///
/// `threads` overrides the thread count; `None` reads `WEBSPEC_EFFECTS_THREADS` from
/// the environment and falls back to [`std::thread::available_parallelism`]. Under the
/// `native` feature this dispatches work across a rayon thread pool; without that feature
/// it falls back to a serial loop so wasm and lib-only builds are unaffected.
fn parallel_prepare(
    sources: &[super::engine::SourceSpec],
    catalog: &Catalog,
    uncached: &[usize],
    threads: Option<usize>,
) -> Vec<(usize, super::local::LocalMatches)> {
    #[cfg(feature = "native")]
    {
        use rayon::prelude::*;
        let thread_count: usize = threads.unwrap_or_else(|| {
            std::env::var("WEBSPEC_EFFECTS_THREADS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(1)
                })
        });
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build()
            .expect("rayon thread pool");
        pool.install(|| {
            uncached
                .par_iter()
                .map(|&i| (i, super::local::prepare(&sources[i], catalog)))
                .collect()
        })
    }
    #[cfg(not(feature = "native"))]
    {
        let _ = threads;
        uncached
            .iter()
            .map(|&i| (i, super::local::prepare(&sources[i], catalog)))
            .collect()
    }
}

pub fn build_and_store_graph(
    conn: &Connection,
    catalog: &Catalog,
    options: &EffectsOptions,
    threads: Option<usize>,
) -> Result<storage::StoredGraphMeta, RequestError> {
    for _ in 0..2 {
        let (generation, mut m, sources) = {
            let tx = conn.unchecked_transaction().map_err(failure)?;
            let generation = storage::generation(&tx).map_err(failure)?;
            let m = manifest(&tx, catalog, AnalysisScope::All, options)?;
            let sources = load_sources(&tx, catalog)?;
            (generation, m, sources)
        };
        let mut matches = super::local::LocalMatches::new();
        {
            let keys: Vec<String> = sources
                .iter()
                .map(|s| super::local::input_key(s, catalog, &options.environment))
                .collect();
            let mut cached_locals: Vec<Option<super::local::LocalMatches>> =
                Vec::with_capacity(sources.len());
            let mut uncached: Vec<usize> = Vec::new();
            for (i, key) in keys.iter().enumerate() {
                match storage::load_local_matches(conn, key).map_err(failure)? {
                    Some(text) => {
                        cached_locals.push(Some(serde_json::from_str(&text).map_err(failure)?));
                    }
                    None => {
                        cached_locals.push(None);
                        uncached.push(i);
                    }
                }
            }
            let computed: Vec<(usize, super::local::LocalMatches)> =
                parallel_prepare(&sources, catalog, &uncached, threads);
            for (i, local) in &computed {
                storage::store_local_matches(
                    conn,
                    &keys[*i],
                    &serde_json::to_string(local).map_err(failure)?,
                )
                .map_err(failure)?;
            }
            let mut computed_map: std::collections::HashMap<usize, super::local::LocalMatches> =
                computed.into_iter().collect();
            for (i, slot) in cached_locals.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(
                        computed_map
                            .remove(&i)
                            .expect("every uncached source was computed"),
                    );
                }
            }
            for local in cached_locals.into_iter().flatten() {
                matches.extend(local);
            }
        }
        let graph = engine::build_graph(
            GraphInput {
                sources: &sources,
                catalog,
                environment: &options.environment,
            },
            Some(&matches),
        )?;

        // Compute missing_inputs from graph algorithm roots.
        let indexed: BTreeMap<_, BTreeSet<_>> = sources
            .iter()
            .map(|source| {
                (
                    source.spec.clone(),
                    source.anchors.iter().map(|a| a.anchor.clone()).collect(),
                )
            })
            .collect();
        let algorithm_roots: std::collections::HashSet<(String, String)> = graph
            .nodes
            .values()
            .filter(|n| n.is_body && n.subject.body_id.is_none())
            .map(|n| (n.subject.spec.clone(), n.subject.anchor.clone()))
            .collect();
        let mut missing = BTreeSet::new();
        for source in &sources {
            for algorithm in source.structure.iter().flat_map(|s| &s.algorithms) {
                if !algorithm_roots
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
        m.missing_inputs = missing
            .into_iter()
            .map(|(spec, anchor)| MissingInput {
                spec,
                anchor: Some(anchor),
            })
            .collect();

        let sk = graph_semantic_key(&m, generation)?;
        let meta = storage::StoredGraphMeta {
            generation,
            semantic_key: sk,
            manifest_json: serde_json::to_string(&m).map_err(failure)?,
            opaque_anchor_issue: None,
        };
        let store_ok = {
            let tx = conn.unchecked_transaction().map_err(failure)?;
            if storage::generation(&tx).map_err(failure)? != generation {
                false
            } else {
                storage::store_graph(&tx, &meta, &graph).map_err(failure)?;
                tx.commit().map_err(failure)?;
                true
            }
        };
        if store_ok {
            GRAPH.with(|cell| cell.borrow_mut().take());
            ANALYSES.with(|cell| cell.borrow_mut().clear());
            return Ok(meta);
        }
    }
    Err(RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: "snapshot_changed: corpus changed during both analysis attempts".into(),
        details: Some(json!({"issue":"snapshot_changed"})),
    })
}

#[cfg(feature = "native")]
pub fn recompute_effects(
    request: &RecomputeEffectsRequest,
) -> Result<RecomputeEffectsResult, RequestError> {
    request.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let catalog = default_catalog(&request.options.rule_paths)?;
    let meta = build_and_store_graph(&conn, &catalog, &request.options, None)?;
    let m: InputManifest = serde_json::from_str(&meta.manifest_json).map_err(failure)?;

    let graph = storage::load_graph(&conn)
        .map_err(failure)?
        .ok_or_else(|| failure("graph was not stored"))?;

    let body_count = graph.nodes.values().filter(|n| n.is_body).count() as u64;
    let relationship_count = graph.edges.len() as u64;

    let artifact =
        engine::analyze_graph(&graph, AnalysisScope::All, request.options.budgets.clone())?;
    store_compact_summaries(&conn, &graph, &artifact, &meta, &request.options.budgets)?;
    let site_store = storage::SqlSiteStore(&conn);
    let all_issue_ids: Vec<_> = (0..artifact.issues.len() as IssueId).collect();
    let all_issues = artifact.materialize_issues(&all_issue_ids, &site_store)?;
    let issue_count = all_issues.len() as u64;
    let cap = 200.min(all_issues.len());

    Ok(RecomputeEffectsResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        input_manifest: m,
        body_count,
        relationship_count,
        issues: all_issues.into_iter().take(cap).collect(),
        issue_count,
    })
}

// ----- Thread-local caches -----

struct LoadedGraph {
    meta: storage::StoredGraphMeta,
    graph: Rc<engine::Graph>,
}

type AnalysisKey = (String, String, String); // (semantic_key, scope_json, budgets_json)
const ANALYSIS_CACHE_CAPACITY: usize = 8;

thread_local! {
    static GRAPH: RefCell<Option<LoadedGraph>> = const { RefCell::new(None) };
    static ANALYSES: RefCell<VecDeque<(AnalysisKey, Rc<AnalysisArtifact>)>> = const { RefCell::new(VecDeque::new()) };
}

fn load_cached_graph(
    conn: &Connection,
) -> Result<Option<(storage::StoredGraphMeta, Rc<engine::Graph>)>, RequestError> {
    let meta = storage::load_graph_meta(conn).map_err(failure)?;
    let Some(meta) = meta else {
        return Ok(None);
    };
    let generation = storage::generation(conn).map_err(failure)?;
    if meta.generation != generation {
        return Ok(None);
    }
    let cached = GRAPH.with(|cell| {
        let borrow = cell.borrow();
        if let Some(loaded) = borrow.as_ref() {
            if loaded.meta.semantic_key == meta.semantic_key
                && loaded.meta.generation == meta.generation
            {
                return Some((loaded.meta.clone(), Rc::clone(&loaded.graph)));
            }
        }
        None
    });
    if let Some(pair) = cached {
        return Ok(Some(pair));
    }
    let graph = storage::load_graph(conn)
        .map_err(failure)?
        .ok_or_else(|| failure("graph metadata exists but topology is missing"))?;
    let rc = Rc::new(graph);
    GRAPH.with(|cell| {
        *cell.borrow_mut() = Some(LoadedGraph {
            meta: meta.clone(),
            graph: Rc::clone(&rc),
        });
    });
    ANALYSES.with(|cell| cell.borrow_mut().clear());
    Ok(Some((meta, rc)))
}

fn graph_for(
    conn: &Connection,
    catalog: &Catalog,
    options: &EffectsOptions,
) -> Result<Option<(storage::StoredGraphMeta, Rc<engine::Graph>)>, RequestError> {
    if let Some(pair) = load_cached_graph(conn)? {
        let meta = &pair.0;
        let m: InputManifest = serde_json::from_str(&meta.manifest_json).map_err(failure)?;
        if m.catalog_digest != catalog.content_digest || m.environment != options.environment {
            return Err(RequestError::invalid(
                "stored graph was built with a different catalog or environment",
            ));
        }
        return Ok(Some(pair));
    }
    match options.mode {
        EffectsMode::Cached => Ok(None),
        #[cfg(feature = "native")]
        EffectsMode::Auto => {
            build_and_store_graph(conn, catalog, options, None)?;
            load_cached_graph(conn)
        }
        #[cfg(not(feature = "native"))]
        EffectsMode::Auto => Ok(None),
        EffectsMode::Off => Ok(None),
    }
}

fn artifact_for(
    graph: &Rc<engine::Graph>,
    semantic_key: &str,
    scope: AnalysisScope,
    budgets: &DiscoveryBudgets,
) -> Result<Rc<AnalysisArtifact>, RequestError> {
    let scope_json =
        canonical_json(&serde_json::to_value(&scope).map_err(failure)?).map_err(failure)?;
    let budget_json =
        canonical_json(&serde_json::to_value(budgets).map_err(failure)?).map_err(failure)?;
    let key = (semantic_key.to_owned(), scope_json, budget_json);
    let found = ANALYSES.with(|cell| {
        let q = cell.borrow();
        q.iter().find(|(k, _)| *k == key).map(|(_, a)| Rc::clone(a))
    });
    if let Some(rc) = found {
        return Ok(rc);
    }
    let artifact = engine::analyze_graph(graph, scope, budgets.clone())?;
    let rc = Rc::new(artifact);
    ANALYSES.with(|cell| {
        let mut q = cell.borrow_mut();
        if q.len() >= ANALYSIS_CACHE_CAPACITY {
            q.pop_front();
        }
        q.push_back((key, Rc::clone(&rc)));
    });
    Ok(rc)
}

fn synthesise_anchor(
    graph: &engine::Graph,
    meta: &storage::StoredGraphMeta,
    spec: &str,
    anchor: &str,
    snapshot_sha: &str,
) -> engine::Graph {
    let mut g = graph.clone();
    let node_id = format!("anchor:{spec}#{anchor}");
    let node = ExecutionNode {
        id: node_id.clone(),
        subject: Subject {
            spec: spec.to_owned(),
            anchor: anchor.to_owned(),
            snapshot_sha: snapshot_sha.to_owned(),
            step_id: None,
            step_path: None,
            body_id: None,
        },
        source_order: u64::MAX,
        is_body: true,
        definition_only: false,
    };
    g.nodes.insert(node_id.clone(), node);
    g.anchor_nodes
        .insert((spec.to_owned(), anchor.to_owned()), node_id.clone());
    if let Some(issue_id) = meta.opaque_anchor_issue {
        g.issues.entry(node_id).or_default().push(issue_id);
    }
    g.index_edges();
    g
}

#[cfg(feature = "native")]
pub fn get_effect_summary(request: &EffectsRequest) -> Result<EffectSummaryResult, RequestError> {
    let conn = db::open_or_create_db().map_err(failure)?;
    get_effect_summary_on(&conn, request)
}

/// Returns the prepared compact summary without loading the topology or running analysis.
/// A missing row is a cache miss; callers can retain their existing analysis fallback.
#[cfg(feature = "native")]
pub fn get_cached_effect_preview(
    request: &EffectsRequest,
) -> Result<Option<EffectSummaryResult>, RequestError> {
    let conn = db::open_or_create_db().map_err(failure)?;
    get_cached_effect_preview_on(&conn, request)
}

#[cfg(feature = "native")]
pub fn get_effect_preview(request: &EffectsRequest) -> Result<EffectSummaryResult, RequestError> {
    match get_cached_effect_preview(request)? {
        Some(summary) => Ok(summary),
        None => get_effect_summary(request),
    }
}

pub fn get_cached_effect_preview_on(
    conn: &Connection,
    request: &EffectsRequest,
) -> Result<Option<EffectSummaryResult>, RequestError> {
    request.validate()?;
    if request.options.mode == EffectsMode::Off || request.filter.is_some() {
        return Ok(None);
    }
    let subject = resolve_subject(conn, &request.subject)?;
    let mut selector = request.subject.clone();
    selector.spec = subject.spec.clone();
    let Some(meta) = storage::load_graph_meta(conn).map_err(failure)? else {
        return Ok(None);
    };
    if meta.generation != storage::generation(conn).map_err(failure)? {
        return Ok(None);
    }
    let manifest: InputManifest = serde_json::from_str(&meta.manifest_json).map_err(failure)?;
    let catalog = default_catalog(&request.options.rule_paths)?;
    if manifest.catalog_digest != catalog.content_digest
        || manifest.environment != request.options.environment
    {
        return Ok(None);
    }
    let budget_key =
        canonical_json(&serde_json::to_value(&request.options.budgets).map_err(failure)?)
            .map_err(failure)?;
    let key = selector_key(&selector)?;
    let payload = storage::load_summary_cache(conn, &key, &meta.semantic_key, &budget_key)
        .map_err(failure)?;
    let Some(payload) = payload else {
        if selector.step_id.is_some() || selector.step_path.is_some() || selector.body_id.is_some()
        {
            return Ok(None);
        }
        if storage::load_summary_cache(conn, COMPLETE_CACHE_KEY, &meta.semantic_key, &budget_key)
            .map_err(failure)?
            .is_none()
        {
            return Ok(None);
        }
        let issue_codes = if meta.opaque_anchor_issue.is_some() {
            vec![IssueCode::UnsupportedStructure]
        } else {
            Vec::new()
        };
        let compact = CompactSummary {
            subject,
            effects: Vec::new(),
            coverage: if issue_codes.is_empty() {
                Coverage::Complete
            } else {
                Coverage::Partial
            },
            issue_codes,
            defined_bodies: Vec::new(),
        };
        let id = format!("an_{}", &meta.semantic_key[..16]);
        return Ok(Some(compact_envelope(compact, manifest, &id)));
    };
    if payload == "null" {
        return Ok(None);
    }
    let compact: CompactSummary = serde_json::from_str(&payload).map_err(failure)?;
    if compact.subject != subject {
        return Ok(None);
    }
    let id = format!("an_{}", &meta.semantic_key[..16]);
    Ok(Some(compact_envelope(compact, manifest, &id)))
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
    let Some((meta, graph)) = graph_for(conn, &catalog, &request.options)? else {
        return Ok(no_result(subject, false));
    };
    let m: InputManifest = serde_json::from_str(&meta.manifest_json).map_err(failure)?;
    let analysis_id = format!("an_{}", &meta.semantic_key[..16]);

    let has_node = graph
        .anchor_nodes
        .contains_key(&(subject.spec.clone(), subject.anchor.clone()));
    let no_step_body =
        selected.step_id.is_none() && selected.step_path.is_none() && selected.body_id.is_none();

    let (use_graph, scope) = if has_node {
        (Rc::clone(&graph), owning_scope(&selected))
    } else if no_step_body {
        let synth = synthesise_anchor(
            &graph,
            &meta,
            &subject.spec,
            &subject.anchor,
            &subject.snapshot_sha,
        );
        let scope = owning_scope(&selected);
        (Rc::new(synth), scope)
    } else {
        return Err(RequestError {
            code: RequestErrorCode::SubjectNotFound,
            message: format!(
                "{}#{} has no graph node and requires step/body selection",
                subject.spec, subject.anchor
            ),
            details: None,
        });
    };

    let artifact = if has_node {
        artifact_for(&graph, &meta.semantic_key, scope, &request.options.budgets)?
    } else {
        Rc::new(engine::analyze_graph(
            &use_graph,
            scope,
            request.options.budgets.clone(),
        )?)
    };
    let site_store = storage::SqlSiteStore(conn);
    let summary = artifact.summary(&selected, request.filter.as_ref(), &site_store)?;
    Ok(envelope(summary, &m, &analysis_id))
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
    request.filter = None;
    let summary = get_effect_summary_on(conn, &request)?;
    let EffectsStatus::Ready { .. } = &summary.effects_status else {
        return Err(unavailable("Prepared effects are unavailable. Run webspec-index effects --all --summary-only after indexing."));
    };

    let mut selected = request.subject.clone();
    selected.spec = summary.subject.spec.clone();
    let catalog = default_catalog(&request.options.rule_paths)?;
    let Some((meta, graph)) = graph_for(conn, &catalog, &request.options)? else {
        return Err(unavailable("Prepared effects are unavailable. Run webspec-index effects --all --summary-only after indexing."));
    };

    let has_node = graph
        .anchor_nodes
        .contains_key(&(summary.subject.spec.clone(), summary.subject.anchor.clone()));
    let no_step_body =
        selected.step_id.is_none() && selected.step_path.is_none() && selected.body_id.is_none();

    let use_graph = if has_node {
        Rc::clone(&graph)
    } else if no_step_body {
        Rc::new(synthesise_anchor(
            &graph,
            &meta,
            &summary.subject.spec,
            &summary.subject.anchor,
            &summary.subject.snapshot_sha,
        ))
    } else {
        return Err(unavailable("Prepared paths are unavailable. Run webspec-index effects --all --summary-only to prepare them."));
    };

    let scope = owning_scope(&selected);
    let artifact = if has_node {
        artifact_for(&graph, &meta.semantic_key, scope, &request.options.budgets)?
    } else {
        Rc::new(engine::analyze_graph(
            &use_graph,
            scope,
            request.options.budgets.clone(),
        )?)
    };

    let site_store = storage::SqlSiteStore(conn);
    let mut witness_map: BTreeMap<String, super::model::Witness> = BTreeMap::new();
    artifact.prepare_witnesses(&site_store, |subj, effect_id, witness| {
        if subj == &summary.subject {
            witness_map.entry(effect_id.to_owned()).or_insert(witness);
        }
        Ok(())
    })?;

    let mut explanations = Vec::new();
    for effect in &summary.effects {
        match witness_map.remove(&effect.id) {
            Some(witness) => {
                explanations.push(EffectExplanation {
                    effect_id: effect.id.clone(),
                    witnesses: vec![witness],
                    witnesses_truncated: true,
                    issues: Vec::new(),
                });
            }
            None => {
                return Err(unavailable("Prepared paths are unavailable. Run webspec-index effects --all --summary-only to prepare them."));
            }
        }
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
    let explanations = if let EffectsStatus::Ready { .. } = &summary.effects_status {
        let mut selected = request.subject.clone();
        selected.spec = summary.subject.spec.clone();
        let catalog = default_catalog(&request.options.rule_paths)?;
        let Some((meta, graph)) = graph_for(&conn, &catalog, &request.options)? else {
            return Ok(ExplainEffectsResult {
                schema_version: summary.schema_version,
                subject: summary.subject,
                effects: summary.effects,
                effects_status: summary.effects_status,
                defined_bodies: summary.defined_bodies,
                explanations: Vec::new(),
                issues: summary.issues,
                input_manifest: summary.input_manifest,
            });
        };
        let scope = owning_scope(&selected);
        let artifact = artifact_for(&graph, &meta.semantic_key, scope, &request.options.budgets)?;
        let site_store = storage::SqlSiteStore(&conn);
        artifact
            .explain(
                &selected,
                request.filter.as_ref(),
                &request.explanation,
                &site_store,
            )?
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
    fn update_builds_a_graph_and_queries_read_it_without_runs() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        assert!(storage::load_graph_meta(&conn).unwrap().is_some());
        let result = get_effect_summary_on(&conn, &request("TEST", "R")).unwrap();
        assert!(matches!(result.effects_status, EffectsStatus::Ready { .. }));
        assert_eq!(result.effects.len(), 1);
    }

    #[test]
    fn compact_preview_uses_prepared_row_without_loading_topology() {
        let (conn, _) = setup();
        seed(
            &conn,
            "EMPTY",
            "<p id=ordinary-anchor>No algorithm here.</p>",
        );
        let catalog = default_catalog(&[]).unwrap();
        let options = EffectsOptions::default();
        let meta = build_and_store_graph(&conn, &catalog, &options, Some(1)).unwrap();
        let graph = storage::load_graph(&conn).unwrap().unwrap();
        assert!(!graph
            .anchor_nodes
            .contains_key(&("EMPTY".into(), "ordinary-anchor".into())));
        let artifact =
            engine::analyze_graph(&graph, AnalysisScope::All, options.budgets.clone()).unwrap();
        store_compact_summaries(&conn, &graph, &artifact, &meta, &options.budgets).unwrap();
        let req = request("TEST", "R");
        let full = get_effect_summary_on(&conn, &req).unwrap();
        conn.execute("UPDATE effect_graph SET topology=x'00' WHERE id=1", [])
            .unwrap();
        let compact = get_cached_effect_preview_on(&conn, &req).unwrap().unwrap();
        assert_eq!(compact.effects, full.effects);
        assert_eq!(compact.effects_status, full.effects_status);
        assert_eq!(compact.defined_bodies, full.defined_bodies);
        assert!(compact.issues.is_empty());

        let empty = get_cached_effect_preview_on(&conn, &request("EMPTY", "ordinary-anchor"))
            .unwrap()
            .unwrap();
        assert!(empty.effects.is_empty());
        assert!(matches!(empty.effects_status, EffectsStatus::Ready { .. }));

        let mut other_budget = req.clone();
        other_budget.options.budgets.max_states += 1;
        assert!(get_cached_effect_preview_on(&conn, &other_budget)
            .unwrap()
            .is_none());
        storage::invalidate(&conn).unwrap();
        assert!(get_cached_effect_preview_on(&conn, &req).unwrap().is_none());
    }

    #[test]
    fn prepared_details_come_from_the_stored_graph() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        let details = prepared_effect_details_on(&conn, &request("TEST", "R")).unwrap();
        assert_eq!(details.explanations.len(), 1);
        assert_eq!(details.explanations[0].witnesses.len(), 1);
        assert!(details.explanations[0].witnesses_truncated);
    }

    #[test]
    fn stale_graph_is_unavailable_in_cached_mode() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        storage::invalidate(&conn).unwrap();
        let mut req = request("TEST", "R");
        req.options.mode = EffectsMode::Cached;
        assert!(matches!(
            get_effect_summary_on(&conn, &req).unwrap().effects_status,
            EffectsStatus::Unavailable { .. }
        ));
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
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
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
    fn source_replacement_invalidates_graph() {
        let (conn, root) = setup();
        let req = request("TEST", "R");
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        let before = get_effect_summary_on(&conn, &req).unwrap();
        assert!(matches!(before.effects_status, EffectsStatus::Ready { .. }));
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
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        let after = get_effect_summary_on(&conn, &req).unwrap();
        assert!(after.effects.is_empty());
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
        let catalog1 = default_catalog(&req.options.rule_paths).unwrap();
        build_and_store_graph(&conn, &catalog1, &req.options, Some(1)).unwrap();
        let before = get_effect_summary_on(&conn, &req).unwrap();

        std::fs::write(&path, yaml.replace("first", "second")).unwrap();
        let catalog2 = default_catalog(&req.options.rule_paths).unwrap();
        build_and_store_graph(&conn, &catalog2, &req.options, Some(1)).unwrap();
        let after = get_effect_summary_on(&conn, &req).unwrap();

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
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM effect_local_matches", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("CREATE TRIGGER reject_local_match_write BEFORE INSERT ON effect_local_matches BEGIN SELECT RAISE(ABORT,'local matches must be reused'); END;").unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
    }

    #[cfg(feature = "native")]
    #[test]
    fn parallel_local_matches_produce_identical_graphs() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        let options = request("TEST", "R").options;

        build_and_store_graph(&conn, &catalog, &options, Some(1)).unwrap();
        let meta1 = storage::load_graph_meta(&conn).unwrap().unwrap();

        conn.execute("DELETE FROM effect_local_matches", [])
            .unwrap();
        build_and_store_graph(&conn, &catalog, &options, Some(2)).unwrap();
        let meta2 = storage::load_graph_meta(&conn).unwrap().unwrap();

        assert_eq!(meta1.semantic_key, meta2.semantic_key);
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
        let catalog = default_catalog(&[]).unwrap();
        build_and_store_graph(&conn, &catalog, &EffectsOptions::default(), Some(1)).unwrap();
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
