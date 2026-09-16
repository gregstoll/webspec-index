//! Snapshot-bound storage shared by all effects consumers.
use anyhow::Result;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};

use crate::effects::engine::{Graph, IssueId, LocalOccurrence};
use crate::effects::graph::{ExecutionEdge, ExecutionNode};
use crate::effects::model::{
    Boundary, ContextRef, Execution, GraphIssue, IssueCode, Relationship, SourceSite, Subject,
};

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS effect_structures (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            representation_version TEXT NOT NULL,
            structure_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_anchors (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            anchor TEXT NOT NULL,
            PRIMARY KEY(snapshot_id, anchor)
        );
        CREATE TABLE IF NOT EXISTS effect_local_matches (
            input_key TEXT PRIMARY KEY, payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_runs (
            analysis_id TEXT PRIMARY KEY,
            generation INTEGER NOT NULL,
            semantic_key TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            budget_key TEXT NOT NULL,
            reached_fixed_point INTEGER NOT NULL,
            manifest_json TEXT NOT NULL,
            artifact_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS effect_runs_lookup ON effect_runs(semantic_key, generation);
        CREATE TABLE IF NOT EXISTS effect_subjects (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            subject_key TEXT NOT NULL,
            summary_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, subject_key)
        );
        CREATE TABLE IF NOT EXISTS effect_witnesses (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            subject_key TEXT NOT NULL,
            effect_id TEXT NOT NULL,
            witness_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, subject_key, effect_id)
        );
        CREATE TABLE IF NOT EXISTS effect_issues (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            issue_id TEXT NOT NULL,
            issue_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, issue_id)
        );
        INSERT OR IGNORE INTO meta(key, value) VALUES ('effects_generation', '0');
        INSERT OR IGNORE INTO meta(key, value) VALUES ('effects_publication', '0');
        CREATE TABLE IF NOT EXISTS effect_graph (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            generation INTEGER NOT NULL,
            semantic_key TEXT NOT NULL,
            manifest_json TEXT NOT NULL,
            topology BLOB NOT NULL,
            opaque_anchor_issue_id INTEGER
        );
        CREATE TABLE IF NOT EXISTS effect_sites (key TEXT PRIMARY KEY, json TEXT NOT NULL);",
    )?;
    // Publishing summaries changes reader caches, not the semantic input key.
    for event in ["INSERT", "UPDATE", "DELETE"] {
        conn.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS effects_publication_{event}
             AFTER {event} ON effect_runs BEGIN
             UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_publication';
             END;"
        ))?;
    }
    // These triggers also cover mutation paths outside the effects engine.
    // Triggers participate in the writer's transaction, including rollbacks.
    for table in ["specs", "snapshots", "effect_structures"] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            conn.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS effects_generation_{table}_{event}
                 AFTER {event} ON {table} BEGIN
                 UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_generation';
                 END;"
            ))?;
        }
    }
    Ok(())
}

pub fn generation(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'effects_generation'",
        [],
        |row| row.get(0),
    )?)
}

pub fn invalidate(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_generation'",
        [],
    )?;
    Ok(())
}

pub fn store_structure(
    conn: &Connection,
    snapshot_id: i64,
    version: &str,
    structure_json: &str,
) -> Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(structure_json)?;
    // Callers normally publish the complete source inside an existing transaction.
    // Joining it also keeps the fragment inventory atomic with the structure.
    super::write::atomic_write(conn, |conn| {
        conn.execute(
        "INSERT INTO effect_structures(snapshot_id, representation_version, structure_json)
         VALUES (?1, ?2, ?3) ON CONFLICT(snapshot_id) DO UPDATE SET
         representation_version=excluded.representation_version, structure_json=excluded.structure_json",
        (snapshot_id, version, structure_json),
    )?;
        conn.execute(
            "DELETE FROM effect_anchors WHERE snapshot_id=?1",
            [snapshot_id],
        )?;
        if let Some(anchors) = parsed.get("anchors").and_then(serde_json::Value::as_array) {
            let mut insert = conn.prepare("INSERT OR IGNORE INTO effect_anchors VALUES (?1,?2)")?;
            for anchor in anchors.iter().filter_map(serde_json::Value::as_str) {
                insert.execute((snapshot_id, anchor))?;
            }
        }
        Ok(())
    })
}

pub fn load_structure(
    conn: &Connection,
    snapshot_id: i64,
    version: &str,
) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT structure_json FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2",
        (snapshot_id, version), |row| row.get(0),
    ).optional()?)
}

pub fn has_structure(conn: &Connection, snapshot_id: i64, version: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2)",
        (snapshot_id, version), |row| row.get(0),
    )?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRun {
    pub analysis_id: String,
    pub generation: i64,
    pub semantic_key: String,
    pub scope_key: String,
    pub budget_key: String,
    pub reached_fixed_point: bool,
    pub manifest_json: String,
    pub artifact_json: String,
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRun> {
    Ok(StoredRun {
        analysis_id: row.get(0)?,
        generation: row.get(1)?,
        semantic_key: row.get(2)?,
        scope_key: row.get(3)?,
        budget_key: row.get(4)?,
        reached_fixed_point: row.get(5)?,
        manifest_json: row.get(6)?,
        artifact_json: row.get(7)?,
    })
}

/// Return false on a snapshot race; callers may retry against new inputs.
pub fn publish_run(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
) -> Result<bool> {
    publish_run_with_issues(conn, run, subjects, &[])
}

pub fn publish_run_with_issues(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
    issues: &[(String, String)],
) -> Result<bool> {
    publish_run_with_witnesses(conn, run, subjects, issues, &[])
}

pub fn publish_run_with_witnesses(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
    issues: &[(String, String)],
    witnesses: &[(String, String, String)],
) -> Result<bool> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if generation(&tx)? != run.generation {
        return Ok(false);
    }
    // V1 does not retain historical source snapshots. Their runs are already
    // unavailable, so reclaim them when publishing against the new generation.
    for table in ["effect_subjects", "effect_issues", "effect_witnesses"] {
        tx.execute(&format!("DELETE FROM {table} WHERE analysis_id IN (SELECT analysis_id FROM effect_runs WHERE generation != ?1)"), [run.generation])?;
    }
    tx.execute(
        "DELETE FROM effect_runs WHERE generation != ?1",
        [run.generation],
    )?;
    tx.execute(
        "DELETE FROM effect_subjects WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "DELETE FROM effect_issues WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "DELETE FROM effect_witnesses WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO effect_runs VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            run.analysis_id,
            run.generation,
            run.semantic_key,
            run.scope_key,
            run.budget_key,
            run.reached_fixed_point,
            run.manifest_json,
            run.artifact_json
        ],
    )?;
    for (subject, summary) in subjects {
        tx.execute(
            "INSERT INTO effect_subjects VALUES (?1,?2,?3)",
            rusqlite::params![&run.analysis_id, subject, encode_payload(summary)],
        )?;
    }
    let mut insert = tx.prepare("INSERT INTO effect_issues VALUES (?1,?2,?3)")?;
    for (id, payload) in issues {
        insert.execute(rusqlite::params![
            &run.analysis_id,
            id,
            encode_payload(payload)
        ])?;
    }
    drop(insert);
    let mut insert = tx.prepare("INSERT INTO effect_witnesses VALUES (?1,?2,?3,?4)")?;
    for (subject, effect, witness) in witnesses {
        insert.execute(rusqlite::params![
            &run.analysis_id,
            subject,
            effect,
            encode_payload(witness)
        ])?;
    }
    drop(insert);
    tx.commit()?;
    Ok(true)
}

pub fn load_run(conn: &Connection, analysis_id: &str) -> Result<Option<StoredRun>> {
    Ok(conn.query_row(
        "SELECT analysis_id,generation,semantic_key,scope_key,budget_key,reached_fixed_point,manifest_json,artifact_json
         FROM effect_runs WHERE analysis_id=?1", [analysis_id], read_run,
    ).optional()?)
}

/// Run headers; explanations load the large artifact separately with `load_run`.
pub fn matching_runs(
    conn: &Connection,
    semantic_key: &str,
    generation: i64,
) -> Result<Vec<StoredRun>> {
    let mut stmt = conn.prepare(
        "SELECT analysis_id,generation,semantic_key,scope_key,budget_key,reached_fixed_point,manifest_json,''
         FROM effect_runs WHERE semantic_key=?1 AND generation=?2
         ORDER BY reached_fixed_point DESC, (json_extract(manifest_json,'$.scope.kind')='all') DESC, analysis_id",
    )?;
    let runs = stmt
        .query_map((semantic_key, generation), read_run)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(runs)
}

pub fn load_issues(conn: &Connection, analysis_id: &str, ids: &[String]) -> Result<Vec<String>> {
    let mut select =
        conn.prepare("SELECT issue_json FROM effect_issues WHERE analysis_id=?1 AND issue_id=?2")?;
    ids.iter()
        .map(|id| {
            select
                .query_row((analysis_id, id), |row| {
                    decode_payload(row.get_ref(0)?)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
                })
                .map_err(Into::into)
        })
        .collect()
}

pub fn load_subject(conn: &Connection, analysis_id: &str, subject: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT summary_json FROM effect_subjects WHERE analysis_id=?1 AND subject_key=?2",
        (analysis_id, subject),
        |row| {
            decode_payload(row.get_ref(0)?)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_local_matches(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT payload_json FROM effect_local_matches WHERE input_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn store_local_matches(conn: &Connection, key: &str, payload: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO effect_local_matches VALUES (?1,?2)",
        (key, payload),
    )?;
    Ok(())
}

fn encode_payload(text: &str) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(text.as_bytes())
        .and_then(|_| encoder.finish())
        .expect("writing to an in-memory buffer cannot fail")
}

fn decode_payload(value: ValueRef<'_>) -> Result<String> {
    match value {
        ValueRef::Text(bytes) => Ok(String::from_utf8(bytes.to_vec())?),
        ValueRef::Blob(bytes) => {
            let mut text = String::new();
            DeflateDecoder::new(bytes).read_to_string(&mut text)?;
            Ok(text)
        }
        other => anyhow::bail!("unexpected payload type {}", other.data_type()),
    }
}

// ----- Graph storage -----

pub struct SqlSiteStore<'a>(pub &'a Connection);

impl crate::effects::graph::SiteStore for SqlSiteStore<'_> {
    fn site(&self, key: &str) -> Option<SourceSite> {
        // SiteStore is infallible; a missing or unreadable site yields None by
        // design — callers treat absent sites as unknown context.
        self.0
            .query_row("SELECT json FROM effect_sites WHERE key=?1", [key], |row| {
                let json: String = row.get(0)?;
                serde_json::from_str(&json)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })
            .optional()
            .ok()
            .flatten()
    }
}

#[derive(Debug, Clone)]
pub struct StoredGraphMeta {
    pub generation: i64,
    pub semantic_key: String,
    pub manifest_json: String,
    pub opaque_anchor_issue: Option<IssueId>,
}

#[derive(Serialize, Deserialize)]
struct Topology {
    engine_version: u32,
    environment: String,
    catalog_digest: String,
    node_ids: Vec<String>,
    nodes: Vec<(u32, Subject, u64, bool, bool)>,
    anchor_nodes: Vec<(String, String, u32)>,
    #[allow(clippy::type_complexity)]
    edges: Vec<(
        String,
        u32,
        u32,
        Relationship,
        Execution,
        String,
        String,
        Vec<ContextRef>,
        bool,
        Option<Boundary>,
        u64,
    )>,
    occurrences: Vec<LocalOccurrence>,
    issue_catalog: Vec<GraphIssue>,
    issues: Vec<(u32, Vec<IssueId>)>,
    definitions: Vec<(u32, Vec<u32>)>,
    effect_categories: BTreeMap<String, String>,
    opaque_anchor_issue: Option<IssueId>,
}

fn intern_id(s: &str, node_ids: &mut Vec<String>, id_idx: &mut HashMap<String, u32>) -> u32 {
    if let Some(&idx) = id_idx.get(s) {
        return idx;
    }
    let idx = node_ids.len() as u32;
    node_ids.push(s.to_string());
    id_idx.insert(s.to_string(), idx);
    idx
}

fn build_topology(graph: &Graph) -> Topology {
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

    // Find the canonical opaque anchor issue id (the issue attached to prunable
    // anchor nodes that have no reusable algorithm body).
    let mut opaque_anchor_issue: Option<IssueId> = None;
    'outer: for node_id in graph.nodes.keys() {
        if !node_id.starts_with("anchor:") {
            continue;
        }
        if edge_endpoints.contains(node_id.as_str())
            || occurrence_subjects.contains(node_id.as_str())
        {
            continue;
        }
        if let Some(ids) = graph.issues.get(node_id) {
            if ids.len() == 1 {
                let iid = ids[0];
                if let Some(issue) = graph.issue_catalog.get(iid as usize) {
                    if issue.code == IssueCode::UnsupportedStructure
                        && issue.message.starts_with(MAGIC_PREFIX)
                    {
                        opaque_anchor_issue = Some(iid);
                        break 'outer;
                    }
                }
            }
        }
    }

    // Determine which nodes to keep.
    let kept: HashSet<&str> = graph
        .nodes
        .keys()
        .filter(|id| {
            if !id.starts_with("anchor:") {
                return true;
            }
            if edge_endpoints.contains(id.as_str()) || occurrence_subjects.contains(id.as_str()) {
                return true;
            }
            match graph.issues.get(id.as_str()).map(Vec::as_slice) {
                None | Some([]) => false,
                Some(ids) => match opaque_anchor_issue {
                    Some(oi) => !(ids.len() == 1 && ids[0] == oi),
                    None => true,
                },
            }
        })
        .map(String::as_str)
        .collect();

    let mut node_ids: Vec<String> = Vec::new();
    let mut id_idx: HashMap<String, u32> = HashMap::new();

    // Intern kept node IDs (deterministic BTreeMap order).
    for id in graph.nodes.keys() {
        if kept.contains(id.as_str()) {
            intern_id(id, &mut node_ids, &mut id_idx);
        }
    }
    // Intern edge endpoints (all edges are kept).
    for edge in graph.edges.values() {
        intern_id(&edge.from, &mut node_ids, &mut id_idx);
        intern_id(&edge.to, &mut node_ids, &mut id_idx);
    }
    // Intern definition keys and values for kept nodes.
    for (k, vals) in &graph.definitions {
        if kept.contains(k.as_str()) {
            intern_id(k, &mut node_ids, &mut id_idx);
            for v in vals {
                intern_id(v, &mut node_ids, &mut id_idx);
            }
        }
    }
    // Intern issues keys for kept nodes.
    for k in graph.issues.keys() {
        if kept.contains(k.as_str()) {
            intern_id(k, &mut node_ids, &mut id_idx);
        }
    }

    let nodes: Vec<_> = graph
        .nodes
        .iter()
        .filter(|(id, _)| kept.contains(id.as_str()))
        .map(|(id, node)| {
            (
                id_idx[id],
                node.subject.clone(),
                node.source_order,
                node.is_body,
                node.definition_only,
            )
        })
        .collect();

    let anchor_nodes: Vec<_> = graph
        .anchor_nodes
        .iter()
        .filter(|(_, v)| kept.contains(v.as_str()))
        .map(|((spec, anchor), node_id)| (spec.clone(), anchor.clone(), id_idx[node_id]))
        .collect();

    let edges: Vec<_> = graph
        .edges
        .values()
        .map(|e| {
            (
                e.id.clone(),
                id_idx[&e.from],
                id_idx[&e.to],
                e.relation,
                e.execution,
                e.site_id.clone(),
                e.site_key.clone(),
                e.context.clone(),
                e.context_truncated,
                e.boundary.clone(),
                e.source_order,
            )
        })
        .collect();

    let issues: Vec<_> = graph
        .issues
        .iter()
        .filter(|(k, _)| kept.contains(k.as_str()))
        .map(|(k, v)| (id_idx[k], v.clone()))
        .collect();

    let definitions: Vec<_> = graph
        .definitions
        .iter()
        .filter(|(k, _)| kept.contains(k.as_str()))
        .map(|(k, vals)| {
            let key_idx = id_idx[k];
            let val_idxs: Vec<u32> = vals
                .iter()
                .map(|v| intern_id(v, &mut node_ids, &mut id_idx))
                .collect();
            (key_idx, val_idxs)
        })
        .collect();

    Topology {
        engine_version: graph.engine_version,
        environment: graph.environment.clone(),
        catalog_digest: graph.catalog_digest.clone(),
        node_ids,
        nodes,
        anchor_nodes,
        edges,
        occurrences: graph.occurrences.values().cloned().collect(),
        issue_catalog: graph.issue_catalog.clone(),
        issues,
        definitions,
        effect_categories: graph.effect_categories.clone(),
        opaque_anchor_issue,
    }
}

pub fn store_graph(conn: &Connection, meta: &StoredGraphMeta, graph: &Graph) -> Result<()> {
    let topology = build_topology(graph);
    let opaque_id = topology.opaque_anchor_issue.map(|v| v as i64);
    let json = serde_json::to_string(&topology)?;
    let blob = encode_payload(&json);
    super::write::atomic_write(conn, |conn| {
        conn.execute(
            "INSERT OR REPLACE INTO effect_graph(id, generation, semantic_key, manifest_json, topology, opaque_anchor_issue_id)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![meta.generation, meta.semantic_key, meta.manifest_json, blob, opaque_id],
        )?;
        conn.execute("DELETE FROM effect_sites", [])?;
        let mut insert = conn.prepare("INSERT INTO effect_sites(key, json) VALUES (?1, ?2)")?;
        for (key, site) in &graph.sites {
            let site_json = serde_json::to_string(site)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
            insert.execute((key, site_json))?;
        }
        Ok(())
    })
}

pub fn load_graph_meta(conn: &Connection) -> Result<Option<StoredGraphMeta>> {
    conn.query_row(
        "SELECT generation, semantic_key, manifest_json, opaque_anchor_issue_id FROM effect_graph WHERE id=1",
        [],
        |row| {
            Ok(StoredGraphMeta {
                generation: row.get(0)?,
                semantic_key: row.get(1)?,
                manifest_json: row.get(2)?,
                opaque_anchor_issue: row
                    .get::<_, Option<i64>>(3)?
                    .map(|v| v as IssueId),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_graph(conn: &Connection) -> Result<Option<Graph>> {
    let Some(blob) = conn
        .query_row("SELECT topology FROM effect_graph WHERE id=1", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .optional()?
    else {
        return Ok(None);
    };

    let json = decode_payload(rusqlite::types::ValueRef::Blob(&blob))?;
    let topo: Topology = serde_json::from_str(&json)?;
    let ids = &topo.node_ids;

    let nodes: BTreeMap<String, ExecutionNode> = topo
        .nodes
        .into_iter()
        .map(|(idx, subject, source_order, is_body, definition_only)| {
            let id = ids[idx as usize].clone();
            (
                id.clone(),
                ExecutionNode {
                    id,
                    subject,
                    source_order,
                    is_body,
                    definition_only,
                },
            )
        })
        .collect();

    let anchor_nodes: BTreeMap<(String, String), String> = topo
        .anchor_nodes
        .into_iter()
        .map(|(spec, anchor, idx)| ((spec, anchor), ids[idx as usize].clone()))
        .collect();

    let edges: BTreeMap<String, ExecutionEdge> = topo
        .edges
        .into_iter()
        .map(
            |(
                id,
                from_idx,
                to_idx,
                relation,
                execution,
                site_id,
                site_key,
                context,
                context_truncated,
                boundary,
                source_order,
            )| {
                (
                    id.clone(),
                    ExecutionEdge {
                        id,
                        from: ids[from_idx as usize].clone(),
                        to: ids[to_idx as usize].clone(),
                        relation,
                        execution,
                        site_id,
                        site_key,
                        context,
                        context_truncated,
                        boundary,
                        source_order,
                    },
                )
            },
        )
        .collect();

    let occurrences: BTreeMap<String, LocalOccurrence> = topo
        .occurrences
        .into_iter()
        .map(|o| (o.id.clone(), o))
        .collect();

    let issues: BTreeMap<String, Vec<IssueId>> = topo
        .issues
        .into_iter()
        .map(|(idx, v)| (ids[idx as usize].clone(), v))
        .collect();

    let definitions: BTreeMap<String, Vec<String>> = topo
        .definitions
        .into_iter()
        .map(|(k_idx, v_idxs)| {
            (
                ids[k_idx as usize].clone(),
                v_idxs
                    .into_iter()
                    .map(|i| ids[i as usize].clone())
                    .collect(),
            )
        })
        .collect();

    let mut graph = Graph {
        engine_version: topo.engine_version,
        environment: topo.environment,
        catalog_digest: topo.catalog_digest,
        nodes,
        anchor_nodes,
        edges,
        occurrences,
        issue_catalog: topo.issue_catalog,
        issues,
        definitions,
        sites: BTreeMap::new(),
        effect_categories: topo.effect_categories,
        edges_by_source: Default::default(),
        edges_by_target: Default::default(),
    };
    graph.index_edges();
    Ok(Some(graph))
}

pub fn load_witness(
    conn: &Connection,
    analysis_id: &str,
    subject_key: &str,
    effect_id: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT witness_json FROM effect_witnesses WHERE analysis_id=?1 AND subject_key=?2 AND effect_id=?3",
        (analysis_id, subject_key, effect_id),
        |row| decode_payload(row.get_ref(0)?).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into())),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::graph::SiteStore as _;

    fn run(generation: i64) -> StoredRun {
        StoredRun {
            analysis_id: "an_test".into(),
            generation,
            semantic_key: "semantic".into(),
            scope_key: "all".into(),
            budget_key: "default".into(),
            reached_fixed_point: true,
            manifest_json: "{}".into(),
            artifact_json: "{}".into(),
        }
    }

    #[test]
    fn snapshot_race_cannot_publish_stale_run() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        crate::db::write::insert_or_get_spec(&conn, "TEST", "https://test.example", "test")
            .unwrap();
        assert!(!publish_run(&conn, &pending, &[]).unwrap());
        assert!(load_run(&conn, "an_test").unwrap().is_none());
    }

    #[test]
    fn failed_run_publication_is_atomic() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        let duplicate = vec![("root".into(), "{}".into()), ("root".into(), "{}".into())];
        assert!(publish_run(&conn, &pending, &duplicate).is_err());
        assert!(load_run(&conn, "an_test").unwrap().is_none());
        assert!(publish_run(&conn, &pending, &duplicate[..1]).unwrap());
        assert_eq!(
            load_subject(&conn, "an_test", "root").unwrap().as_deref(),
            Some("{}")
        );
        assert!(load_subject(&conn, "an_test", "unprocessed")
            .unwrap()
            .is_none());
    }

    #[test]
    fn prepared_paths_publish_atomically_and_are_purged_with_the_run() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        let duplicate = vec![("root".into(), "effect".into(), "{}".into()); 2];
        assert!(publish_run_with_witnesses(&conn, &pending, &[], &[], &duplicate).is_err());
        assert!(load_run(&conn, &pending.analysis_id).unwrap().is_none());
        assert!(publish_run_with_witnesses(&conn, &pending, &[], &[], &duplicate[..1]).unwrap());
        crate::db::schema::purge_if_version_changed(&conn, "test-new-parser").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM effect_witnesses", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn payload_round_trips_through_deflate() {
        let text = r#"{"hops":[{"from":{"spec":"HTML","anchor":"navigate"}}]}"#;
        let encoded = encode_payload(text);
        assert!(encoded.len() < text.len() * 2);
        let decoded = decode_payload(rusqlite::types::ValueRef::Blob(&encoded)).unwrap();
        assert_eq!(decoded, text);
    }

    #[test]
    fn legacy_text_payload_is_returned_verbatim() {
        let decoded =
            decode_payload(rusqlite::types::ValueRef::Text(b"{\"legacy\":true}")).unwrap();
        assert_eq!(decoded, "{\"legacy\":true}");
    }

    #[test]
    fn published_rows_are_blobs_and_load_back_as_text() {
        let conn = crate::db::open_test_db().unwrap();
        let run = StoredRun {
            analysis_id: "a1".into(),
            generation: generation(&conn).unwrap(),
            semantic_key: "s".into(),
            scope_key: "sc".into(),
            budget_key: "b".into(),
            reached_fixed_point: true,
            manifest_json: "{}".into(),
            artifact_json: "{}".into(),
        };
        let ok = publish_run_with_witnesses(
            &conn,
            &run,
            &[("subj".into(), "{\"summary\":1}".into())],
            &[("iss".into(), "{\"issue\":1}".into())],
            &[("subj".into(), "eff".into(), "{\"witness\":1}".into())],
        )
        .unwrap();
        assert!(ok);
        let stored_type: String = conn
            .query_row(
                "SELECT typeof(summary_json) FROM effect_subjects WHERE analysis_id='a1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_type, "blob");
        assert_eq!(
            load_subject(&conn, "a1", "subj").unwrap().unwrap(),
            "{\"summary\":1}"
        );
        assert_eq!(
            load_issues(&conn, "a1", &["iss".into()]).unwrap(),
            vec!["{\"issue\":1}".to_string()]
        );
        assert_eq!(
            load_witness(&conn, "a1", "subj", "eff").unwrap().unwrap(),
            "{\"witness\":1}"
        );
    }

    #[test]
    fn legacy_text_rows_still_load() {
        let conn = crate::db::open_test_db().unwrap();
        conn.execute(
            "INSERT INTO effect_runs VALUES ('old', 0, 's', 'sc', 'b', 1, '{}', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO effect_subjects(analysis_id, subject_key, summary_json) VALUES ('old', 'k', '{\"legacy\":1}')",
            [],
        )
        .unwrap();
        assert_eq!(
            load_subject(&conn, "old", "k").unwrap().unwrap(),
            "{\"legacy\":1}"
        );
    }

    #[test]
    fn rolled_back_corpus_write_does_not_advance_generation() {
        let conn = crate::db::open_test_db().unwrap();
        let before = generation(&conn).unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            crate::db::write::insert_or_get_spec(&tx, "TEST", "https://test.example", "test")
                .unwrap();
            assert!(generation(&tx).unwrap() > before);
        }
        assert_eq!(generation(&conn).unwrap(), before);
    }

    fn tiny_graph() -> Graph {
        use crate::effects::model::{Evidence, EvidenceBasis, IssueCode};

        let subject1 = Subject {
            spec: "HTML".into(),
            anchor: "foo".into(),
            snapshot_sha: "abc".into(),
            step_id: None,
            step_path: None,
            body_id: None,
        };
        let subject2 = Subject {
            spec: "HTML".into(),
            anchor: "bar".into(),
            snapshot_sha: "abc".into(),
            step_id: None,
            step_path: None,
            body_id: None,
        };
        let site = SourceSite {
            id: "site_1".into(),
            subject: subject1.clone(),
            url: "https://html.spec.whatwg.org/#foo".into(),
            segment_id: None,
            span: None,
            step_text: None,
        };
        let site_k = crate::effects::graph::site_key(&site);

        let node1 = ExecutionNode {
            id: "n1".into(),
            subject: subject1.clone(),
            source_order: 0,
            is_body: false,
            definition_only: false,
        };
        let node2 = ExecutionNode {
            id: "n2".into(),
            subject: subject2.clone(),
            source_order: 1,
            is_body: true,
            definition_only: false,
        };
        let edge = ExecutionEdge {
            id: "rel_x".into(),
            from: "n1".into(),
            to: "n2".into(),
            relation: Relationship::Invoke,
            execution: Execution::Inline,
            site_key: site_k.clone(),
            site_id: "site_1".into(),
            context: vec![],
            context_truncated: false,
            boundary: None,
            source_order: 0,
        };
        let evidence = Evidence {
            id: "ev1".into(),
            basis: EvidenceBasis::Matched,
            rule_id: "rule1".into(),
            site: site.clone(),
            captures: None,
            argument_expressions: None,
            reason: None,
        };
        let occurrence = LocalOccurrence {
            id: "occ1".into(),
            subject_id: "n1".into(),
            kind: "test_effect".into(),
            params: BTreeMap::new(),
            evidence: vec![evidence],
            issue_codes: vec![],
            source_order: 0,
        };
        let issue = GraphIssue {
            code: IssueCode::MissingAnchor,
            message: "test issue".into(),
            site_key: None,
        };

        let mut nodes = BTreeMap::new();
        nodes.insert("n1".into(), node1);
        nodes.insert("n2".into(), node2);
        let mut edges = BTreeMap::new();
        edges.insert("rel_x".into(), edge);
        let mut occurrences = BTreeMap::new();
        occurrences.insert("occ1".into(), occurrence);
        let mut sites = BTreeMap::new();
        sites.insert(site_k, site);
        let mut issues_map = BTreeMap::new();
        issues_map.insert("n1".into(), vec![0u32]);

        let mut definitions = BTreeMap::new();
        definitions.insert("n1".into(), vec!["n2".into()]);

        let mut g = Graph {
            engine_version: 1,
            environment: "web".into(),
            catalog_digest: "digest1".into(),
            nodes,
            anchor_nodes: BTreeMap::new(),
            edges,
            occurrences,
            issue_catalog: vec![issue],
            issues: issues_map,
            definitions,
            sites,
            effect_categories: BTreeMap::new(),
            edges_by_source: Default::default(),
            edges_by_target: Default::default(),
        };
        g.index_edges();
        g
    }

    #[test]
    fn graph_store_roundtrips_topology_and_serves_sites() {
        let conn = crate::db::open_test_db().unwrap();
        let graph = tiny_graph();
        store_graph(
            &conn,
            &StoredGraphMeta {
                generation: 3,
                semantic_key: "k".into(),
                manifest_json: "{}".into(),
                opaque_anchor_issue: None,
            },
            &graph,
        )
        .unwrap();
        let meta = load_graph_meta(&conn).unwrap().unwrap();
        assert_eq!((meta.generation, meta.semantic_key.as_str()), (3, "k"));
        let loaded = load_graph(&conn).unwrap().unwrap();
        assert_eq!(loaded.nodes, graph.nodes);
        assert_eq!(loaded.edges, graph.edges);
        assert_eq!(loaded.occurrences, graph.occurrences);
        assert_eq!(loaded.issue_catalog, graph.issue_catalog);
        assert_eq!(loaded.definitions, graph.definitions);
        assert!(loaded.sites.is_empty());
        let key = graph.sites.keys().next().unwrap();
        assert_eq!(SqlSiteStore(&conn).site(key), graph.sites.get(key).cloned());
        assert!(!loaded.edges_by_source.is_empty());
    }

    #[test]
    fn graph_store_prunes_opaque_anchor_nodes() {
        use crate::effects::model::IssueCode;

        let subj = |anchor: &str| Subject {
            spec: "HTML".into(),
            anchor: anchor.to_string(),
            snapshot_sha: "sha1".into(),
            step_id: None,
            step_path: None,
            body_id: None,
        };
        let node = |id: &str, order: u64| ExecutionNode {
            id: id.to_string(),
            subject: subj(id),
            source_order: order,
            is_body: false,
            definition_only: false,
        };

        // issue_catalog[0] is the opaque "no reusable algorithm body" issue
        // issue_catalog[1] is an ordinary second issue
        let opaque_issue = GraphIssue {
            code: IssueCode::UnsupportedStructure,
            message: "indexed anchor has no reusable algorithm body for test".into(),
            site_key: None,
        };
        let extra_issue = GraphIssue {
            code: IssueCode::MissingSpec,
            message: "second issue".into(),
            site_key: None,
        };

        let mut nodes = BTreeMap::new();
        nodes.insert("n1".into(), node("n1", 0));
        // will be pruned: only issue is the opaque one, not an edge endpoint
        nodes.insert("anchor:pruned".into(), node("anchor:pruned", 1));
        // will be kept: is the target of an edge
        nodes.insert("anchor:kept-edge".into(), node("anchor:kept-edge", 2));
        // will be kept: has opaque issue plus a second issue
        nodes.insert("anchor:kept-issues".into(), node("anchor:kept-issues", 3));

        let edge = ExecutionEdge {
            id: "e1".into(),
            from: "n1".into(),
            to: "anchor:kept-edge".into(),
            relation: Relationship::Invoke,
            execution: Execution::Inline,
            site_key: "k".into(),
            site_id: "s".into(),
            context: vec![],
            context_truncated: false,
            boundary: None,
            source_order: 0,
        };
        let mut edges = BTreeMap::new();
        edges.insert("e1".into(), edge);

        let mut anchor_nodes_map = BTreeMap::new();
        anchor_nodes_map.insert(("HTML".into(), "base".into()), "n1".into());
        anchor_nodes_map.insert(("HTML".into(), "pruned".into()), "anchor:pruned".into());
        anchor_nodes_map.insert(
            ("HTML".into(), "kept-edge".into()),
            "anchor:kept-edge".into(),
        );
        anchor_nodes_map.insert(
            ("HTML".into(), "kept-issues".into()),
            "anchor:kept-issues".into(),
        );

        let mut issues_map = BTreeMap::new();
        issues_map.insert("anchor:pruned".into(), vec![0u32]);
        issues_map.insert("anchor:kept-issues".into(), vec![0u32, 1u32]);

        let mut g = Graph {
            engine_version: 1,
            environment: "web".into(),
            catalog_digest: "digest2".into(),
            nodes,
            anchor_nodes: anchor_nodes_map,
            edges,
            occurrences: BTreeMap::new(),
            issue_catalog: vec![opaque_issue, extra_issue],
            issues: issues_map,
            definitions: BTreeMap::new(),
            sites: BTreeMap::new(),
            effect_categories: BTreeMap::new(),
            edges_by_source: Default::default(),
            edges_by_target: Default::default(),
        };
        g.index_edges();

        let conn = crate::db::open_test_db().unwrap();
        store_graph(
            &conn,
            &StoredGraphMeta {
                generation: 1,
                semantic_key: "pruning-test".into(),
                manifest_json: "{}".into(),
                opaque_anchor_issue: None,
            },
            &g,
        )
        .unwrap();

        let meta = load_graph_meta(&conn).unwrap().unwrap();
        assert_eq!(
            meta.opaque_anchor_issue,
            Some(0u32),
            "opaque issue id must be 0"
        );

        let loaded = load_graph(&conn).unwrap().unwrap();
        assert!(
            !loaded.nodes.contains_key("anchor:pruned"),
            "pruned anchor must be absent"
        );
        assert!(
            loaded.nodes.contains_key("anchor:kept-edge"),
            "edge-target anchor must be kept"
        );
        assert!(
            loaded.nodes.contains_key("anchor:kept-issues"),
            "multi-issue anchor must be kept"
        );
        assert!(
            loaded.nodes.contains_key("n1"),
            "ordinary node must be kept"
        );

        // anchor_nodes entry for the pruned node must be gone
        assert!(
            !loaded.anchor_nodes.values().any(|v| v == "anchor:pruned"),
            "anchor_nodes must not reference pruned node"
        );
        assert!(
            loaded
                .anchor_nodes
                .values()
                .any(|v| v == "anchor:kept-edge"),
            "anchor_nodes must reference kept edge-target node"
        );

        // issues for kept anchor nodes round-trip; pruned node has no entry
        assert_eq!(
            loaded.issues.get("anchor:kept-issues"),
            Some(&vec![0u32, 1u32])
        );
        assert!(
            !loaded.issues.contains_key("anchor:pruned"),
            "pruned node must have no issues entry"
        );
    }
}
