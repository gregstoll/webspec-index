//! Builds the read-only, chunked database the web UI reads over HTTP range requests.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_PROVIDERS: &[&str] = &["whatwg", "w3c", "tc39"];

pub struct ExportOptions {
    pub providers: Vec<String>,
    /// If non-empty, only specs whose canonical name (upper-case) matches one
    /// of these strings are exported. Case-insensitive.
    pub specs: Vec<String>,
    pub chunk_size: u64,
    pub max_total_bytes: u64,
    pub page_size: u32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            providers: DEFAULT_PROVIDERS.iter().map(|s| s.to_string()).collect(),
            specs: Vec::new(),
            chunk_size: 50 * 1024 * 1024,
            max_total_bytes: 900 * 1024 * 1024,
            page_size: 16384,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportedSpec {
    pub name: String,
    pub provider: String,
    pub sha: String,
    pub commit_date: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub size: u64,
    pub chunk_size: u64,
    pub chunks: u32,
    pub sha256: String,
    pub generated_at: String,
    pub specs: Vec<ExportedSpec>,
}

pub fn export_web(source_db: &Path, out_dir: &Path, options: &ExportOptions) -> Result<Manifest> {
    fs::create_dir_all(out_dir)?;
    let work = out_dir.join("webspec.db.tmp");
    if work.exists() {
        fs::remove_file(&work)?;
    }

    let source = Connection::open_with_flags(source_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", source_db.display()))?;
    ensure_prepared_effects(&source)?;
    source.execute(&format!("VACUUM INTO '{}'", work.display()), [])?;
    drop(source);

    let conn = Connection::open(&work)?;
    // Pruning changes the corpus; the publication stays valid for the specs that
    // remain, and every fragment is kept so read-time linking still sees the
    // callees of pruned specs, whose snapshots are gone.
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    prune(&conn, &options.providers, &options.specs)?;
    conn.execute_batch(
        "UPDATE effect_publication SET frozen = 1;
         DROP TABLE IF EXISTS effect_structures;
         DROP TABLE IF EXISTS markdown_memo;
         DROP TABLE IF EXISTS update_checks;
         DROP TABLE IF EXISTS state_models;
         INSERT INTO sections_fts(sections_fts) VALUES('optimize');",
    )?;
    let specs = exported_specs(&conn)?;
    conn.execute_batch(&format!(
        "PRAGMA page_size = {}; VACUUM;",
        options.page_size
    ))?;
    drop(conn);

    let manifest = write_chunks(&work, out_dir, options, specs)?;
    fs::remove_file(&work)?;
    fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(manifest)
}

/// The export serves the publication for the bundled catalog and the default
/// environment, so it must be current for those.
fn ensure_prepared_effects(conn: &Connection) -> Result<()> {
    use crate::effects::service::{catalog_digest, publication_is_current};
    let digest = catalog_digest(&[]).map_err(|e| anyhow::anyhow!(e.message))?;
    let environment = crate::effects::EffectsOptions::default().environment;
    let current = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='effect_publication')",
            [],
            |row| row.get::<_, bool>(0),
        )?
        .then(|| publication_is_current(conn, &digest, &environment))
        .transpose()
        .map_err(|e| anyhow::anyhow!(e.message))?
        .unwrap_or(false);
    if !current {
        bail!("effects are not published for the current index; run `webspec-index effects --all` first");
    }
    Ok(())
}

fn prune(conn: &Connection, providers: &[String], specs: &[String]) -> Result<()> {
    let prov_placeholders = vec!["?"; providers.len()].join(", ");
    let mut params: Vec<String> = providers.to_vec();

    // When a spec filter is given, build a second IN clause on the spec name
    // (case-insensitive via UPPER()).
    let spec_clause = if specs.is_empty() {
        String::new()
    } else {
        let spec_placeholders = vec!["?"; specs.len()].join(", ");
        params.extend(specs.iter().map(|s| s.to_uppercase()));
        format!(" OR UPPER(sp.name) NOT IN ({spec_placeholders})")
    };

    let sql = format!(
        "CREATE TEMP TABLE doomed_snapshots AS \
         SELECT sn.id FROM snapshots sn JOIN specs sp ON sp.id = sn.spec_id \
         WHERE sn.pr_number IS NOT NULL OR sn.sha NOT LIKE 'hash:%' \
         OR sp.provider NOT IN ({prov_placeholders}){spec_clause}"
    );
    conn.execute(&sql, rusqlite::params_from_iter(&params))?;
    // Build per-snapshot DELETEs from STATE_TABLES (single source of truth).
    let mut batch = crate::db::state::STATE_TABLES
        .iter()
        .map(|t| format!("DELETE FROM {t} WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);"))
        .collect::<Vec<_>>()
        .join("\n         ");
    batch.push_str(
        "\n         DELETE FROM sections WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM refs WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM idl_defs WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM effect_anchors WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM effect_structures WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM snapshots WHERE id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM specs WHERE id NOT IN (SELECT spec_id FROM snapshots);
         DELETE FROM effect_summaries WHERE spec NOT IN (SELECT name FROM specs);
         DROP TABLE doomed_snapshots;",
    );
    conn.execute_batch(&batch)?;
    Ok(())
}

fn exported_specs(conn: &Connection) -> Result<Vec<ExportedSpec>> {
    let mut stmt = conn.prepare(
        "SELECT sp.name, sp.provider, sn.sha, sn.commit_date \
         FROM specs sp JOIN snapshots sn ON sn.spec_id = sp.id \
         ORDER BY sp.name",
    )?;
    let specs = stmt
        .query_map([], |row| {
            Ok(ExportedSpec {
                name: row.get(0)?,
                provider: row.get(1)?,
                sha: row.get(2)?,
                commit_date: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(specs)
}

fn write_chunks(
    work: &Path,
    out_dir: &Path,
    options: &ExportOptions,
    specs: Vec<ExportedSpec>,
) -> Result<Manifest> {
    let data = fs::read(work)?;
    let size = data.len() as u64;
    if size > options.max_total_bytes {
        bail!(
            "export is {size} bytes, exceeds the limit of {} bytes",
            options.max_total_bytes
        );
    }

    for entry in fs::read_dir(out_dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|x| x == "bin") {
            fs::remove_file(entry.path())?;
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let sha256 = format!("{:x}", hasher.finalize());

    let chunk_size = options.chunk_size as usize;
    let chunks_count = data.chunks(chunk_size).count() as u32;
    for (i, chunk) in data.chunks(chunk_size).enumerate() {
        let path = out_dir.join(format!("{i:04}.bin"));
        let mut f = fs::File::create(&path)?;
        f.write_all(chunk)?;
    }

    Ok(Manifest {
        size,
        chunk_size: options.chunk_size,
        chunks: chunks_count,
        sha256,
        generated_at: chrono::Utc::now().to_rfc3339(),
        specs,
    })
}
