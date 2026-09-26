//! Corpus parity harness: each iteration edits one spec's cached HTML, refreshes copy A
//! incrementally, rebuilds copy B from scratch, and compares them with the `dump` function.
//!
//! Usage:
//!   cargo run --release --example incremental_parity -- \
//!     --source-db /path/to/backup.db --work /tmp/si-parity --iterations 10 --seed 1
//!
//! Safety: refuses to start when both --source-db and --work resolve inside
//! `~/.webspec-index`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::Connection;
use webspec_index::db;
use webspec_index::effects::bundled::default_catalog;
use webspec_index::effects::model::EffectsOptions;
use webspec_index::effects::service::{publish, PublishMode};
use webspec_index::fetch::freshness::FreshnessOptions;
use webspec_index::fetch::testing::HttpStub;
use webspec_index::fetch::{html_cache_path, reparse_specs};
use webspec_index::refresh::{refresh, EffectsRefresh, LockPolicy, RefreshOptions};

#[derive(Parser)]
#[command(about = "Corpus parity: incremental refresh vs. full rebuild under random HTML edits")]
struct Args {
    #[arg(long)]
    source_db: PathBuf,
    #[arg(long)]
    work: PathBuf,
    #[arg(long, default_value_t = 20)]
    iterations: u64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

/// Backup `src` into a fresh SQLite DB at `dest_path`.
fn backup_db(src: &Connection, dest_path: &Path) -> Result<Connection> {
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut dest = Connection::open(dest_path)?;
    {
        let backup =
            rusqlite::backup::Backup::new(src, &mut dest).context("backup::Backup::new")?;
        backup
            .run_to_completion(1000, std::time::Duration::ZERO, None)
            .context("backup run_to_completion")?;
    }
    Ok(dest)
}

/// List the current trunk spec corpus: `(spec_name, host_and_path, content_hash)`.
fn corpus_entries(conn: &Connection) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT sp.name, sp.base_url, uc.content_hash, uc.etag
         FROM specs sp
         JOIN update_checks uc ON uc.spec_id = sp.id
         WHERE uc.content_hash IS NOT NULL
         ORDER BY sp.name",
    )?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Read the cached HTML for a spec.
fn read_html(source_dir: &Path, spec: &str, content_hash: &str) -> Option<String> {
    let path = html_cache_path(source_dir, spec, content_hash);
    std::fs::read_to_string(&path).ok()
}

/// Mutate an HTML string: apply one of five DOM-level edits using string operations.
///
/// The edit type is chosen by `rng.below(5)`:
///   0 — change a section's text (add a word to a `<p>`)
///   1 — insert a new dummy `<h2>` heading with a unique id
///   2 — delete an `<li>` step from the first `<ol>` found
///   3 — rename a `<dfn id=...>` anchor (appends "-x")
///   4 — no-op (returns unchanged to test pass-through)
fn mutate(html: &str, rng: &mut Rng) -> (String, &'static str) {
    match rng.below(5) {
        0 => {
            if let Some(pos) = html.find("<p>") {
                let (before, after) = html.split_at(pos + 3);
                return (format!("{before}[parity-edit] {after}"), "text-change");
            }
            (html.to_owned(), "text-change-noop")
        }
        1 => {
            let id = format!("parity-section-{}", rng.next());
            let inserted = format!("<h2 id=\"{id}\">Parity section</h2>\n");
            let (before, after) = html.split_at(html.len() / 2);
            (format!("{before}{inserted}{after}"), "insert-section")
        }
        2 => {
            if let Some(start) = html.find("<li>") {
                if let Some(end) = html[start..].find("</li>") {
                    let (before, rest) = html.split_at(start);
                    let after = &rest[end + 5..];
                    return (format!("{before}{after}"), "delete-step");
                }
            }
            (html.to_owned(), "delete-step-noop")
        }
        3 => {
            if let Some(pos) = html.find(" id=\"") {
                let after_quote = pos + 5;
                if let Some(end) = html[after_quote..].find('"') {
                    let old_id = &html[after_quote..after_quote + end];
                    let new_id = format!("{old_id}-x");
                    return (html.replacen(old_id, &new_id, 1), "rename-anchor");
                }
            }
            (html.to_owned(), "rename-anchor-noop")
        }
        _ => (html.to_owned(), "noop"),
    }
}

/// Dump comparable DB state filtered to `spec`.
fn dump_for_spec(conn: &Connection, spec: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let q = format!("'{spec}'");
    for (table, query) in [
        (
            "sections",
            format!(
                "SELECT sp.name, s.anchor, s.title, s.content_text, s.section_type,
                        s.parent_anchor, s.prev_anchor, s.next_anchor, s.depth, s.number, s.ord
                 FROM sections s JOIN snapshots sn ON s.snapshot_id=sn.id JOIN specs sp ON sn.spec_id=sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN ({q})
                 ORDER BY sp.name, s.anchor"
            ),
        ),
        (
            "effect_structures",
            format!(
                "SELECT sp.name, e.representation_version
                 FROM effect_structures e JOIN snapshots sn ON e.snapshot_id=sn.id JOIN specs sp ON sn.spec_id=sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN ({q})
                 ORDER BY sp.name"
            ),
        ),
        (
            "effect_summaries",
            format!(
                "SELECT subject_key, spec, digest FROM effect_summaries WHERE spec IN ({q}) ORDER BY subject_key"
            ),
        ),
        (
            "effect_fragments",
            format!(
                "SELECT spec, config_key FROM effect_fragments WHERE spec IN ({q}) ORDER BY spec"
            ),
        ),
    ] {
        let mut rows = Vec::new();
        if let Ok(mut stmt) = conn.prepare(&query) {
            let col_count = stmt.column_count();
            if let Ok(mapped) = stmt.query_map([], |row| {
                Ok((0..col_count)
                    .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                    .collect::<Vec<_>>()
                    .join("|"))
            }) {
                rows = mapped.filter_map(|r| r.ok()).collect();
            }
        }
        rows.sort();
        out.insert(table.to_string(), rows);
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let home_index = db::get_db_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let source_canonical = args
        .source_db
        .canonicalize()
        .unwrap_or_else(|_| args.source_db.clone());
    let work_canonical = args
        .work
        .canonicalize()
        .unwrap_or_else(|_| args.work.clone());
    if source_canonical.starts_with(&home_index) && work_canonical.starts_with(&home_index) {
        bail!(
            "--source-db and --work both resolve inside ~/.webspec-index; use a backup directory"
        );
    }

    let source_conn = Connection::open(&args.source_db)
        .with_context(|| format!("opening {}", args.source_db.display()))?;
    let source_html_dir = args
        .source_db
        .parent()
        .unwrap_or(Path::new("."))
        .join("html");

    let work = &args.work;
    std::fs::create_dir_all(work)?;
    let dir_a = work.join("a");
    let dir_b = work.join("b");
    std::fs::create_dir_all(&dir_a)?;
    std::fs::create_dir_all(&dir_b)?;

    println!("Backing up source DB to A and B…");
    let t = Instant::now();
    let conn_a = backup_db(&source_conn, &dir_a.join("index.db"))?;
    let conn_b = backup_db(&source_conn, &dir_b.join("index.db"))?;
    println!("  backup: {:.1}s", t.elapsed().as_secs_f64());

    let html_a = dir_a.join("html");
    let html_b = dir_b.join("html");
    std::fs::create_dir_all(&html_a)?;
    std::fs::create_dir_all(&html_b)?;

    let corpus = corpus_entries(&source_conn)?;
    if corpus.is_empty() {
        bail!("source DB has no indexed specs with cached HTML");
    }

    let mut rng = Rng::new(args.seed);
    let mut total_mismatches = 0u64;

    for iteration in 0..args.iterations {
        let pick = rng.below(corpus.len() as u64) as usize;
        let (spec, base_url, content_hash, etag) = &corpus[pick];

        let html = match read_html(&source_html_dir, spec, content_hash) {
            Some(h) => h,
            None => {
                println!("[{iteration}] {spec}: no cached HTML, skipping");
                continue;
            }
        };

        let (mutated_html, edit_kind) = mutate(&html, &mut rng);
        let host_and_path = base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned();
        let new_hash = format!("hash:parity-{iteration}");

        println!("[{iteration}] {spec} ({edit_kind})");

        let stub = HttpStub::start();
        let etag_str = if etag.is_empty() {
            format!("W/\"parity-{iteration}\"")
        } else {
            format!("{etag}-parity-{iteration}")
        };
        stub.put(&host_and_path, &mutated_html, Some(&etag_str), None);

        let catalog = default_catalog(&[]).context("default_catalog")?;
        let options = EffectsOptions::default();

        let refresh_opts = RefreshOptions {
            freshness: FreshnessOptions {
                origin: Some(stub.origin()),
                ..FreshnessOptions::default()
            },
            check_interval: chrono::Duration::zero(),
            ..RefreshOptions::new(LockPolicy::Block, EffectsRefresh::Always)
        };

        let html_a_spec = html_a.join(spec);
        std::fs::create_dir_all(&html_a_spec)?;
        std::fs::write(html_a_spec.join(format!("{new_hash}.html")), &mutated_html)?;
        let html_b_spec = html_b.join(spec);
        std::fs::create_dir_all(&html_b_spec)?;
        std::fs::write(html_b_spec.join(format!("{new_hash}.html")), &mutated_html)?;

        let t_refresh = Instant::now();
        let report_a = refresh(&conn_a, std::slice::from_ref(spec), &refresh_opts)
            .await
            .context("refresh A")?;
        let refresh_ms = t_refresh.elapsed().as_millis();

        let t_reparse = Instant::now();
        let spec_path_b = dir_b.join("index.db");
        let _ = spec_path_b;
        {
            reparse_specs(&conn_b, Some(spec), &[])
                .await
                .context("reparse B")?;
            publish(
                &conn_b,
                &catalog,
                &options,
                PublishMode::Rebuild,
                BTreeMap::new(),
            )
            .map_err(|e| anyhow::anyhow!("{}", e.message))?;
        }
        let rebuild_ms = t_reparse.elapsed().as_millis();

        let dump_a = dump_for_spec(&conn_a, spec);
        let dump_b = dump_for_spec(&conn_b, spec);

        let mut mismatches = 0usize;
        for (table, rows_b) in &dump_b {
            let rows_a = dump_a.get(table).cloned().unwrap_or_default();
            if rows_a != *rows_b {
                mismatches += 1;
                let diff: Vec<_> = rows_b
                    .iter()
                    .filter(|r| !rows_a.contains(r))
                    .take(3)
                    .collect();
                println!("  MISMATCH {table}: +{} rows in B", diff.len());
                for d in diff {
                    println!("    + {}", &d[..d.len().min(120)]);
                }
            }
        }
        if mismatches == 0 {
            println!(
                "  OK  refresh={refresh_ms}ms rebuild={rebuild_ms}ms A={:?}",
                report_a.effects
            );
        } else {
            total_mismatches += 1;
            println!("  FAIL {mismatches} table(s) differ");
        }
    }

    if total_mismatches > 0 {
        bail!("{total_mismatches} iterations had mismatches");
    }
    println!("All {} iterations matched.", args.iterations);
    Ok(())
}
