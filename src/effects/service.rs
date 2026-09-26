//! Snapshot-consistent analysis orchestration, published summaries, and cache policy.
//!
//! Every trunk snapshot has one stored fragment. A publication links all
//! fragments, propagates effects over the whole graph and writes the summary
//! rows as a diff; it is current while its configuration and corpus identity
//! match the database.
use super::bundled::{bundled_catalog_digest, default_catalog};
use super::catalog::Catalog;
use super::compact::{propagate_compact, selector_key, summary_rows, CompactSummary};
use super::engine::{
    self, AnalysisArtifact, ArtifactSummary, Graph, IndexedAnchor, IssueId, SourceSpec,
};
use super::fragment::{
    build_fragment, decode_fragment, encode_fragment, Fragment, FragmentInput,
    FRAGMENT_FORMAT_VERSION,
};
use super::graph::ExecutionNode;
use super::link::link;
use super::model::*;
#[cfg(any(feature = "native", test))]
use crate::db;
use crate::db::effects::{self as storage, FragmentKey, Publication};
use crate::parse::steps::{StructuralSpec, STRUCTURE_VERSION};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

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

const REBUILD_COMMAND: &str = "webspec-index effects --all";

/// Stored effects do not match the indexed snapshots; they are never served.
fn stale_publication() -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisUnavailable,
        message: format!("snapshot_changed: effects are not current; run `{REBUILD_COMMAND}`"),
        details: Some(json!({"issue": "snapshot_changed", "command": REBUILD_COMMAND})),
    }
}

fn snapshot_changed() -> RequestError {
    RequestError {
        code: RequestErrorCode::AnalysisFailed,
        message: "snapshot_changed: corpus changed during both publication attempts".into(),
        details: Some(json!({"issue": "snapshot_changed"})),
    }
}

/// Site partition of the sites that `link` creates.
const LINK_PARTITION: &str = "#link";
const REGISTRY_RESOLUTION_VERSION: u32 = 1;

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

// ----- Keys -----

/// One trunk snapshot of the effects corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorpusEntry {
    pub spec: String,
    pub snapshot_id: i64,
    pub sha: String,
    pub indexed_at: String,
}

/// The trunk snapshots, sorted by spec.
pub fn corpus(conn: &Connection) -> Result<Vec<CorpusEntry>, RequestError> {
    let mut statement = conn
        .prepare_cached(
            "SELECT sp.name, sn.id, sn.sha, sn.indexed_at FROM specs sp
             JOIN snapshots sn ON sn.spec_id=sp.id
             WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' ORDER BY sp.name, sn.id",
        )
        .map_err(failure)?;
    let entries = statement
        .query_map([], |row| {
            Ok(CorpusEntry {
                spec: row.get(0)?,
                snapshot_id: row.get(1)?,
                sha: row.get(2)?,
                indexed_at: row.get(3)?,
            })
        })
        .map_err(failure)?
        .collect::<rusqlite::Result<_>>()
        .map_err(failure)?;
    Ok(entries)
}

fn digest_of(value: &Value) -> String {
    canonical_json_sha256(value).expect("strings and integers serialize")
}

pub fn corpus_digest(corpus: &[CorpusEntry]) -> String {
    digest_of(&Value::Array(
        corpus
            .iter()
            .map(|entry| json!([entry.spec, entry.snapshot_id, entry.sha, entry.indexed_at]))
            .collect(),
    ))
}

pub fn config_key(catalog_digest: &str, environment: &str) -> String {
    digest_of(&json!({
        "catalog": catalog_digest,
        "environment": environment,
        "engine": engine::ANALYSIS_ENGINE_VERSION,
        "structure": STRUCTURE_VERSION,
        "fragment": FRAGMENT_FORMAT_VERSION,
    }))
}

fn semantic_key(corpus: &[CorpusEntry], catalog_digest: &str, environment: &str) -> String {
    let mut specs: Vec<(&str, &str)> = corpus
        .iter()
        .map(|entry| (entry.spec.as_str(), entry.sha.as_str()))
        .collect();
    specs.sort_unstable();
    digest_of(&json!({
        "specs": specs,
        "representation": STRUCTURE_VERSION,
        "registry": REGISTRY_RESOLUTION_VERSION,
        "engine": engine::ANALYSIS_ENGINE_VERSION,
        "catalog": catalog_digest,
        "environment": environment,
    }))
}

fn analysis_id(publication: &Publication) -> String {
    format!("an_{}", &publication.semantic_key[..16])
}

fn is_current(publication: &Publication, config_key: &str, corpus: &[CorpusEntry]) -> bool {
    publication.config_key == config_key
        && (publication.frozen || publication.corpus_digest == corpus_digest(corpus))
}

/// The catalog digest for `rule_paths`; the bundled catalog's is computed without loading it.
pub fn catalog_digest(rule_paths: &[String]) -> Result<String, RequestError> {
    if rule_paths.is_empty() {
        bundled_catalog_digest()
    } else {
        Ok(default_catalog(rule_paths)?.content_digest)
    }
}

/// The publication, when it is current for this catalog and environment.
fn current_publication(
    conn: &Connection,
    catalog_digest: &str,
    environment: &str,
) -> Result<Option<Publication>, RequestError> {
    let Some(publication) = storage::load_publication(conn).map_err(failure)? else {
        return Ok(None);
    };
    let current = is_current(
        &publication,
        &config_key(catalog_digest, environment),
        &corpus(conn)?,
    );
    Ok(current.then_some(publication))
}

pub fn publication_is_current(
    conn: &Connection,
    catalog_digest: &str,
    environment: &str,
) -> Result<bool, RequestError> {
    Ok(current_publication(conn, catalog_digest, environment)?.is_some())
}

#[cfg(feature = "native")]
pub fn input_fingerprint(options: &EffectsOptions) -> Result<String, RequestError> {
    options.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let digest = catalog_digest(&options.rule_paths)?;
    let tx = conn.unchecked_transaction().map_err(failure)?;
    // A current publication and a stale one must fingerprint differently: an LSP
    // that cached an unavailable result has to notice when effects get published.
    Ok(
        match current_publication(&tx, &digest, &options.environment)? {
            Some(publication) => format!("{}:current", publication.semantic_key),
            None => format!(
                "{}:stale",
                semantic_key(&corpus(&tx)?, &digest, &options.environment)
            ),
        },
    )
}

/// Whether each corpus entry's stored fragment is missing or stale.
fn stale_entries<'c>(
    corpus: &'c [CorpusEntry],
    keys: &[FragmentKey],
    config_key: &str,
) -> Vec<&'c CorpusEntry> {
    let fresh: HashSet<(i64, &str)> = keys
        .iter()
        .filter(|key| key.config_key == config_key)
        .map(|key| (key.snapshot_id, key.indexed_at.as_str()))
        .collect();
    corpus
        .iter()
        .filter(|entry| !fresh.contains(&(entry.snapshot_id, entry.indexed_at.as_str())))
        .collect()
}

/// Raw structure sizes of the specs whose fragments a publish would rebuild.
pub fn stale_structure_bytes(
    conn: &Connection,
    catalog_digest: &str,
    environment: &str,
) -> Result<Vec<u64>, RequestError> {
    let entries = corpus(conn)?;
    let keys = storage::fragment_keys(conn).map_err(failure)?;
    stale_entries(&entries, &keys, &config_key(catalog_digest, environment))
        .into_iter()
        .map(|entry| {
            Ok(storage::structure_bytes(conn, entry.snapshot_id)
                .map_err(failure)?
                .unwrap_or(0))
        })
        .collect()
}

// ----- Fragment inputs -----

/// `spec#anchor` identities whose section text the catalog reviews.
fn reviewed_anchors(catalog: &Catalog) -> BTreeSet<String> {
    catalog
        .summaries()
        .map(|s| s.subject.as_identity())
        .chain(
            catalog
                .implementations()
                .map(|i| i.implementation.as_identity()),
        )
        .collect()
}

fn indexed_anchor(
    base_url: &str,
    anchor: String,
    text: String,
    idl_kind: Option<String>,
) -> IndexedAnchor {
    IndexedAnchor {
        url: format!("{}#{anchor}", base_url.trim_end_matches('#')),
        anchor,
        text,
        idl_kind,
    }
}

/// The anchor list of a freshly parsed spec, equal to what [`load_sources`]
/// reads back once the parse is stored.
#[cfg(feature = "native")]
pub(crate) fn parsed_anchors(
    spec: &str,
    base_url: &str,
    parsed: &crate::model::ParsedSpec,
    structure: &StructuralSpec,
    catalog: &Catalog,
) -> Vec<IndexedAnchor> {
    let reviewed = reviewed_anchors(catalog);
    let mut texts: HashMap<&str, Option<&str>> = HashMap::new();
    for section in &parsed.sections {
        texts
            .entry(section.anchor.as_str())
            .or_insert(section.content_text.as_deref());
    }
    let mut idl_kinds: HashMap<&str, &str> = HashMap::new();
    for definition in &parsed.idl_definitions {
        idl_kinds
            .entry(definition.anchor.as_str())
            .and_modify(|kind| *kind = (*kind).min(definition.kind.as_str()))
            .or_insert(definition.kind.as_str());
    }
    let anchors: BTreeSet<&str> = texts
        .keys()
        .copied()
        .chain(structure.anchors.iter().map(String::as_str))
        .chain(parsed.references.iter().map(|r| r.from_anchor.as_str()))
        .chain(idl_kinds.keys().copied())
        .collect();
    anchors
        .into_iter()
        .map(|anchor| {
            let text = if reviewed.contains(&format!("{spec}#{anchor}")) {
                texts
                    .get(anchor)
                    .copied()
                    .flatten()
                    .unwrap_or_default()
                    .to_owned()
            } else {
                String::new()
            };
            indexed_anchor(
                base_url,
                anchor.to_owned(),
                text,
                idl_kinds.get(anchor).map(|kind| (*kind).to_owned()),
            )
        })
        .collect()
}

/// Structure and anchor list of the corpus snapshots in `snapshot_ids`.
fn load_sources(
    conn: &Connection,
    catalog: &Catalog,
    snapshot_ids: &BTreeSet<i64>,
) -> Result<Vec<SourceSpec>, RequestError> {
    let mut statement = conn.prepare("SELECT sp.name,sn.sha,sp.base_url,sn.id FROM specs sp JOIN snapshots sn ON sn.spec_id=sp.id
        WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' ORDER BY sp.name,sn.sha").map_err(failure)?;
    let rows: Vec<(String, String, String, i64)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(failure)?
        .collect::<rusqlite::Result<_>>()
        .map_err(failure)?;
    let reviewed = reviewed_anchors(catalog);
    let mut anchor_statement = conn
        .prepare(
            "WITH anchors(anchor) AS (
            SELECT anchor FROM sections WHERE snapshot_id=?1
            UNION SELECT anchor FROM effect_anchors WHERE snapshot_id=?1
            UNION SELECT from_anchor FROM refs WHERE snapshot_id=?1
            UNION SELECT anchor FROM idl_defs WHERE snapshot_id=?1)
            SELECT anchors.anchor, sections.content_text,
                (SELECT MIN(kind) FROM idl_defs
                 WHERE snapshot_id=?1 AND anchor=anchors.anchor) AS idl_kind
            FROM anchors
            LEFT JOIN sections ON sections.snapshot_id=?1 AND sections.anchor=anchors.anchor
            ORDER BY anchors.anchor",
        )
        .map_err(failure)?;
    let mut sources = Vec::new();
    for (spec, snapshot_sha, base_url, snapshot_id) in rows {
        if !snapshot_ids.contains(&snapshot_id) {
            continue;
        }
        let structure: Option<StructuralSpec> =
            storage::load_structure(conn, snapshot_id, STRUCTURE_VERSION)
                .map_err(failure)?
                .map(|text| serde_json::from_str(&text))
                .transpose()
                .map_err(failure)?;
        let anchors = anchor_statement
            .query_map([snapshot_id], |row| {
                let anchor: String = row.get(0)?;
                let text = if reviewed.contains(&format!("{spec}#{anchor}")) {
                    row.get::<_, Option<String>>(1)?.unwrap_or_default()
                } else {
                    String::new()
                };
                Ok(indexed_anchor(&base_url, anchor, text, row.get(2)?))
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

/// Map `items` on the effects thread pool.
///
/// `threads` overrides the thread count; `None` reads `WEBSPEC_EFFECTS_THREADS`
/// and falls back to [`std::thread::available_parallelism`]. Without the
/// `native` feature this is a serial loop, so wasm and lib-only builds are
/// unaffected.
fn par_map<T: Sync, U: Send>(
    items: &[T],
    threads: Option<usize>,
    map: impl Fn(&T) -> U + Sync + Send,
) -> Vec<U> {
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
        pool.install(|| items.par_iter().map(map).collect())
    }
    #[cfg(not(feature = "native"))]
    {
        let _ = threads;
        items.iter().map(map).collect()
    }
}

fn build_fragments(
    sources: &[SourceSpec],
    catalog: &Catalog,
    environment: &str,
    threads: Option<usize>,
) -> Vec<Fragment> {
    par_map(sources, threads, |source| {
        build_fragment(&FragmentInput::for_source(source, catalog, environment))
    })
}

fn decode_fragments(payloads: &[(String, Vec<u8>)]) -> Result<Vec<Fragment>, RequestError> {
    par_map(payloads, None, |(spec, payload)| {
        decode_fragment(payload).map_err(|e| failure(format!("fragment of {spec}: {e}")))
    })
    .into_iter()
    .collect()
}

// ----- Publication -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishMode {
    /// Rebuild only missing and stale fragments.
    Incremental,
    /// Rebuild every fragment.
    Rebuild,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct PublishReport {
    pub fragments_built: Vec<String>,
    pub rows_upserted: usize,
    pub rows_deleted: usize,
    pub published: bool,
}

/// The issue attached to anchor nodes that have no reusable algorithm body and
/// nothing else to show: summaries of indexed anchors outside the graph carry it.
fn opaque_anchor_issue(graph: &Graph) -> Option<IssueId> {
    const MAGIC_PREFIX: &str = "indexed anchor has no reusable algorithm body";
    let edge_endpoints: HashSet<&str> = graph
        .edges
        .values()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();
    let occurrence_subjects: HashSet<&str> = graph
        .occurrences
        .values()
        .map(|o| o.subject_id.as_str())
        .collect();
    graph
        .nodes
        .keys()
        .filter(|id| id.starts_with("anchor:"))
        .filter(|id| {
            !edge_endpoints.contains(id.as_str()) && !occurrence_subjects.contains(id.as_str())
        })
        .find_map(|id| match graph.issues.get(id).map(Vec::as_slice) {
            Some(&[issue_id]) => graph
                .issue_catalog
                .get(issue_id as usize)
                .filter(|issue| {
                    issue.code == IssueCode::UnsupportedStructure
                        && issue.message.starts_with(MAGIC_PREFIX)
                })
                .map(|_| issue_id),
            _ => None,
        })
}

fn manifest(
    corpus: &[CorpusEntry],
    catalog_digest: &str,
    options: &EffectsOptions,
    missing_inputs: Vec<MissingInput>,
) -> Result<InputManifest, RequestError> {
    let mut specs: Vec<ManifestSpec> = corpus
        .iter()
        .map(|entry| ManifestSpec {
            spec: entry.spec.clone(),
            snapshot_sha: entry.sha.clone(),
        })
        .collect();
    specs.sort_by(|a, b| (&a.spec, &a.snapshot_sha).cmp(&(&b.spec, &b.snapshot_sha)));
    Ok(InputManifest {
        specs,
        structural_representation_version: STRUCTURE_VERSION.parse().map_err(failure)?,
        registry_resolution_version: REGISTRY_RESOLUTION_VERSION,
        environment: options.environment.clone(),
        catalog_digest: catalog_digest.to_owned(),
        analysis_engine_version: engine::ANALYSIS_ENGINE_VERSION,
        scope: AnalysisScope::All,
        budget_profile: options.budgets.clone(),
        missing_inputs,
    })
}

fn row_digest(payload: &str) -> Vec<u8> {
    Sha256::digest(payload.as_bytes())[..16].to_vec()
}

/// Bring the stored fragments and the publication up to date with the corpus.
///
/// Fragments in `prebuilt` (by spec) replace a rebuild when their snapshot sha
/// is the corpus one.
pub fn publish(
    conn: &Connection,
    catalog: &Catalog,
    options: &EffectsOptions,
    mode: PublishMode,
    mut prebuilt: BTreeMap<String, Fragment>,
) -> Result<PublishReport, RequestError> {
    options.validate()?;
    let config = config_key(&catalog.content_digest, &options.environment);
    for _ in 0..2 {
        if let Some(report) = publish_once(conn, catalog, options, &config, mode, &mut prebuilt)? {
            return Ok(report);
        }
    }
    Err(snapshot_changed())
}

/// One publish attempt; `None` when the corpus moved before the write.
fn publish_once(
    conn: &Connection,
    catalog: &Catalog,
    options: &EffectsOptions,
    config: &str,
    mode: PublishMode,
    prebuilt: &mut BTreeMap<String, Fragment>,
) -> Result<Option<PublishReport>, RequestError> {
    let (entries, stale, fresh, digests, sources, site_specs) = {
        let tx = conn.unchecked_transaction().map_err(failure)?;
        let entries = corpus(&tx)?;
        let keys = storage::fragment_keys(&tx).map_err(failure)?;
        let stale: Vec<CorpusEntry> = match mode {
            PublishMode::Incremental => stale_entries(&entries, &keys, config)
                .into_iter()
                .cloned()
                .collect(),
            PublishMode::Rebuild => entries.clone(),
        };
        let publication = storage::load_publication(&tx).map_err(failure)?;
        if stale.is_empty()
            && publication.is_some_and(|publication| is_current(&publication, config, &entries))
        {
            return Ok(Some(PublishReport::default()));
        }
        let stale_specs: HashSet<&str> = stale.iter().map(|entry| entry.spec.as_str()).collect();
        let fresh: Vec<(String, Vec<u8>)> = if stale.len() == entries.len() {
            Vec::new()
        } else {
            storage::load_fragment_payloads(&tx, config)
                .map_err(failure)?
                .into_iter()
                .filter(|(spec, _)| !stale_specs.contains(spec.as_str()))
                .collect()
        };
        let needed: BTreeSet<i64> = stale
            .iter()
            .filter(|entry| {
                prebuilt
                    .get(&entry.spec)
                    .is_none_or(|fragment| fragment.snapshot_sha != entry.sha)
            })
            .map(|entry| entry.snapshot_id)
            .collect();
        let sources = load_sources(&tx, catalog, &needed)?;
        let digests = storage::summary_digests(&tx).map_err(failure)?;
        let site_specs = storage::site_partition_specs(&tx).map_err(failure)?;
        (entries, stale, fresh, digests, sources, site_specs)
    };

    let mut built: BTreeMap<String, Fragment> =
        build_fragments(&sources, catalog, &options.environment, None)
            .into_iter()
            .map(|fragment| (fragment.spec.clone(), fragment))
            .collect();
    for entry in &stale {
        if !built.contains_key(&entry.spec) {
            if let Some(fragment) = prebuilt.remove(&entry.spec) {
                built.insert(entry.spec.clone(), fragment);
            }
        }
    }
    let built: Vec<(&CorpusEntry, Fragment)> = stale
        .iter()
        .filter_map(|entry| built.remove(&entry.spec).map(|fragment| (entry, fragment)))
        .collect();
    let payloads = par_map(&built, None, |(_, fragment)| encode_fragment(fragment));

    let mut fragments = decode_fragments(&fresh)?;
    fragments.extend(built.iter().map(|(_, fragment)| fragment.clone()));
    fragments.sort_by(|left, right| left.spec.cmp(&right.spec));
    let linked = link(&fragments, catalog, &options.environment)?;
    let graph = linked.graph;

    let mut upserts = Vec::new();
    let mut published_keys = HashSet::new();
    for row in summary_rows(&graph, &propagate_compact(&graph))? {
        let digest = row_digest(&row.payload);
        if digests.get(&row.subject_key) != Some(&digest) {
            upserts.push((
                row.subject_key.clone(),
                row.spec,
                digest,
                storage::encode_payload(&row.payload),
            ));
        }
        published_keys.insert(row.subject_key);
    }
    let deletes: Vec<String> = digests
        .into_keys()
        .filter(|key| !published_keys.contains(key))
        .collect();

    let site_owner: HashMap<&str, &str> = fragments
        .iter()
        .flat_map(|fragment| {
            fragment
                .sites
                .keys()
                .map(|key| (key.as_str(), fragment.spec.as_str()))
        })
        .collect();
    let mut sites = BTreeMap::new();
    for (_, fragment) in &built {
        sites.extend(fragment.sites.iter().map(|(k, s)| (k.clone(), s.clone())));
    }
    for (key, site) in &graph.sites {
        if !site_owner.contains_key(key.as_str()) {
            sites.insert(key.clone(), site.clone());
        }
    }
    let mut partitions: Vec<String> = built.iter().map(|(entry, _)| entry.spec.clone()).collect();
    partitions.push(LINK_PARTITION.into());
    let indexed: HashSet<&str> = entries.iter().map(|entry| entry.spec.as_str()).collect();
    partitions.extend(
        site_specs
            .into_iter()
            .filter(|spec| spec != LINK_PARTITION && !indexed.contains(spec.as_str())),
    );

    let publication = Publication {
        config_key: config.to_owned(),
        budget_key: canonical_json(&serde_json::to_value(&options.budgets).map_err(failure)?)
            .map_err(failure)?,
        corpus_digest: corpus_digest(&entries),
        semantic_key: semantic_key(&entries, &catalog.content_digest, &options.environment),
        manifest_json: serde_json::to_string(&manifest(
            &entries,
            &catalog.content_digest,
            options,
            linked.missing_inputs,
        )?)
        .map_err(failure)?,
        opaque_anchor_issue: opaque_anchor_issue(&graph),
        frozen: false,
    };

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(failure)?;
    let now = corpus(&tx)?;
    let current: HashSet<(i64, &str)> = now
        .iter()
        .map(|entry| (entry.snapshot_id, entry.indexed_at.as_str()))
        .collect();
    for ((entry, _), payload) in built.iter().zip(&payloads) {
        if current.contains(&(entry.snapshot_id, entry.indexed_at.as_str())) {
            let key = FragmentKey {
                snapshot_id: entry.snapshot_id,
                spec: entry.spec.clone(),
                indexed_at: entry.indexed_at.clone(),
                config_key: config.to_owned(),
            };
            storage::store_fragment(&tx, &key, payload).map_err(failure)?;
        }
    }
    if now != entries {
        tx.commit().map_err(failure)?;
        return Ok(None);
    }
    storage::replace_site_partitions(&tx, &partitions, &sites, |key| {
        site_owner
            .get(key)
            .copied()
            .unwrap_or(LINK_PARTITION)
            .to_owned()
    })
    .map_err(failure)?;
    storage::apply_summary_diff(&tx, &upserts, &deletes).map_err(failure)?;
    storage::store_publication(&tx, &publication).map_err(failure)?;
    storage::store_spec_deps(&tx, &linked.spec_deps).map_err(failure)?;
    storage::delete_orphan_fragments(&tx).map_err(failure)?;
    tx.commit().map_err(failure)?;

    ANALYSES.with(|cell| cell.borrow_mut().clear());
    LINKED.with(|cell| {
        *cell.borrow_mut() = Some(LinkedGraph {
            key: linked_key(&publication),
            graph: Rc::new(graph),
        })
    });
    Ok(Some(PublishReport {
        fragments_built: built.iter().map(|(entry, _)| entry.spec.clone()).collect(),
        rows_upserted: upserts.len(),
        rows_deleted: deletes.len(),
        published: true,
    }))
}

#[cfg(feature = "native")]
fn publish_effects(
    request: &RecomputeEffectsRequest,
    mode: PublishMode,
) -> Result<RecomputeEffectsResult, RequestError> {
    request.validate()?;
    let conn = db::open_or_create_db().map_err(failure)?;
    let catalog = default_catalog(&request.options.rule_paths)?;
    publish(&conn, &catalog, &request.options, mode, BTreeMap::new())?;
    let publication = storage::load_publication(&conn)
        .map_err(failure)?
        .ok_or_else(|| failure("publication was not stored"))?;
    let graph = linked_graph(
        &conn,
        &publication,
        Some(&catalog),
        &request.options.rule_paths,
    )?;
    let issues = graph
        .issue_catalog
        .iter()
        .take(200)
        .map(|issue| Issue {
            code: issue.code,
            message: issue.message.clone(),
            site: issue
                .site_key
                .as_ref()
                .and_then(|key| graph.sites.get(key).cloned()),
        })
        .collect();
    Ok(RecomputeEffectsResult {
        schema_version: EFFECTS_SCHEMA_VERSION,
        input_manifest: serde_json::from_str(&publication.manifest_json).map_err(failure)?,
        body_count: graph.nodes.values().filter(|n| n.is_body).count() as u64,
        relationship_count: graph.edges.len() as u64,
        issues,
        issue_count: graph.issue_catalog.len() as u64,
    })
}

/// Publish incrementally: rebuild only missing and stale fragments.
#[cfg(feature = "native")]
pub fn recompute_effects(
    request: &RecomputeEffectsRequest,
) -> Result<RecomputeEffectsResult, RequestError> {
    publish_effects(request, PublishMode::Incremental)
}

/// Publish with every fragment built from scratch.
#[cfg(feature = "native")]
pub fn rebuild_effects(
    request: &RecomputeEffectsRequest,
) -> Result<RecomputeEffectsResult, RequestError> {
    publish_effects(request, PublishMode::Rebuild)
}

// ----- Thread-local caches -----

struct LinkedGraph {
    key: (String, String),
    graph: Rc<Graph>,
}

type AnalysisKey = (String, String, String); // (semantic_key, scope_json, budgets_json)
const ANALYSIS_CACHE_CAPACITY: usize = 8;

thread_local! {
    static LINKED: RefCell<Option<LinkedGraph>> = const { RefCell::new(None) };
    static ANALYSES: RefCell<VecDeque<(AnalysisKey, Rc<AnalysisArtifact>)>> = const { RefCell::new(VecDeque::new()) };
}

fn linked_key(publication: &Publication) -> (String, String) {
    (
        publication.semantic_key.clone(),
        publication.corpus_digest.clone(),
    )
}

/// The publication's fragments linked into one graph, once per publication.
fn linked_graph(
    conn: &Connection,
    publication: &Publication,
    catalog: Option<&Catalog>,
    rule_paths: &[String],
) -> Result<Rc<Graph>, RequestError> {
    let key = linked_key(publication);
    let cached = LINKED.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|linked| linked.key == key)
            .map(|linked| Rc::clone(&linked.graph))
    });
    if let Some(graph) = cached {
        return Ok(graph);
    }
    let loaded;
    let catalog = match catalog {
        Some(catalog) => catalog,
        None => {
            loaded = default_catalog(rule_paths)?;
            &loaded
        }
    };
    let manifest: InputManifest =
        serde_json::from_str(&publication.manifest_json).map_err(failure)?;
    let payloads =
        storage::load_fragment_payloads(conn, &publication.config_key).map_err(failure)?;
    let fragments = decode_fragments(&payloads)?;
    let graph = Rc::new(link(&fragments, catalog, &manifest.environment)?.graph);
    LINKED.with(|cell| {
        *cell.borrow_mut() = Some(LinkedGraph {
            key,
            graph: Rc::clone(&graph),
        })
    });
    ANALYSES.with(|cell| cell.borrow_mut().clear());
    Ok(graph)
}

/// The linked graph of the current publication. A stale publication is never
/// served: `Cached` and `Off` get `None`, `Auto` gets `snapshot_changed`.
fn graph_for(
    conn: &Connection,
    options: &EffectsOptions,
) -> Result<Option<(Publication, Rc<Graph>)>, RequestError> {
    let digest = catalog_digest(&options.rule_paths)?;
    match current_publication(conn, &digest, &options.environment)? {
        Some(publication) => {
            let graph = linked_graph(conn, &publication, None, &options.rule_paths)?;
            Ok(Some((publication, graph)))
        }
        None => match options.mode {
            EffectsMode::Auto => Err(stale_publication()),
            EffectsMode::Cached | EffectsMode::Off => Ok(None),
        },
    }
}

fn issue_codes(issues: &[Issue]) -> Vec<IssueCode> {
    issues
        .iter()
        .map(|issue| issue.code)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

fn artifact_for(
    graph: &Rc<Graph>,
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
    graph: &Graph,
    opaque_anchor_issue: Option<IssueId>,
    spec: &str,
    anchor: &str,
    snapshot_sha: &str,
) -> Graph {
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
    if let Some(issue_id) = opaque_anchor_issue {
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
    let digest = catalog_digest(&request.options.rule_paths)?;
    let Some(publication) = current_publication(conn, &digest, &request.options.environment)?
    else {
        return Ok(None);
    };
    let manifest: InputManifest =
        serde_json::from_str(&publication.manifest_json).map_err(failure)?;
    let id = analysis_id(&publication);
    let Some(payload) = storage::load_summary(conn, &selector_key(&selector)?).map_err(failure)?
    else {
        // An absent key is an indexed anchor outside the graph.
        if selector.step_id.is_some() || selector.step_path.is_some() || selector.body_id.is_some()
        {
            return Ok(None);
        }
        let issue_codes = if publication.opaque_anchor_issue.is_some() {
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
        return Ok(Some(compact_envelope(compact, manifest, &id)));
    };
    let compact: CompactSummary = serde_json::from_str(&payload).map_err(failure)?;
    if compact.subject != subject {
        return Ok(None);
    }
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
    let Some((publication, graph)) = graph_for(conn, &request.options)? else {
        return Ok(no_result(subject, false));
    };
    let m: InputManifest = serde_json::from_str(&publication.manifest_json).map_err(failure)?;
    let analysis_id = analysis_id(&publication);

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
            publication.opaque_anchor_issue,
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
        artifact_for(
            &graph,
            &publication.semantic_key,
            scope,
            &request.options.budgets,
        )?
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
    let Some((publication, graph)) = graph_for(conn, &request.options)? else {
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
            publication.opaque_anchor_issue,
            &summary.subject.spec,
            &summary.subject.anchor,
            &summary.subject.snapshot_sha,
        ))
    } else {
        return Err(unavailable("Prepared paths are unavailable. Run webspec-index effects --all --summary-only to prepare them."));
    };

    let scope = owning_scope(&selected);
    let artifact = if has_node {
        artifact_for(
            &graph,
            &publication.semantic_key,
            scope,
            &request.options.budgets,
        )?
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
    let conn = db::open_or_create_db().map_err(failure)?;
    explain_effects_on(&conn, request)
}

pub fn explain_effects_on(
    conn: &Connection,
    request: &ExplainEffectsRequest,
) -> Result<ExplainEffectsResult, RequestError> {
    request.validate()?;
    let summary = get_effect_summary_on(
        conn,
        &EffectsRequest {
            schema_version: request.schema_version,
            subject: request.subject.clone(),
            options: request.options.clone(),
            filter: request.filter.clone(),
        },
    )?;
    let explanations = if summary.effects.is_empty() {
        Vec::new()
    } else if let EffectsStatus::Ready { .. } = &summary.effects_status {
        let mut selected = request.subject.clone();
        selected.spec = summary.subject.spec.clone();
        let Some((publication, graph)) = graph_for(conn, &request.options)? else {
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
        let artifact = artifact_for(
            &graph,
            &publication.semantic_key,
            scope,
            &request.options.budgets,
        )?;
        let site_store = storage::SqlSiteStore(conn);
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

    fn publish_all(conn: &Connection, catalog: &Catalog, options: &EffectsOptions) {
        publish(
            conn,
            catalog,
            options,
            PublishMode::Incremental,
            BTreeMap::new(),
        )
        .unwrap();
    }

    /// Moves the corpus identity without changing content.
    fn touch(conn: &Connection) {
        conn.execute("UPDATE snapshots SET indexed_at = indexed_at || '+'", [])
            .unwrap();
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
        publish_all(&conn, &catalog, &EffectsOptions::default());
        assert!(storage::load_publication(&conn).unwrap().is_some());
        let result = get_effect_summary_on(&conn, &request("TEST", "R")).unwrap();
        assert!(matches!(result.effects_status, EffectsStatus::Ready { .. }));
        assert_eq!(result.effects.len(), 1);
    }

    #[test]
    fn compact_preview_uses_prepared_row_without_linking() {
        let (conn, _) = setup();
        seed(
            &conn,
            "EMPTY",
            "<p id=ordinary-anchor>No algorithm here.</p>",
        );
        let catalog = default_catalog(&[]).unwrap();
        let options = EffectsOptions::default();
        publish_all(&conn, &catalog, &options);
        let req = request("TEST", "R");
        let full = get_effect_summary_on(&conn, &req).unwrap();
        conn.execute("UPDATE effect_fragments SET payload=x'00'", [])
            .unwrap();
        LINKED.with(|cell| cell.borrow_mut().take());
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

        touch(&conn);
        assert!(get_cached_effect_preview_on(&conn, &req).unwrap().is_none());
    }

    #[test]
    fn on_demand_explanation_returns_multiple_witness_examples() {
        let conn = db::open_test_db().unwrap();
        seed(
            &conn,
            "DOM",
            "<p>To <dfn id=concept-event-fire>fire an event</dfn>, dispatch it.</p>",
        );
        seed(
            &conn,
            "TEST",
            "<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol>
             <li><a href='https://dom.spec.whatwg.org/#concept-event-fire'>Fire an event</a> named <code>hello</code>.</li>
             <li><a href='https://dom.spec.whatwg.org/#concept-event-fire'>Fire an event</a> named <code>hello</code>.</li>
             </ol></div>",
        );
        let options = EffectsOptions::default();
        publish_all(&conn, &default_catalog(&[]).unwrap(), &options);
        let subject = request("TEST", "R").subject;
        let summary = get_effect_summary_on(&conn, &request("TEST", "R")).unwrap();
        assert_eq!(summary.effects.len(), 1);
        let effect_id = summary.effects[0].id.clone();
        let make_request = |limit| ExplainEffectsRequest {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: subject.clone(),
            options: EffectsOptions {
                mode: EffectsMode::Cached,
                ..EffectsOptions::default()
            },
            filter: Some(EffectFilter {
                effect_id: Some(effect_id.clone()),
                ..EffectFilter::default()
            }),
            explanation: ExplanationOptions {
                limit,
                ..ExplanationOptions::default()
            },
        };
        let one = explain_effects_on(&conn, &make_request(1)).unwrap();
        let more = explain_effects_on(&conn, &make_request(8)).unwrap();
        assert_eq!(one.explanations[0].witnesses.len(), 1);
        assert!(one.explanations[0].witnesses_truncated);
        assert_eq!(more.explanations[0].witnesses.len(), 2);
        assert!(!more.explanations[0].witnesses_truncated);
        assert_eq!(more.effects, summary.effects);

        let response: serde_json::Value = serde_json::from_str(&crate::api::handle_json(
            &conn,
            &json!({
                "type": "effects_paths",
                "subject": {"spec": "TEST", "anchor": "R"},
                "effect_id": effect_id,
                "limit": 8
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(response["type"], "effects_paths", "{response}");
        assert_eq!(response["result"]["effects"].as_array().unwrap().len(), 1);
        assert_eq!(
            response["result"]["explanations"][0]["witnesses"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        for limit in [0, 129] {
            let response: serde_json::Value = serde_json::from_str(&crate::api::handle_json(
                &conn,
                &json!({
                    "type": "effects_paths",
                    "subject": {"spec": "TEST", "anchor": "R"},
                    "effect_id": summary.effects[0].id,
                    "limit": limit
                })
                .to_string(),
            ))
            .unwrap();
            assert_eq!(response["type"], "error", "{response}");
            assert_eq!(response["code"], "invalid_request", "{response}");
        }
    }

    #[test]
    fn aoid_call_propagates_callee_effect_to_caller() {
        let conn = db::open_test_db().unwrap();
        seed(
            &conn,
            "DOM",
            "<p>To <dfn id=concept-event-fire>fire an event</dfn>, dispatch it.</p>",
        );
        seed(
            &conn,
            "TEST",
            "<emu-clause id=caller><h1>Caller</h1><emu-alg><ol><li>Run <emu-xref aoid=Called>Called</emu-xref>.</li></ol></emu-alg></emu-clause>
             <emu-clause id=callee aoid=Called><h1>Called</h1><emu-alg><ol><li><a href='https://dom.spec.whatwg.org/#concept-event-fire'>Fire an event</a> named <code>hello</code>.</li></ol></emu-alg></emu-clause>",
        );
        publish_all(
            &conn,
            &default_catalog(&[]).unwrap(),
            &EffectsOptions::default(),
        );
        let summary = get_effect_summary_on(&conn, &request("TEST", "caller")).unwrap();
        assert!(
            summary
                .effects
                .iter()
                .any(|effect| effect.kind == "event.fire"),
            "{summary:?}"
        );
    }

    #[test]
    fn prepared_details_come_from_the_stored_graph() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        publish_all(&conn, &catalog, &EffectsOptions::default());
        let details = prepared_effect_details_on(&conn, &request("TEST", "R")).unwrap();
        assert_eq!(details.explanations.len(), 1);
        assert_eq!(details.explanations[0].witnesses.len(), 1);
        assert!(details.explanations[0].witnesses_truncated);
    }

    #[test]
    fn stale_graph_is_unavailable_in_cached_mode() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        publish_all(&conn, &catalog, &EffectsOptions::default());
        touch(&conn);
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
        let ids: BTreeSet<i64> = corpus(&conn)
            .unwrap()
            .iter()
            .map(|entry| entry.snapshot_id)
            .collect();
        let sources = load_sources(&conn, &catalog, &ids).unwrap();
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
        publish_all(&conn, &catalog, &EffectsOptions::default());
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
        publish_all(&conn, &catalog, &EffectsOptions::default());
        let before = get_effect_summary_on(&conn, &req).unwrap();
        assert!(matches!(before.effects_status, EffectsStatus::Ready { .. }));
        let replacement = crate::parse::steps::extract_step_structure(
            "<div class=algorithm><p>To <dfn id=R>R</dfn>:</p><ol><li>Return.</li></ol></div>",
            "TEST",
            "https://test.example/",
            "hash:replacement",
        );
        conn.execute(
            "UPDATE snapshots SET sha='hash:replacement', indexed_at='later' WHERE id=?1",
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
        publish_all(&conn, &catalog, &EffectsOptions::default());
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
        publish_all(&conn, &catalog1, &req.options);
        let before = get_effect_summary_on(&conn, &req).unwrap();

        std::fs::write(&path, yaml.replace("first", "second")).unwrap();
        let catalog2 = default_catalog(&req.options.rule_paths).unwrap();
        publish_all(&conn, &catalog2, &req.options);
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

    #[cfg(feature = "native")]
    #[test]
    fn parallel_fragments_are_identical() {
        let (conn, _) = setup();
        let catalog = default_catalog(&[]).unwrap();
        let ids: BTreeSet<i64> = corpus(&conn)
            .unwrap()
            .iter()
            .map(|entry| entry.snapshot_id)
            .collect();
        let sources = load_sources(&conn, &catalog, &ids).unwrap();
        assert_eq!(
            build_fragments(&sources, &catalog, "web", Some(1)),
            build_fragments(&sources, &catalog, "web", Some(4))
        );
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
        publish_all(&conn, &catalog, &EffectsOptions::default());
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

    #[cfg(feature = "native")]
    mod publication {
        use super::*;

        const DOM_CALLER_ANCHOR: &str = "a-run";
        const BETA: &str = include_str!("../../tests/fixtures/effects/multi/beta.html");
        const MULTI: [(&str, &str, &str); 3] = [
            (
                "DOM",
                "https://dom.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/alpha.html"),
            ),
            ("INFRA", "https://infra.spec.whatwg.org/", BETA),
            (
                "URL",
                "https://url.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/multi/gamma.html"),
            ),
        ];

        fn infra_edited() -> String {
            BETA.replace("Return a new object.", "Return a new, empty object.")
        }

        fn infra_with_renamed_algorithm() -> String {
            BETA.replace("id=\"b-algo\"", "id=\"b-algo-renamed\"")
        }

        fn corpus_db(specs: &[(&str, &str, &str)]) -> Connection {
            let conn = db::open_test_db().unwrap();
            for (spec, base, html) in specs {
                crate::fetch::index_html(&conn, spec, base, "whatwg", html.to_string()).unwrap();
            }
            conn
        }

        fn reindex(conn: &Connection, spec: &str, html: &str) {
            let (_, base, _) = MULTI.iter().find(|(name, _, _)| *name == spec).unwrap();
            crate::fetch::index_html(conn, spec, base, "whatwg", html.to_owned()).unwrap();
        }

        /// The bundled catalog plus a reviewed summary on INFRA's `b-algo`, so
        /// DOM's caller has an effect that depends on the callee.
        fn fixture_catalog_and_options() -> (Catalog, EffectsOptions) {
            static RULES: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
            let rules = RULES.get_or_init(|| {
                let dir = tempfile::tempdir().unwrap();
                std::fs::write(
                    dir.path().join("multi.yaml"),
                    "schema: 1\npackage: multi\nsummaries:\n  - id: beta\n    subject: INFRA#b-algo\n    expect_text: '(?i)Run alpha'\n    reason: fixture declaration\n    emit:\n      kind: script.opportunity\n      params: {}\n",
                )
                .unwrap();
                dir
            });
            let options = EffectsOptions {
                rule_paths: vec![rules.path().to_string_lossy().into()],
                ..EffectsOptions::default()
            };
            (default_catalog(&options.rule_paths).unwrap(), options)
        }

        fn rows(conn: &Connection) -> BTreeMap<String, String> {
            let mut statement = conn
                .prepare("SELECT subject_key FROM effect_summaries ORDER BY subject_key")
                .unwrap();
            let keys: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            keys.into_iter()
                .map(|key| {
                    let row = storage::load_summary(conn, &key).unwrap().unwrap();
                    (key, row)
                })
                .collect()
        }

        fn fragments(conn: &Connection) -> BTreeMap<String, Fragment> {
            let publication = storage::load_publication(conn).unwrap().unwrap();
            storage::load_fragment_payloads(conn, &publication.config_key)
                .unwrap()
                .into_iter()
                .map(|(spec, payload)| (spec, decode_fragment(&payload).unwrap()))
                .collect()
        }

        fn sites(conn: &Connection) -> Vec<(String, String, String)> {
            let mut statement = conn
                .prepare("SELECT key, spec, json FROM effect_sites ORDER BY key")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }

        fn deps(conn: &Connection) -> BTreeMap<String, Vec<(String, u64)>> {
            storage::load_spec_deps(conn).unwrap()
        }

        fn preview_request(spec: &str, anchor: &str, mode: EffectsMode) -> EffectsRequest {
            let mut request = request(spec, anchor);
            request.options.mode = mode;
            request
        }

        fn dom_caller_row(conn: &Connection) -> String {
            let key = selector_key(&request("DOM", DOM_CALLER_ANCHOR).subject).unwrap();
            rows(conn).remove(&key).unwrap()
        }

        #[test]
        fn second_publish_builds_nothing() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            let first = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(first.fragments_built, ["DOM", "INFRA", "URL"]);
            assert!(
                publication_is_current(&conn, &catalog.content_digest, &options.environment)
                    .unwrap()
            );
            let second = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            assert!(second.fragments_built.is_empty());
            assert_eq!((second.rows_upserted, second.rows_deleted), (0, 0));
        }

        #[test]
        fn incremental_publish_equals_rebuild() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            reindex(&conn, "INFRA", &infra_edited());
            let report = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(report.fragments_built, ["INFRA"]);
            assert!(report.rows_upserted > 0, "the edited body's row changes");
            let incremental = (rows(&conn), fragments(&conn), sites(&conn), deps(&conn));
            let rebuilt = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Rebuild,
                BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(rebuilt.fragments_built, ["DOM", "INFRA", "URL"]);
            assert_eq!(
                incremental,
                (rows(&conn), fragments(&conn), sites(&conn), deps(&conn))
            );
        }

        #[test]
        fn callee_anchor_rename_updates_caller_rows_without_reparse() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            let before = dom_caller_row(&conn);
            reindex(&conn, "INFRA", &infra_with_renamed_algorithm());
            let report = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(report.fragments_built, ["INFRA"], "DOM is not rebuilt");
            let dom_row = dom_caller_row(&conn);
            assert!(dom_row.contains("missing_anchor"), "{dom_row}");
            assert_ne!(before, dom_row, "the caller row follows the callee");
        }

        #[test]
        fn removed_spec_orphans_fragment_and_marks_callers_missing_spec() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            let before = dom_caller_row(&conn);
            assert!(sites(&conn).iter().any(|(_, spec, _)| spec == "INFRA"));
            let infra: i64 = conn
                .query_row("SELECT id FROM specs WHERE name='INFRA'", [], |r| r.get(0))
                .unwrap();
            crate::db::write::delete_spec_data(&conn, infra).unwrap();
            assert!(
                !publication_is_current(&conn, &catalog.content_digest, &options.environment)
                    .unwrap()
            );
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(
                fragments(&conn).keys().cloned().collect::<Vec<_>>(),
                ["DOM", "URL"]
            );
            let dom_row = dom_caller_row(&conn);
            assert!(dom_row.contains("missing_spec"), "{dom_row}");
            assert_ne!(before, dom_row);
            assert!(rows(&conn).keys().all(|key| !key.contains("\"INFRA\"")));
            assert!(sites(&conn).iter().all(|(_, spec, _)| spec != "INFRA"));
        }

        #[test]
        fn preview_serves_the_current_publication_and_nothing_stale() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            let mut request = preview_request("DOM", DOM_CALLER_ANCHOR, EffectsMode::Cached);
            request.options.rule_paths = options.rule_paths.clone();
            assert!(get_cached_effect_preview_on(&conn, &request)
                .unwrap()
                .is_some());
            reindex(&conn, "INFRA", &infra_edited());
            assert!(
                get_cached_effect_preview_on(&conn, &request)
                    .unwrap()
                    .is_none(),
                "stale rows are never served"
            );
            let mut auto = request.clone();
            auto.options.mode = EffectsMode::Auto;
            let error = get_effect_summary_on(&conn, &auto).unwrap_err();
            assert_eq!(error.details.unwrap()["issue"], "snapshot_changed");
        }

        #[test]
        fn fingerprint_distinguishes_current_and_stale() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            let digest = catalog_digest(&options.rule_paths).unwrap();
            assert_eq!(digest, catalog.content_digest);
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            let stale_bytes = stale_structure_bytes(&conn, &digest, &options.environment).unwrap();
            assert!(stale_bytes.is_empty());
            reindex(&conn, "INFRA", &infra_edited());
            let stale_bytes = stale_structure_bytes(&conn, &digest, &options.environment).unwrap();
            assert_eq!(stale_bytes.len(), 1);
            assert!(stale_bytes[0] > 0);
        }

        #[test]
        fn prebuilt_fragments_replace_the_build() {
            let conn = corpus_db(&MULTI);
            let (catalog, options) = fixture_catalog_and_options();
            publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::new(),
            )
            .unwrap();
            let mut prebuilt = fragments(&conn);
            let infra = prebuilt.remove("INFRA").unwrap();
            let mut marked = infra.clone();
            marked.base_url = "https://prebuilt.test/".into();
            touch(&conn);
            let report = publish(
                &conn,
                &catalog,
                &options,
                PublishMode::Incremental,
                BTreeMap::from([("INFRA".to_string(), marked.clone())]),
            )
            .unwrap();
            assert_eq!(report.fragments_built, ["DOM", "INFRA", "URL"]);
            assert_eq!(fragments(&conn)["INFRA"], marked);
        }
    }
}
