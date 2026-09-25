//! Invariant fuzz harness: differential conservation checks between each indexed section's source
//! HTML and what webspec-index returns for it.
//!
//! Design: docs/superpowers/specs/2026-09-25-invariant-fuzzer-design.md
//!
//! ```text
//! cargo run --release --example spec_fuzz -- run [--spec HTML,DOM] [--invariant C1,S3] [--loop]
//! cargo run --release --example spec_fuzz -- report DIR
//! cargo run --release --example spec_fuzz -- replay DIR FINDING_ID
//! cargo run --release --example spec_fuzz -- promote DIR FINDING_ID --name NAME [--status known_bug]
//! cargo run --release --example spec_fuzz -- promote --section SPEC#anchor --invariant C1 --name NAME --status fixed
//! ```
//!
//! The index is opened read-only and never rebuilt.

mod check;
mod index;
mod invariants;
mod oracle;
mod promote;
mod report;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rayon::prelude::*;

use check::{SpecInput, SpecResult};
use index::SpecEntry;
use invariants::{DbView, Invariant};
use report::Manifest;

#[derive(Parser)]
#[command(about = "Invariant fuzz harness for webspec-index (read-only)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One full pass over every testable spec.
    Run {
        #[arg(long, value_delimiter = ',')]
        spec: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        provider: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        invariant: Vec<String>,
        #[arg(long = "skip-invariant", value_delimiter = ',')]
        skip_invariant: Vec<String>,
        /// Specs checked in parallel.
        #[arg(long)]
        jobs: Option<usize>,
        /// After the pass, re-check specs whose snapshot changes.
        #[arg(long = "loop")]
        watch: bool,
        /// Seconds between snapshot polls with --loop.
        #[arg(long, default_value_t = 60)]
        poll: u64,
        /// Check the testable specs even when others are mid-rebuild or stale.
        #[arg(long)]
        allow_partial: bool,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Print a run's findings and totals.
    Report { dir: PathBuf },
    /// Re-run one finding from its stored region.
    Replay { dir: PathBuf, id: String },
    /// Write a regression fixture from a finding, or from any section with --section.
    Promote {
        dir: Option<PathBuf>,
        id: Option<String>,
        /// SPEC#anchor to promote from the index cache instead of a finding.
        #[arg(long)]
        section: Option<String>,
        /// Invariant for --section.
        #[arg(long)]
        invariant: Option<String>,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value = "known-bug")]
        status: promote::Status,
        /// Bug id or description recorded in the fixture.
        #[arg(long, default_value = "")]
        bug: String,
        /// Keep the full region instead of reducing it.
        #[arg(long)]
        no_reduce: bool,
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

thread_local! {
    static LAST_PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

fn main() -> Result<()> {
    // M0 provokes panics on purpose; keep their messages for the report instead of stderr.
    std::panic::set_hook(Box::new(|info| {
        LAST_PANIC.with(|p| *p.borrow_mut() = info.to_string());
    }));
    match Cli::parse().cmd {
        Cmd::Run {
            spec,
            provider,
            invariant,
            skip_invariant,
            jobs,
            watch,
            poll,
            allow_partial,
            out,
            db,
        } => run(RunOpts {
            specs: spec,
            providers: provider,
            invariants: select(&invariant, &skip_invariant)?,
            jobs: jobs.unwrap_or_else(|| num_cpus().min(8)),
            watch,
            poll: Duration::from_secs(poll),
            allow_partial,
            out,
            db: db.unwrap_or_else(index::default_db_path),
        }),
        Cmd::Report { dir } => report::print_report(&dir),
        Cmd::Replay { dir, id } => replay(&dir, &id),
        Cmd::Promote {
            dir,
            id,
            section,
            invariant,
            name,
            status,
            bug,
            no_reduce,
            db,
        } => {
            let req = promote::Request {
                name,
                status,
                bug,
                reduce: !no_reduce,
            };
            let path = match (dir, id, section) {
                (Some(dir), Some(id), None) => {
                    let rec = report::load_finding(&dir, &id)?;
                    let html = rec
                        .region_html
                        .as_deref()
                        .context("the finding's region was too large to store")?;
                    promote::promote_html(
                        &req,
                        &rec.invariant,
                        &rec.spec,
                        &rec.base_url,
                        &rec.anchor,
                        html,
                        Some(&rec.signature),
                    )?
                }
                (None, None, Some(section)) => {
                    let inv = invariant.context("--section needs --invariant")?;
                    promote_section(
                        &req,
                        &section,
                        &inv,
                        &db.unwrap_or_else(index::default_db_path),
                    )?
                }
                _ => bail!("promote takes either DIR FINDING_ID or --section SPEC#anchor"),
            };
            println!("wrote {}", path.display());
            Ok(())
        }
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn select(only: &[String], skip: &[String]) -> Result<Vec<&'static dyn Invariant>> {
    for id in only.iter().chain(skip) {
        if invariants::by_id(id).is_none() {
            bail!("unknown invariant {id}");
        }
    }
    Ok(invariants::all()
        .into_iter()
        .filter(|i| only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(i.id())))
        .filter(|i| !skip.iter().any(|o| o.eq_ignore_ascii_case(i.id())))
        .collect())
}

struct RunOpts {
    specs: Vec<String>,
    providers: Vec<String>,
    invariants: Vec<&'static dyn Invariant>,
    jobs: usize,
    watch: bool,
    poll: Duration,
    allow_partial: bool,
    out: Option<PathBuf>,
    db: PathBuf,
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn harness_rev() -> (String, bool) {
    let rev = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    (rev, dirty)
}

fn db_dir(db: &Path) -> PathBuf {
    db.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn testable(opts: &RunOpts, census: &index::Census) -> Vec<SpecEntry> {
    census
        .testable
        .iter()
        .filter(|e| {
            opts.specs.is_empty() || opts.specs.iter().any(|s| s.eq_ignore_ascii_case(&e.name))
        })
        .filter(|e| {
            opts.providers.is_empty()
                || opts
                    .providers
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(&e.provider))
        })
        .cloned()
        .collect()
}

fn run(opts: RunOpts) -> Result<()> {
    let conn = index::open_ro(&opts.db)?;
    let census = index::census(&conn, &db_dir(&opts.db))?;
    census.print();
    let index_version = index::check_index_version(&conn)?;
    let partial = census.partial();
    if !partial.is_empty() && !opts.allow_partial {
        bail!(
            "the index is partially built or stale: {} specs fail G2/G3 (e.g. {}: {}). \
             Finish the rebuild, or pass --allow-partial to check the {} testable specs",
            partial.len(),
            partial[0].name,
            partial[0].reason,
            census.testable.len()
        );
    }
    let entries = testable(&opts, &census);
    if entries.is_empty() {
        bail!("no testable spec matches the filters");
    }
    drop(conn);

    let run_id = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = opts.out.clone().unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts/spec-fuzz")
            .join(&run_id)
    });
    let (rev, dirty) = harness_rev();
    let mut manifest = Manifest {
        run_id,
        harness_rev: rev,
        dirty,
        index_version,
        structure_version: webspec_index::parse::steps::STRUCTURE_VERSION.to_string(),
        db: opts.db.display().to_string(),
        invariants: opts.invariants.iter().map(|i| i.id().to_string()).collect(),
        snapshots: BTreeMap::new(),
    };
    eprintln!("writing results to {}", dir.display());

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.jobs)
        .build()?;
    let started = Instant::now();
    let mut results: BTreeMap<String, SpecResult> = BTreeMap::new();
    let mut pinned: BTreeMap<String, String> = BTreeMap::new();
    let mut pending = entries;
    loop {
        for e in &pending {
            manifest.snapshots.insert(
                e.name.clone(),
                (e.base_url.clone(), e.provider.clone(), e.sha.clone()),
            );
            pinned.insert(e.name.clone(), e.sha.clone());
        }
        for r in check_all(&pool, &pending, &opts) {
            results.insert(r.spec.clone(), r);
        }
        let ordered: Vec<SpecResult> = std::mem::take(&mut results).into_values().collect();
        let (store, summary) = report::build(
            &ordered,
            &census,
            &manifest,
            &dir,
            started.elapsed().as_secs_f64(),
        );
        results = ordered.into_iter().map(|r| (r.spec.clone(), r)).collect();
        report::write(&dir, &manifest, &store, &summary)?;
        report::print_report(&dir)?;
        if !opts.watch {
            return Ok(());
        }
        pending = loop {
            std::thread::sleep(opts.poll);
            let Ok(conn) = index::open_ro(&opts.db) else {
                continue;
            };
            if let Err(e) = index::check_index_version(&conn) {
                eprintln!("waiting: {e}");
                continue;
            }
            let Ok(c) = index::census(&conn, &db_dir(&opts.db)) else {
                continue;
            };
            let changed: Vec<SpecEntry> = testable(&opts, &c)
                .into_iter()
                .filter(|e| pinned.get(&e.name) != Some(&e.sha))
                .collect();
            if !changed.is_empty() {
                eprintln!("{} specs changed; re-checking", changed.len());
                break changed;
            }
        };
    }
}

fn check_all(pool: &rayon::ThreadPool, entries: &[SpecEntry], opts: &RunOpts) -> Vec<SpecResult> {
    let mut order: Vec<&SpecEntry> = entries.iter().collect();
    // Largest first, so one big spec does not start last and bound the wall time.
    order.sort_by_key(|e| {
        std::cmp::Reverse(
            std::fs::metadata(&e.cache_path)
                .map(|m| m.len())
                .unwrap_or(0),
        )
    });
    let done = AtomicUsize::new(0);
    let total = order.len();
    let mut results: Vec<SpecResult> = pool.install(|| {
        order
            .par_iter()
            .with_max_len(1)
            .map(|e| {
                let t = Instant::now();
                let r = check_one(e, opts);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!(
                    "[{n}/{total}] {} sections={} findings={} {:.1}s{}",
                    e.name,
                    r.sections,
                    r.findings.len(),
                    t.elapsed().as_secs_f64(),
                    r.skipped
                        .as_deref()
                        .map(|s| format!(" SKIPPED: {s}"))
                        .unwrap_or_default()
                );
                r
            })
            .collect()
    });
    results.sort_by(|a, b| a.spec.cmp(&b.spec));
    results
}

fn check_one(e: &SpecEntry, opts: &RunOpts) -> SpecResult {
    let skipped = |why: String| SpecResult {
        spec: e.name.clone(),
        skipped: Some(why),
        ..Default::default()
    };
    let html = match index::read_cache(e) {
        Ok(h) => h,
        Err(err) => return skipped(err.to_string()),
    };
    let (conn, stored) = match index::open_ro(&opts.db).and_then(|c| {
        let s = index::load_stored(&c, e.snapshot_id)?;
        Ok((c, s))
    }) {
        Ok(x) => x,
        Err(err) => return skipped(format!("index busy or unreadable: {err}")),
    };
    let input = SpecInput {
        spec: &e.name,
        base_url: &e.base_url,
        sha: &e.sha,
        html: &html,
        db: Some(DbView {
            conn,
            snapshot_id: e.snapshot_id,
        }),
        stored: Some(stored),
        with_structure: e.has_structure,
    };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check::check_spec(input, &opts.invariants)
    }));
    let r = match r {
        Ok(r) => r,
        Err(_) => {
            return skipped(format!(
                "panic: {}",
                LAST_PANIC.with(|p| p.borrow().clone())
            ))
        }
    };
    let now = index::open_ro(&opts.db).and_then(|c| index::current_sha(&c, &e.name));
    match now {
        Ok(Some(sha)) if sha == e.sha => r,
        Ok(other) => skipped(format!(
            "snapshot changed mid-pass ({} -> {other:?})",
            e.sha
        )),
        Err(err) => skipped(format!("index busy on re-check: {err}")),
    }
}

fn replay(dir: &Path, id: &str) -> Result<()> {
    let rec = report::load_finding(dir, id)?;
    let (rev, dirty) = harness_rev();
    if rev != rec.harness_rev || dirty != rec.dirty {
        eprintln!(
            "warning: finding from harness {}{}, now {rev}{}",
            rec.harness_rev,
            if rec.dirty { "+dirty" } else { "" },
            if dirty { "+dirty" } else { "" }
        );
    }
    if rec.index_version != webspec_index::parse::INDEX_VERSION {
        eprintln!(
            "warning: finding from index version {}, harness is {}",
            rec.index_version,
            webspec_index::parse::INDEX_VERSION
        );
    }
    if let Ok(Some(sha)) =
        index::open_ro(&index::default_db_path()).and_then(|c| index::current_sha(&c, &rec.spec))
    {
        if sha != rec.snapshot {
            eprintln!(
                "warning: {} snapshot is now {sha}, finding was {}",
                rec.spec, rec.snapshot
            );
        }
    }
    let inv = invariants::by_id(&rec.invariant).context("unknown invariant")?;
    if inv.spec_level() || matches!(inv.id(), "M1" | "Q1") {
        bail!(
            "{} needs the index; re-run `run --spec {} --invariant {}`",
            inv.id(),
            rec.spec,
            inv.id()
        );
    }
    let html = rec
        .region_html
        .as_deref()
        .context("the finding's region was too large to store")?;
    println!(
        "{} {} {}#{} ({})",
        rec.id, rec.signature, rec.spec, rec.anchor, rec.shape
    );
    check::with_section(html, &rec.spec, &rec.base_url, &rec.anchor, |s| {
        match inv.check(invariants::Ctx::Section(s)) {
            invariants::Outcome::Pass => println!("PASS: the finding no longer reproduces"),
            invariants::Outcome::NotApplicable => println!("not applicable on the stored region"),
            invariants::Outcome::Fail(f) => {
                println!("FAIL\nexpected: {}\nactual:   {}", f.expected, f.actual);
                for e in &f.items {
                    println!("  {} {} [{}]", e.kind, e.item, e.construct);
                    if let Some(x) = &e.source_excerpt {
                        println!("    source:   {x}");
                    }
                    if let Some(x) = &e.rendered_excerpt {
                        println!("    rendered: {x}");
                    }
                }
            }
        }
        println!("--- content ---\n{}", oracle::excerpt(s.content(), 4000));
    })
}

fn promote_section(
    req: &promote::Request,
    section: &str,
    invariant: &str,
    db: &Path,
) -> Result<PathBuf> {
    let (spec, anchor) = section
        .split_once('#')
        .context("--section takes SPEC#anchor")?;
    let conn = index::open_ro(db)?;
    index::check_index_version(&conn)?;
    let census = index::census(&conn, &db_dir(db))?;
    let entry = census
        .testable
        .iter()
        .find(|e| e.name == spec)
        .with_context(|| format!("{spec} is not testable in this index"))?;
    let html = index::read_cache(entry)?;
    let region = check::with_section(&html, spec, &entry.base_url, anchor, |s| {
        s.fixture_html_for(invariant)
    })?;
    promote::promote_html(req, invariant, spec, &entry.base_url, anchor, &region, None)
}
