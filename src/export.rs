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
    pub chunk_size: u64,
    pub max_total_bytes: u64,
    pub page_size: u32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            providers: DEFAULT_PROVIDERS.iter().map(|s| s.to_string()).collect(),
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
    // Pruning fires the effects invalidation triggers on specs and snapshots, which
    // would mark the prepared run stale in the export. The run stays valid for the
    // specs that remain, so the counter is put back afterwards.
    let generation = crate::db::effects::generation(&conn)?;
    prune(&conn, &options.providers)?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS effect_structures;
         DROP TABLE IF EXISTS effect_local_matches;
         DROP TABLE IF EXISTS update_checks;
         UPDATE effect_runs SET artifact_json = '';
         INSERT INTO sections_fts(sections_fts) VALUES('optimize');",
    )?;
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'effects_generation'",
        [generation],
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

fn ensure_prepared_effects(conn: &Connection) -> Result<()> {
    let prepared: i64 = conn.query_row(
        "SELECT COUNT(*) FROM effect_runs r
         WHERE r.generation = (SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'effects_generation')
           AND EXISTS (SELECT 1 FROM effect_subjects s WHERE s.analysis_id = r.analysis_id)",
        [],
        |row| row.get(0),
    )?;
    if prepared == 0 {
        bail!("no prepared effects for the current index; run `webspec-index effects --all --summary-only` first");
    }
    Ok(())
}

fn prune(conn: &Connection, providers: &[String]) -> Result<()> {
    let placeholders = vec!["?"; providers.len()].join(", ");
    let sql = format!(
        "CREATE TEMP TABLE doomed_snapshots AS \
         SELECT sn.id FROM snapshots sn JOIN specs sp ON sp.id = sn.spec_id \
         WHERE sn.pr_number IS NOT NULL OR sn.sha NOT LIKE 'hash:%' \
         OR sp.provider NOT IN ({placeholders})"
    );
    conn.execute(&sql, rusqlite::params_from_iter(providers))?;
    conn.execute_batch(
        "DELETE FROM sections WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM refs WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM idl_defs WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM effect_anchors WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM effect_structures WHERE snapshot_id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM snapshots WHERE id IN (SELECT id FROM doomed_snapshots);
         DELETE FROM specs WHERE id NOT IN (SELECT spec_id FROM snapshots);
         DROP TABLE doomed_snapshots;",
    )?;
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
