//! Refresh of a set of specs under the refresh lock: one freshness batch, a
//! parallel parse of what changed, a write per changed spec, then the effects
//! publication.

use crate::db::lock::{self, RefreshGuard};
use crate::db::queries;
use crate::effects::catalog::Catalog;
use crate::effects::service::{self, PublishMode};
use crate::effects::{default_catalog, EffectsOptions, RequestError};
use crate::fetch::freshness::{check_batch, Freshness, FreshnessOptions};
use crate::fetch::pipeline::{self, FragmentContext, ParseJob};
use crate::fetch::{self, ChangedHtml};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a waiting refresh polls the lock.
pub(crate) const LOCK_POLL: Duration = Duration::from_millis(100);
/// How often a running refresh renews the lock; well under [`lock::STALE_AFTER`].
const TOUCH_EVERY: Duration = Duration::from_secs(30);
/// Most dependencies refreshed with a queried spec.
pub const DEPENDENCY_CAP: usize = 64;
const DEFAULT_INLINE_BUDGET_MS: u64 = 5000;

pub enum LockPolicy {
    /// Wait at most this long for the lock (queries, LSP).
    Wait(Duration),
    /// Wait until the lock is free (`update`, `reparse`, `effects --all`).
    Block,
}

#[derive(Debug, Clone, Copy)]
pub enum EffectsRefresh {
    Off,
    /// Publish when the estimated rebuild fits in `budget`.
    Inline {
        budget: Duration,
    },
    Always,
}

pub struct RefreshOptions {
    pub lock: LockPolicy,
    pub effects: EffectsRefresh,
    pub effects_options: EffectsOptions,
    pub freshness: FreshnessOptions,
    /// A spec checked more recently than this is not checked again.
    pub check_interval: chrono::Duration,
    /// The pid the lock is claimed for.
    pub pid: u32,
}

impl RefreshOptions {
    /// Default effects and freshness options, a 24 h check interval, this
    /// process's pid.
    pub fn new(lock: LockPolicy, effects: EffectsRefresh) -> Self {
        Self {
            lock,
            effects,
            effects_options: EffectsOptions::default(),
            freshness: FreshnessOptions::default(),
            check_interval: chrono::Duration::hours(24),
            pid: std::process::id(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct RefreshReport {
    pub checked: Vec<String>,
    pub not_modified: Vec<String>,
    pub same_content: Vec<String>,
    pub failed: Vec<String>,
    pub parsed: Vec<String>,
    pub fragments_built: Vec<String>,
    pub effects: EffectsOutcome,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub enum EffectsOutcome {
    #[default]
    NotRequested,
    Current,
    Rebuilt,
    OverBudget {
        estimate_ms: u64,
    },
    LockTimeout,
    SnapshotChanged,
}

/// The inline effects budget: `WEBSPEC_EFFECTS_INLINE_BUDGET_MS`, default 5 s.
pub fn inline_budget() -> Duration {
    Duration::from_millis(
        std::env::var("WEBSPEC_EFFECTS_INLINE_BUDGET_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_INLINE_BUDGET_MS),
    )
}

/// Estimated effects rebuild over the stale specs' raw structure sizes:
/// `1.0 s + 30 ms/MB × max(largest_MB, total_MB / threads) + 5 ms/MB × total_MB`.
pub fn estimate_rebuild(stale_structure_bytes: &[u64], threads: usize) -> Duration {
    let mb = |bytes: u64| bytes as f64 / 1_000_000.0;
    let largest = stale_structure_bytes.iter().copied().max().map_or(0.0, mb);
    let total: f64 = stale_structure_bytes.iter().copied().map(mb).sum();
    let parallel = largest.max(total / threads.max(1) as f64);
    Duration::from_secs_f64(1.0 + 0.030 * parallel + 0.005 * total)
}

/// The specs `spec` depends on, breadth first over `effect_spec_deps`, each
/// node's callees by descending edge count then name, at most `cap`.
pub fn dependency_closure(
    conn: &Connection,
    spec: &str,
    cap: usize,
) -> anyhow::Result<Vec<String>> {
    let deps = crate::db::effects::load_spec_deps(conn)?;
    let mut seen = HashSet::from([spec.to_owned()]);
    let mut queue = VecDeque::from([spec.to_owned()]);
    let mut closure = Vec::new();
    while let Some(node) = queue.pop_front() {
        let Some(callees) = deps.get(&node) else {
            continue;
        };
        let mut callees: Vec<&(String, u64)> = callees.iter().collect();
        callees.sort_by(|(a, a_edges), (b, b_edges)| b_edges.cmp(a_edges).then_with(|| a.cmp(b)));
        for (callee, _) in callees {
            if closure.len() == cap {
                return Ok(closure);
            }
            if seen.insert(callee.clone()) {
                closure.push(callee.clone());
                queue.push_back(callee.clone());
            }
        }
    }
    Ok(closure)
}

/// Refresh `spec` and its dependency closure.
pub async fn refresh_for_query(
    conn: &Connection,
    spec: &str,
    options: &RefreshOptions,
) -> anyhow::Result<RefreshReport> {
    let mut specs = vec![spec.to_owned()];
    specs.extend(dependency_closure(conn, spec, DEPENDENCY_CAP)?);
    refresh(conn, &specs, options).await
}

/// A spec to refresh. ITU specs have their own sync path and are never targets.
struct Target {
    spec_id: i64,
    name: String,
    base_url: String,
    provider: String,
}

fn load_targets(conn: &Connection, specs: &[String]) -> anyhow::Result<Vec<Target>> {
    let mut targets = Vec::with_capacity(specs.len());
    for spec in specs {
        let Some((name, base_url, provider)) = queries::get_spec_meta(conn, spec)? else {
            continue;
        };
        if provider == "itu" {
            continue;
        }
        let spec_id: Option<i64> = conn
            .query_row("SELECT id FROM specs WHERE name=?1", [&name], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(spec_id) = spec_id {
            targets.push(Target {
                spec_id,
                name,
                base_url,
                provider,
            });
        }
    }
    Ok(targets)
}

/// What a target's stored index says about the need to check it.
struct Standing {
    snapshot: Option<i64>,
    state: Option<queries::UpdateCheckState>,
    /// A snapshot parsed by this build, reusable as is.
    index_current: bool,
    /// Checked within the check interval.
    recently_checked: bool,
}

fn standing(
    conn: &Connection,
    target: &Target,
    now: &DateTime<Utc>,
    check_interval: chrono::Duration,
) -> anyhow::Result<Standing> {
    let snapshot = queries::get_snapshot(conn, &target.name)?;
    let state = queries::get_update_check(conn, target.spec_id)?;
    let index_current = match snapshot {
        Some(snapshot_id) => {
            state.as_ref().is_some_and(fetch::index_is_current)
                && fetch::snapshot_is_reusable(conn, snapshot_id)?
        }
        None => false,
    };
    let recently_checked = state
        .as_ref()
        .is_some_and(|state| now.signed_duration_since(state.last_checked) < check_interval);
    Ok(Standing {
        snapshot,
        state,
        index_current,
        recently_checked,
    })
}

/// Whether nothing is left to do: every target current and recently checked,
/// and the publication current when effects are requested.
fn everything_current(
    conn: &Connection,
    targets: &[Target],
    options: &RefreshOptions,
) -> anyhow::Result<bool> {
    let now = Utc::now();
    for target in targets {
        let standing = standing(conn, target, &now, options.check_interval)?;
        if !(standing.index_current && standing.recently_checked) {
            return Ok(false);
        }
    }
    Ok(match options.effects {
        EffectsRefresh::Off => true,
        EffectsRefresh::Inline { .. } | EffectsRefresh::Always => {
            publication_is_current(conn, &options.effects_options)?
        }
    })
}

fn publication_is_current(conn: &Connection, options: &EffectsOptions) -> anyhow::Result<bool> {
    let digest = service::catalog_digest(&options.rule_paths).map_err(request_error)?;
    service::publication_is_current(conn, &digest, &options.environment).map_err(request_error)
}

fn request_error(error: RequestError) -> anyhow::Error {
    anyhow::anyhow!("{}", error.message)
}

/// Claim the refresh lock for this process, waiting as long as it takes.
pub async fn hold_lock(conn: &Connection) -> anyhow::Result<RefreshGuard<'_>> {
    loop {
        if let Some(guard) = lock::try_claim(conn, std::process::id(), Utc::now())? {
            return Ok(guard);
        }
        tokio::time::sleep(LOCK_POLL).await;
    }
}

/// Run `work` while `guard` holds the lock, restarting the lock's age
/// periodically so a long refresh is not taken for a stale one.
pub async fn while_holding<T>(guard: &RefreshGuard<'_>, work: impl Future<Output = T>) -> T {
    let mut touch = tokio::time::interval(TOUCH_EVERY);
    touch.tick().await;
    tokio::pin!(work);
    loop {
        tokio::select! {
            out = &mut work => return out,
            _ = touch.tick() => {
                if let Err(e) = guard.touch(Utc::now()) {
                    eprintln!("warning: could not renew the refresh lock: {e}");
                }
            }
        }
    }
}

enum Claim<'c> {
    Held(RefreshGuard<'c>),
    /// Another process's refresh made every target current.
    Current,
    TimedOut,
}

async fn claim<'c>(
    conn: &'c Connection,
    targets: &[Target],
    options: &RefreshOptions,
) -> anyhow::Result<Claim<'c>> {
    let mut deadline = match options.lock {
        LockPolicy::Wait(budget) => Some(Instant::now() + budget),
        LockPolicy::Block => None,
    };
    loop {
        if let Some(guard) = lock::try_claim(conn, options.pid, Utc::now())? {
            return Ok(Claim::Held(guard));
        }
        let Some(until) = deadline else {
            tokio::time::sleep(LOCK_POLL).await;
            continue;
        };
        if everything_current(conn, targets, options)? {
            return Ok(Claim::Current);
        }
        let now = Instant::now();
        if now < until {
            tokio::time::sleep(LOCK_POLL.min(until - now)).await;
            continue;
        }
        // A spec without a snapshot has nothing to serve: its content is required.
        let mut all_indexed = true;
        for target in targets {
            all_indexed &= queries::get_snapshot(conn, &target.name)?.is_some();
        }
        if all_indexed {
            return Ok(Claim::TimedOut);
        }
        deadline = None;
    }
}

/// Refresh `specs`: check the ones due or not current in one batch, parse and
/// write the changed ones, then bring the effects publication up to date as
/// `options.effects` asks.
pub async fn refresh(
    conn: &Connection,
    specs: &[String],
    options: &RefreshOptions,
) -> anyhow::Result<RefreshReport> {
    let targets = load_targets(conn, specs)?;
    let mut report = RefreshReport::default();
    // Fast path: skip the lock entirely when every target is current and the
    // publication is up to date.  This avoids the two write transactions that
    // `try_claim` + its guard-drop commit on every warm query or `exists`.
    if everything_current(conn, &targets, options)? {
        report.effects = match options.effects {
            EffectsRefresh::Off => EffectsOutcome::NotRequested,
            _ => EffectsOutcome::Current,
        };
        return Ok(report);
    }
    let guard = match claim(conn, &targets, options).await? {
        Claim::Held(guard) => guard,
        Claim::Current => {
            report.effects = match options.effects {
                EffectsRefresh::Off => EffectsOutcome::NotRequested,
                _ => EffectsOutcome::Current,
            };
            return Ok(report);
        }
        Claim::TimedOut => {
            report.effects = EffectsOutcome::LockTimeout;
            return Ok(report);
        }
    };

    while_holding(&guard, async {
        let changed = check_targets(conn, &targets, options, &mut report).await?;
        let mut catalog = None;
        if !changed.is_empty() {
            let context = match options.effects {
                EffectsRefresh::Off => None,
                EffectsRefresh::Inline { .. } | EffectsRefresh::Always => Some(FragmentContext {
                    catalog: load_catalog(&mut catalog, &options.effects_options)?,
                    environment: options.effects_options.environment.clone(),
                }),
            };
            parse_and_write(conn, changed, context, &mut report).await?;
        }
        report.effects = refresh_effects(conn, options, &mut catalog)?;
        anyhow::Ok(())
    })
    .await?;

    drop(guard);
    if !report.parsed.is_empty() && report.effects != EffectsOutcome::Rebuilt {
        service::clear_caches();
    }
    Ok(report)
}

/// One freshness batch over the targets that are due or not current; returns
/// the changed documents with their targets and stored snapshots.
async fn check_targets<'t>(
    conn: &Connection,
    targets: &'t [Target],
    options: &RefreshOptions,
    report: &mut RefreshReport,
) -> anyhow::Result<Vec<(&'t Target, Option<i64>, ChangedHtml)>> {
    let now = Utc::now();
    let mut candidates = Vec::new();
    let mut checked = Vec::new();
    for target in targets {
        let standing = standing(conn, target, &now, options.check_interval)?;
        if standing.index_current && standing.recently_checked {
            continue;
        }
        candidates.push(fetch::candidate_for(
            target.spec_id,
            &target.name,
            &target.base_url,
            &target.provider,
            standing.state.as_ref(),
            standing.index_current,
        ));
        checked.push((target, standing.snapshot));
        report.checked.push(target.name.clone());
    }

    let results = check_batch(&candidates, &options.freshness).await;
    let mut changed = Vec::new();
    for ((candidate, (target, snapshot)), result) in candidates.iter().zip(checked).zip(results) {
        let outcome = match &result {
            Freshness::NotModified { .. } => Some(&mut report.not_modified),
            Freshness::SameContent { .. } => Some(&mut report.same_content),
            Freshness::Failed(_) => Some(&mut report.failed),
            Freshness::Changed { .. } => None,
        };
        if let Some(list) = outcome {
            list.push(target.name.clone());
        }
        if let Some(html) = fetch::apply_freshness(conn, candidate, result, &now)?.print_notes() {
            changed.push((target, snapshot, html));
        }
    }
    Ok(changed)
}

/// Parse the changed documents in parallel chunks and write each chunk's
/// results in target order.
async fn parse_and_write(
    conn: &Connection,
    changed: Vec<(&Target, Option<i64>, ChangedHtml)>,
    fragment: Option<FragmentContext>,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let threads = pipeline::parse_threads();
    let now = Utc::now();
    let mut changed = changed.into_iter().peekable();
    while changed.peek().is_some() {
        let mut jobs = Vec::new();
        let mut pending = Vec::new();
        for (target, snapshot, html) in changed.by_ref().take(pipeline::chunk_len(threads)) {
            jobs.push(ParseJob {
                spec_name: target.name.clone(),
                base_url: target.base_url.clone(),
                html: Arc::new(html.html),
                content_hash: html.content_hash,
                previous_memo: fetch::previous_memo(conn, snapshot)?,
                fragment: fragment.clone(),
                #[cfg(test)]
                test_fail: html.test_fail,
            });
            pending.push((target, snapshot, html.validators));
        }
        let parsed = fetch::parse_chunk(jobs, threads).await;
        for ((target, snapshot, validators), parsed) in pending.into_iter().zip(parsed) {
            let parsed = match parsed {
                Ok(p) => p,
                Err(e) => {
                    if snapshot.is_some() {
                        eprintln!(
                            "note: {}: re-index failed ({e}); serving the cached snapshot",
                            target.name
                        );
                        report.failed.push(target.name.clone());
                        continue;
                    }
                    return Err(e);
                }
            };
            let built_fragment = parsed.fragment.is_some();
            fetch::write_parsed_html(
                conn,
                target.spec_id,
                &target.name,
                &target.base_url,
                &target.provider,
                parsed,
                &now,
                &validators,
            )?;
            report.parsed.push(target.name.clone());
            if built_fragment {
                report.fragments_built.push(target.name.clone());
            }
        }
    }
    Ok(())
}

fn load_catalog(
    slot: &mut Option<Arc<Catalog>>,
    options: &EffectsOptions,
) -> anyhow::Result<Arc<Catalog>> {
    if let Some(catalog) = slot {
        return Ok(catalog.clone());
    }
    let catalog = Arc::new(default_catalog(&options.rule_paths).map_err(request_error)?);
    *slot = Some(catalog.clone());
    Ok(catalog)
}

fn refresh_effects(
    conn: &Connection,
    options: &RefreshOptions,
    catalog: &mut Option<Arc<Catalog>>,
) -> anyhow::Result<EffectsOutcome> {
    if matches!(options.effects, EffectsRefresh::Off) {
        return Ok(EffectsOutcome::NotRequested);
    }
    if publication_is_current(conn, &options.effects_options)? {
        return Ok(EffectsOutcome::Current);
    }
    let catalog = load_catalog(catalog, &options.effects_options)?;
    let effects = &options.effects_options;
    match options.effects {
        EffectsRefresh::Off => unreachable!("returned above"),
        EffectsRefresh::Always => service::publish_outcome(service::publish(
            conn,
            &catalog,
            effects,
            PublishMode::Incremental,
            BTreeMap::new(),
        )),
        EffectsRefresh::Inline { budget } => {
            service::publish_within_budget(conn, &catalog, effects, budget)
        }
    }
    .map_err(request_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EffectsMode, EffectsRequest, SubjectSelector, EFFECTS_SCHEMA_VERSION};
    use crate::fetch::testing::HttpStub;

    const DOM_CALLER_ANCHOR: &str = "a-run";
    const BETA: &str = include_str!("../tests/fixtures/effects/multi/beta.html");
    static MULTI: [(&str, &str); 3] = [
        (
            "dom.spec.whatwg.org/",
            include_str!("../tests/fixtures/effects/multi/alpha.html"),
        ),
        ("infra.spec.whatwg.org/", BETA),
        (
            "url.spec.whatwg.org/",
            include_str!("../tests/fixtures/effects/multi/gamma.html"),
        ),
    ];
    static MULTI_INFRA_EDITED: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        BETA.replace("Return a new object.", "Return a new, empty object.")
    });

    fn open(dir: &tempfile::TempDir) -> Connection {
        crate::db::open_db_at(&dir.path().join("index.db")).unwrap()
    }

    fn test_options(stub: &HttpStub, effects: EffectsRefresh) -> RefreshOptions {
        RefreshOptions {
            freshness: FreshnessOptions {
                origin: Some(stub.origin()),
                ..FreshnessOptions::default()
            },
            check_interval: chrono::Duration::zero(),
            ..RefreshOptions::new(LockPolicy::Wait(Duration::from_secs(5)), effects)
        }
    }

    fn preview_request(spec: &str, anchor: &str, mode: EffectsMode) -> EffectsRequest {
        EffectsRequest {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: SubjectSelector {
                spec: spec.into(),
                anchor: anchor.into(),
                step_path: None,
                step_id: None,
                body_id: None,
            },
            options: EffectsOptions {
                mode,
                ..EffectsOptions::default()
            },
            filter: None,
        }
    }

    /// A private index of the three fixture specs served from a stub, with a
    /// current publication.
    async fn indexed_corpus_with_stub(corpus: &[(&str, &str)]) -> (tempfile::TempDir, HttpStub) {
        let stub = HttpStub::start();
        for (host_and_path, html) in corpus {
            stub.put(host_and_path, html, Some("W/\"old\""), None);
        }
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir);
        let mut options = test_options(&stub, EffectsRefresh::Always);
        options.lock = LockPolicy::Block;
        let specs = ["DOM", "INFRA", "URL"].map(String::from);
        let report = refresh(&conn, &specs, &options).await.unwrap();
        assert_eq!(report.parsed, specs);
        assert_eq!(report.effects, EffectsOutcome::Rebuilt);
        let request = preview_request("DOM", DOM_CALLER_ANCHOR, EffectsMode::Cached);
        assert!(service::get_cached_effect_preview_on(&conn, &request)
            .unwrap()
            .is_some());
        (dir, stub)
    }

    #[test]
    fn dependency_closure_is_bfs_ranked_and_capped() {
        let conn = crate::db::open_test_db().unwrap();
        let mut deps = BTreeMap::new();
        deps.insert(
            "HTML".to_string(),
            BTreeMap::from([("DOM".to_string(), 5), ("INFRA".to_string(), 9)]),
        );
        deps.insert(
            "DOM".to_string(),
            BTreeMap::from([("INFRA".to_string(), 3), ("WEBIDL".to_string(), 1)]),
        );
        deps.insert(
            "INFRA".to_string(),
            BTreeMap::from([("HTML".to_string(), 1)]),
        );
        crate::db::effects::store_spec_deps(&conn, &deps).unwrap();
        assert_eq!(
            dependency_closure(&conn, "HTML", 64).unwrap(),
            ["INFRA", "DOM", "WEBIDL"]
        );
        assert_eq!(
            dependency_closure(&conn, "HTML", 2).unwrap(),
            ["INFRA", "DOM"]
        );
        assert!(dependency_closure(&conn, "UNKNOWN", 64).unwrap().is_empty());
    }

    #[test]
    fn estimate_matches_the_spec_table() {
        let mb = |x: f64| (x * 1_000_000.0) as u64;
        let html = estimate_rebuild(&[mb(32.0)], 32);
        assert!((2100..=2300).contains(&html.as_millis()), "{html:?}");
    }

    #[tokio::test]
    async fn no_change_refresh_parses_and_publishes_nothing() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        let conn = open(&dir);
        let report = refresh_for_query(
            &conn,
            "DOM",
            &test_options(
                &stub,
                EffectsRefresh::Inline {
                    budget: Duration::from_secs(60),
                },
            ),
        )
        .await
        .unwrap();
        assert!(!report.checked.is_empty());
        assert!(report.parsed.is_empty() && report.fragments_built.is_empty());
        assert_eq!(report.effects, EffectsOutcome::Current);
    }

    #[tokio::test]
    async fn publication_for_custom_options_is_current_for_matching_query() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        let conn = open(&dir);
        let custom = EffectsOptions {
            environment: "generic".into(),
            ..EffectsOptions::default()
        };
        let publish = RefreshOptions {
            effects_options: custom.clone(),
            ..test_options(&stub, EffectsRefresh::Always)
        };
        let report = refresh_for_query(&conn, "DOM", &publish).await.unwrap();
        assert_eq!(report.effects, EffectsOutcome::Rebuilt);

        let query = RefreshOptions {
            effects_options: custom,
            ..test_options(
                &stub,
                EffectsRefresh::Inline {
                    budget: Duration::from_secs(60),
                },
            )
        };
        let report = refresh_for_query(&conn, "DOM", &query).await.unwrap();
        assert!(report.parsed.is_empty() && report.fragments_built.is_empty());
        assert_eq!(report.effects, EffectsOutcome::Current);
    }

    #[tokio::test]
    async fn changed_callee_is_reparsed_with_the_query_spec() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        stub.put(
            "infra.spec.whatwg.org/",
            &MULTI_INFRA_EDITED,
            Some("W/\"new\""),
            None,
        );
        let conn = open(&dir);
        let report = refresh_for_query(
            &conn,
            "DOM",
            &test_options(
                &stub,
                EffectsRefresh::Inline {
                    budget: Duration::from_secs(60),
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(report.parsed, ["INFRA"]);
        assert_eq!(report.fragments_built, ["INFRA"]);
        assert_eq!(report.effects, EffectsOutcome::Rebuilt);
    }

    #[tokio::test]
    async fn over_budget_reports_unavailable_and_keeps_rows_unserved() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        stub.put(
            "infra.spec.whatwg.org/",
            &MULTI_INFRA_EDITED,
            Some("W/\"new\""),
            None,
        );
        let conn = open(&dir);
        let report = refresh_for_query(
            &conn,
            "DOM",
            &test_options(
                &stub,
                EffectsRefresh::Inline {
                    budget: Duration::ZERO,
                },
            ),
        )
        .await
        .unwrap();
        assert!(matches!(report.effects, EffectsOutcome::OverBudget { .. }));
        let request = preview_request("DOM", DOM_CALLER_ANCHOR, EffectsMode::Cached);
        assert!(service::get_cached_effect_preview_on(&conn, &request)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn second_refresh_waits_then_reports_lock_timeout() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        let holder = open(&dir);
        let _guard = lock::try_claim(&holder, std::process::id(), Utc::now())
            .unwrap()
            .unwrap();
        let conn = open(&dir);
        let mut options = test_options(
            &stub,
            EffectsRefresh::Inline {
                budget: Duration::from_millis(300),
            },
        );
        options.lock = LockPolicy::Wait(Duration::from_millis(300));
        options.pid = std::process::id() + 1;
        let start = Instant::now();
        let report = refresh_for_query(&conn, "DOM", &options).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(300));
        assert_eq!(report.effects, EffectsOutcome::LockTimeout);
        assert!(report.parsed.is_empty(), "the cached snapshot is served");
    }

    #[tokio::test]
    async fn always_refresh_stores_effect_sites_for_inline_built_fragments() {
        // Regression test: when fragments are pre-built inline during parsing
        // (EffectsRefresh::Always → FragmentContext is Some), the subsequent
        // publish(Incremental) saw stale=[] and skipped storing sites because
        // sites were only taken from the (empty) `built` list. Verify that
        // effect_sites is non-empty after an Always refresh of a corpus that
        // contains cross-spec links.
        let (dir, _stub) = indexed_corpus_with_stub(&MULTI).await;
        let conn = open(&dir);
        let site_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM effect_sites", [], |r| r.get(0))
            .unwrap();
        assert!(
            site_count > 0,
            "EffectsRefresh::Always must store effect_sites, got 0"
        );
    }

    #[tokio::test]
    async fn warm_refresh_leaves_db_unchanged() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        let conn = open(&dir);
        let changes_before = conn.total_changes();
        let lock_rows_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM refresh_lock", [], |r| r.get(0))
            .unwrap();
        let options = RefreshOptions {
            freshness: FreshnessOptions {
                origin: Some(stub.origin()),
                ..FreshnessOptions::default()
            },
            ..RefreshOptions::new(
                LockPolicy::Wait(Duration::from_secs(5)),
                EffectsRefresh::Inline {
                    budget: Duration::from_secs(60),
                },
            )
        };
        let report = refresh_for_query(&conn, "DOM", &options).await.unwrap();
        assert_eq!(
            conn.total_changes(),
            changes_before,
            "warm refresh must not write the DB"
        );
        let lock_rows_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM refresh_lock", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            lock_rows_before, lock_rows_after,
            "warm refresh must not touch refresh_lock"
        );
        assert!(report.parsed.is_empty());
        assert_eq!(report.effects, EffectsOutcome::Current);
    }

    #[tokio::test]
    async fn dep_parse_failure_serves_cached_target() {
        let (dir, stub) = indexed_corpus_with_stub(&MULTI).await;
        stub.put(
            "infra.spec.whatwg.org/",
            &MULTI_INFRA_EDITED,
            Some("W/\"new\""),
            None,
        );
        let conn = open(&dir);
        let targets = load_targets(&conn, &["INFRA".to_string()]).unwrap();
        let snapshot = crate::db::queries::get_snapshot(&conn, "INFRA").unwrap();
        let changed = vec![(
            &targets[0],
            snapshot,
            crate::fetch::ChangedHtml {
                html: BETA.to_string(),
                content_hash: "new-hash".to_string(),
                validators: Default::default(),
                test_fail: true,
            },
        )];
        let mut report = RefreshReport::default();
        parse_and_write(&conn, changed, None, &mut report)
            .await
            .unwrap();
        assert!(
            report.failed.contains(&"INFRA".to_string()),
            "failed dep must appear in report.failed"
        );
        assert!(
            report.parsed.is_empty(),
            "no spec must be recorded as parsed"
        );
        assert!(
            crate::db::queries::get_snapshot(&conn, "DOM")
                .unwrap()
                .is_some(),
            "DOM snapshot must still be present"
        );
    }
}
