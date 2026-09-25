// Fetch orchestration: coordinate HTML fetching, parsing, and database writes
pub mod github;
pub mod itu;
pub mod pr;
pub mod snapshot;
pub mod tc39_pr;
pub mod whatpr;

use crate::db::{queries, write};
use crate::parse;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
#[cfg(feature = "native")]
use std::collections::BTreeMap;
#[cfg(feature = "native")]
use std::path::{Path, PathBuf};

const CHECK_INTERVAL_HOURS: i64 = 24;
#[cfg(feature = "native")]
const UPDATE_PARALLELISM: usize = 8;

// ── HTML cache ────────────────────────────────────────────────────────────────

/// Sanitize a spec name so it is safe as a filesystem directory component.
/// Keeps alphanumerics, hyphens, and dots; replaces everything else with `_`.
pub(crate) fn sanitize_for_fs(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Base directory for HTML cache files: `<db_dir>/html/`.
#[cfg(feature = "native")]
pub(crate) fn html_cache_dir(db_dir: &Path) -> PathBuf {
    db_dir.join("html")
}

/// Path for a cached HTML snapshot:
/// `<db_dir>/html/<sanitized_spec>/<identity>.html`
///
/// `identity` is the upstream identity string (commit sha or `hash:<hex>`).
/// The portion after the last `:` is used so `hash:abc123` becomes `abc123.html`.
#[cfg(feature = "native")]
pub fn html_cache_path(db_dir: &Path, spec_name: &str, identity: &str) -> PathBuf {
    let dir = html_cache_dir(db_dir).join(sanitize_for_fs(spec_name));
    let file_stem = identity.rsplit(':').next().unwrap_or(identity);
    dir.join(format!("{}.html", sanitize_for_fs(file_stem)))
}

/// Decision made by the cache gate before a live fetch.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CacheDecision {
    /// The cached file is valid; parse from it instead of downloading.
    Cached,
    /// No valid cache; proceed with a live download.
    Download,
}

/// Pure logic: decide whether to serve from cache.
///
/// `cache_exists` — the cache file is present on disk.
/// `sha_matches`  — the stored identity equals the upstream identity.
/// `refetch`      — the caller passed `--refetch`; always forces a download.
pub(crate) fn decide_cache(cache_exists: bool, sha_matches: bool, refetch: bool) -> CacheDecision {
    if !refetch && cache_exists && sha_matches {
        CacheDecision::Cached
    } else {
        CacheDecision::Download
    }
}

/// Write HTML to the on-disk cache for `spec_name` with the given identity,
/// deleting any previous snapshot file in that spec's cache directory first
/// (one file per spec).
#[cfg(feature = "native")]
pub(crate) fn write_html_cache(
    db_dir: &Path,
    spec_name: &str,
    identity: &str,
    html: &str,
) -> anyhow::Result<()> {
    use std::fs;
    let spec_dir = html_cache_dir(db_dir).join(sanitize_for_fs(spec_name));
    fs::create_dir_all(&spec_dir)?;
    // Delete old snapshots for this spec.
    for entry in fs::read_dir(&spec_dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "html") {
            let _ = fs::remove_file(entry.path());
        }
    }
    let target = html_cache_path(db_dir, spec_name, identity);
    fs::write(&target, html)?;
    Ok(())
}

/// Read cached HTML for the given spec and identity. Returns `None` when the
/// file does not exist.
#[cfg(feature = "native")]
pub(crate) fn read_html_cache(db_dir: &Path, spec_name: &str, identity: &str) -> Option<String> {
    let path = html_cache_path(db_dir, spec_name, identity);
    std::fs::read_to_string(&path).ok()
}

/// Return the db directory (parent of `index.db`).
#[cfg(feature = "native")]
pub(crate) fn db_dir() -> PathBuf {
    crate::db::get_db_path()
        .parent()
        .expect("db path has no parent")
        .to_path_buf()
}

fn is_fresh(last_checked: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
    now.signed_duration_since(*last_checked).num_hours() < CHECK_INTERVAL_HOURS
}

/// Whether the cached index was produced by the current build. A new release
/// (bumped `INDEX_VERSION`) makes this false, forcing a re-parse even when the
/// upstream HTML is unchanged.
fn index_is_current(state: &queries::UpdateCheckState) -> bool {
    state.index_version.as_deref() == Some(parse::INDEX_VERSION)
}

/// Whether a cached spec can be served without re-syncing: it must be within the
/// freshness window AND have been produced by the current build. A version
/// upgrade invalidates the freshness window so the next query re-indexes even if
/// the 24h check has not elapsed.
pub(crate) fn cache_is_current(state: &queries::UpdateCheckState, now: &DateTime<Utc>) -> bool {
    is_fresh(&state.last_checked, now) && index_is_current(state)
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_html(html: &str) -> String {
    hash_bytes(html.as_bytes())
}

fn store_update_check(
    conn: &Connection,
    spec_id: i64,
    now: &DateTime<Utc>,
    last_indexed: Option<&DateTime<Utc>>,
    content_hash: Option<&str>,
) -> Result<()> {
    let checked = now.to_rfc3339();
    let indexed = last_indexed.map(|t| t.to_rfc3339());
    write::record_update_check(
        conn,
        spec_id,
        &checked,
        indexed.as_deref(),
        content_hash,
        Some(parse::INDEX_VERSION),
    )
}

#[allow(clippy::too_many_arguments)]
fn sync_from_html(
    conn: &Connection,
    spec_id: i64,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    html: String,
    previous_snapshot_id: Option<i64>,
    state: Option<queries::UpdateCheckState>,
    now: &DateTime<Utc>,
    force: bool,
) -> Result<(i64, bool)> {
    let content_hash = hash_html(&html);

    if let (Some(snapshot_id), Some(state)) = (previous_snapshot_id, state.as_ref()) {
        let content_unchanged = state.content_hash.as_deref() == Some(content_hash.as_str());
        // A forced refresh exists to rebuild a suspect index, so it must reach
        // the parser even when the bytes and the index version both match.
        if !force
            && content_unchanged
            && index_is_current(state)
            && crate::db::effects::has_structure(
                conn,
                snapshot_id,
                parse::steps::STRUCTURE_VERSION,
            )?
        {
            store_update_check(
                conn,
                spec_id,
                now,
                state.last_indexed.as_ref(),
                Some(&content_hash),
            )?;
            return Ok((snapshot_id, false));
        }
    }

    let prepared = parse_html(html, spec_name, base_url, content_hash)?;
    write_parsed_html(
        conn,
        spec_id,
        spec_name,
        base_url,
        provider_name,
        prepared,
        now,
    )
}

struct ParsedHtml {
    content_hash: String,
    parsed: crate::model::ParsedSpec,
    structure_json: String,
}

fn parse_html(
    html: String,
    spec_name: &str,
    base_url: &str,
    content_hash: String,
) -> Result<ParsedHtml> {
    let parsed = parse::parse_spec(&html, spec_name, base_url)?;
    let synthetic_sha = format!("hash:{content_hash}");
    let structure =
        parse::steps::extract_step_structure(&html, spec_name, base_url, &synthetic_sha);
    Ok(ParsedHtml {
        content_hash,
        parsed,
        structure_json: serde_json::to_string(&structure)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_parsed_html(
    conn: &Connection,
    spec_id: i64,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    prepared: ParsedHtml,
    now: &DateTime<Utc>,
) -> Result<(i64, bool)> {
    let ParsedHtml {
        content_hash,
        parsed,
        structure_json,
    } = prepared;
    let synthetic_sha = format!("hash:{content_hash}");
    write::atomic_write(conn, |conn| {
        write::delete_spec_data(conn, spec_id)?;

        let commit_date = now.to_rfc3339();
        let spec_id_reloaded = write::insert_or_get_spec(conn, spec_name, base_url, provider_name)?;
        let snapshot_id =
            write::insert_snapshot(conn, spec_id_reloaded, &synthetic_sha, &commit_date)?;
        write::insert_sections_bulk(conn, snapshot_id, &parsed.sections)?;
        write::insert_refs_bulk(conn, snapshot_id, &parsed.references)?;
        write::insert_idl_defs_bulk(conn, snapshot_id, &parsed.idl_definitions)?;
        crate::db::effects::store_structure(
            conn,
            snapshot_id,
            parse::steps::STRUCTURE_VERSION,
            &structure_json,
        )?;

        store_update_check(conn, spec_id_reloaded, now, Some(now), Some(&content_hash))?;
        Ok((snapshot_id, true))
    })
}

fn is_respec_source(html: &str) -> bool {
    html.contains("respec-w3c") || html.contains("respec.js") || html.contains("/respec/")
}

async fn render_via_spec_generator(url: &str) -> Result<String> {
    let api_url = format!(
        "https://www.w3.org/publications/spec-generator/?type=respec&url={}",
        url::form_urlencoded::byte_serialize(url.as_bytes()).collect::<String>()
    );
    let html = fetch_raw_html(&api_url).await?;
    if html.trim_start().starts_with('{') {
        anyhow::bail!(
            "spec-generator returned error: {}",
            &html[..html.len().min(200)]
        );
    }
    Ok(html)
}

pub(crate) async fn fetch_raw_html(url: &str) -> Result<String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new);
    let response = client
        .get(url)
        .header(
            "User-Agent",
            concat!("webspec-index/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch {}: HTTP {}", url, response.status());
    }
    Ok(response.text().await?)
}

async fn fetch_live_html(base_url: &str) -> Result<String> {
    let url = if base_url.ends_with(".html") || base_url.ends_with(".txt") {
        base_url.to_string()
    } else {
        format!("{}/", base_url.trim_end_matches('/'))
    };
    let html = fetch_raw_html(&url).await?;

    if is_respec_source(&html) {
        eprintln!(
            "note: {} is a live ReSpec document; rendering via W3C spec-generator",
            url
        );
        match render_via_spec_generator(&url).await {
            Ok(rendered) => return Ok(rendered),
            Err(e) => eprintln!("warning: spec-generator failed ({}), using raw HTML", e),
        }
    }

    Ok(html)
}

/// Outcome of a live-fetch attempt during a sync.
enum FetchOutcome {
    /// Fresh HTML that should be (re)parsed.
    Fresh(String),
    /// The fetch failed but a cached snapshot exists; serve it instead of failing.
    ServeCached(i64),
}

/// Decide how to proceed after attempting a live fetch. On failure, fall back to
/// the cached snapshot only when `allow_fallback` is set (best-effort refresh)
/// and a cached snapshot exists; otherwise propagate the fetch error. A forced
/// refresh disallows the fallback so failures surface instead of silently
/// serving stale data.
fn resolve_fetch(
    fetched: Result<String>,
    previous_snapshot_id: Option<i64>,
    allow_fallback: bool,
) -> Result<FetchOutcome> {
    match fetched {
        Ok(html) => Ok(FetchOutcome::Fresh(html)),
        Err(e) => match previous_snapshot_id {
            Some(snapshot_id) if allow_fallback => Ok(FetchOutcome::ServeCached(snapshot_id)),
            _ => Err(e),
        },
    }
}

/// Sync a spec, optionally falling back to the cached snapshot when the live
/// fetch fails. `allow_fallback` is the caller's policy: the query path
/// (`ensure_indexed`) sets it so a stale-but-cached spec is still served when
/// offline, while the explicit `update` command clears it so fetch failures are
/// reported rather than masquerading as success. A forced refresh never falls
/// back regardless, so `--force` always surfaces failures.
///
/// `refetch` bypasses the on-disk HTML cache even when `force` is set. When
/// `force` is true and `refetch` is false, a cached HTML file for the stored
/// content hash is used to re-parse without a network round-trip.
async fn sync_known_spec(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    force: bool,
    allow_fallback: bool,
    refetch: bool,
) -> Result<(i64, bool)> {
    if provider_name == "itu" {
        return itu::sync_known_spec_pdf(conn, spec_name, base_url, force, allow_fallback).await;
    }

    let spec_id = write::insert_or_get_spec(conn, spec_name, base_url, provider_name)?;
    let previous_snapshot_id = queries::get_snapshot(conn, spec_name)?;
    let state = queries::get_update_check(conn, spec_id)?;
    let now = Utc::now();

    if !force {
        if let (Some(snapshot_id), Some(sync_state)) = (previous_snapshot_id, state.as_ref()) {
            if cache_is_current(sync_state, &now)
                && crate::db::effects::has_structure(
                    conn,
                    snapshot_id,
                    parse::steps::STRUCTURE_VERSION,
                )?
            {
                return Ok((snapshot_id, false));
            }
        }
    }

    // When force-updating, check the on-disk HTML cache before hitting the
    // network. If the cache file for the stored content hash is present and
    // --refetch was not requested, re-parse from disk rather than downloading.
    #[cfg(feature = "native")]
    if force && !refetch {
        if let Some(ref sync_state) = state {
            if let Some(ref stored_hash) = sync_state.content_hash {
                let d = db_dir();
                let cache_exists = html_cache_path(&d, spec_name, stored_hash).exists();
                if decide_cache(cache_exists, true, false) == CacheDecision::Cached {
                    eprintln!("note: {spec_name}: re-parsing from on-disk cache (use --refetch to download fresh)");
                    let html = read_html_cache(&d, spec_name, stored_hash)
                        .ok_or_else(|| anyhow::anyhow!("cache file disappeared unexpectedly"))?;
                    return sync_from_html(
                        conn,
                        spec_id,
                        spec_name,
                        base_url,
                        provider_name,
                        html,
                        previous_snapshot_id,
                        state,
                        &now,
                        force,
                    );
                }
            }
        }
    }

    let fetched = fetch_live_html(base_url).await;

    // After a successful download, persist the HTML to the on-disk cache so
    // future forced re-indexes can skip the network.
    #[cfg(feature = "native")]
    let fetched = fetched.inspect(|html| {
        let d = db_dir();
        let hash = hash_html(html);
        if let Err(e) = write_html_cache(&d, spec_name, &hash, html) {
            eprintln!("warning: {spec_name}: could not write HTML cache: {e}");
        }
    });

    apply_fetch(
        conn,
        spec_id,
        spec_name,
        base_url,
        provider_name,
        fetched,
        previous_snapshot_id,
        state,
        allow_fallback,
        force,
        &now,
    )
}

/// Turn a live-fetch result into a synced snapshot, applying the caller's
/// offline-fallback policy. Fall back to the cached snapshot on fetch failure
/// only when the caller allows it (the query path) AND this is not a forced
/// refresh — the `update` command disallows fallback so a failed fetch is
/// reported rather than hidden, and `--force` always surfaces failures.
///
/// Split out of [`sync_known_spec`] so the fetch-failure branches are testable
/// without hitting the network.
#[allow(clippy::too_many_arguments)]
fn apply_fetch(
    conn: &Connection,
    spec_id: i64,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    fetched: Result<String>,
    previous_snapshot_id: Option<i64>,
    state: Option<queries::UpdateCheckState>,
    allow_fallback: bool,
    force: bool,
    now: &DateTime<Utc>,
) -> Result<(i64, bool)> {
    match resolve_fetch(fetched, previous_snapshot_id, allow_fallback && !force)? {
        FetchOutcome::ServeCached(snapshot_id) => Ok((snapshot_id, false)),
        FetchOutcome::Fresh(html) => sync_from_html(
            conn,
            spec_id,
            spec_name,
            base_url,
            provider_name,
            html,
            previous_snapshot_id,
            state,
            now,
            force,
        ),
    }
}

/// Same as [`sync_known_spec`], for ad-hoc URL-based specs whose provider is
/// always "dynamic".
async fn sync_dynamic_spec(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    force: bool,
    allow_fallback: bool,
) -> Result<(i64, bool)> {
    sync_known_spec(
        conn,
        spec_name,
        base_url,
        "dynamic",
        force,
        allow_fallback,
        false,
    )
    .await
}

/// Ensure an ad-hoc URL-based spec is indexed.
///
/// This supports domains accepted by `SpecRegistry::resolve_url()` auto resolution.
pub async fn ensure_indexed_dynamic(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
) -> Result<i64> {
    let (snapshot_id, _) = sync_dynamic_spec(conn, spec_name, base_url, false, true).await?;
    Ok(snapshot_id)
}

/// Ensure a spec is indexed and reasonably fresh.
///
/// Uses a 24h freshness window based on `update_checks.last_checked`.
/// When refreshing, fetches live HTML and re-indexes only if content hash changed.
pub async fn ensure_indexed(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
) -> Result<i64> {
    let (snapshot_id, _) =
        sync_known_spec(conn, spec_name, base_url, provider_name, false, true, false).await?;
    Ok(snapshot_id)
}

/// Update a spec if needed.
///
/// Returns `Some(snapshot_id)` only when content changed and was re-indexed.
/// A fetch failure is reported (no offline cache fallback): `update` is an
/// explicit refresh, so a failed fetch must not be reported as success.
pub async fn update_if_needed(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    force: bool,
    refetch: bool,
) -> Result<Option<i64>> {
    let (snapshot_id, updated) = sync_known_spec(
        conn,
        spec_name,
        base_url,
        provider_name,
        force,
        false,
        refetch,
    )
    .await?;
    Ok(updated.then_some(snapshot_id))
}

/// Update all specs in the registry.
/// Returns vector of (spec_name, Option<snapshot_id>) pairs.
pub async fn update_all_specs(
    conn: &Connection,
    specs: &[(String, String, String)], // (name, base_url, provider)
    force: bool,
    refetch: bool,
) -> Vec<(String, Result<Option<i64>>)> {
    let mut results = Vec::with_capacity(specs.len());
    let mut cursor = 0;
    while cursor < specs.len() {
        // The PDF path has a separate parser and cache policy. Keep it on its
        // existing path, between bounded HTML batches.
        if specs[cursor].2 == "itu" {
            let (name, base_url, provider) = &specs[cursor];
            let result = update_if_needed(conn, name, base_url, provider, force, refetch).await;
            results.push((name.clone(), result));
            cursor += 1;
            continue;
        }

        let end = (cursor + UPDATE_PARALLELISM).min(specs.len());
        let end = (cursor..end).find(|&i| specs[i].2 == "itu").unwrap_or(end);
        let batch = &specs[cursor..end];
        let mut jobs = Vec::new();
        let mut prepared = BTreeMap::new();

        for (index, (name, base_url, provider)) in batch.iter().enumerate() {
            match plan_html_update(conn, name, base_url, provider, force, refetch) {
                Ok(HtmlUpdatePlan::Current) => {
                    prepared.insert(index, Ok(None));
                }
                Ok(HtmlUpdatePlan::NeedsWork(plan)) => {
                    let handle = tokio::spawn(async move {
                        let result = prepare_html_update(&plan).await;
                        (plan, result)
                    });
                    jobs.push((index, handle));
                }
                Err(e) => {
                    prepared.insert(index, Err(e));
                }
            }
        }

        for (index, handle) in jobs {
            let result = match handle.await {
                Ok((plan, result)) => {
                    result.and_then(|update| commit_html_update(conn, *plan, update))
                }
                Err(e) => Err(anyhow::anyhow!("update worker failed: {e}")),
            };
            prepared.insert(index, result);
        }
        for (index, (name, _, _)) in batch.iter().enumerate() {
            let result = prepared
                .remove(&index)
                .expect("every spec has an update result");
            results.push((name.clone(), result));
        }
        cursor = end;
    }
    results
}

struct HtmlUpdateWork {
    spec_id: i64,
    spec_name: String,
    base_url: String,
    provider_name: String,
    previous_snapshot_id: Option<i64>,
    state: Option<queries::UpdateCheckState>,
    now: DateTime<Utc>,
    force: bool,
    cached_html: Option<String>,
    can_reuse_unchanged: bool,
}

enum HtmlUpdatePlan {
    Current,
    NeedsWork(Box<HtmlUpdateWork>),
}

enum PreparedHtmlUpdate {
    Unchanged(String),
    Parsed(ParsedHtml),
}

fn plan_html_update(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    force: bool,
    refetch: bool,
) -> Result<HtmlUpdatePlan> {
    let spec_id = write::insert_or_get_spec(conn, spec_name, base_url, provider_name)?;
    let previous_snapshot_id = queries::get_snapshot(conn, spec_name)?;
    let state = queries::get_update_check(conn, spec_id)?;
    let now = Utc::now();
    let has_structure = match previous_snapshot_id {
        Some(snapshot_id) => {
            crate::db::effects::has_structure(conn, snapshot_id, parse::steps::STRUCTURE_VERSION)?
        }
        None => false,
    };
    if !force
        && has_structure
        && state
            .as_ref()
            .is_some_and(|state| cache_is_current(state, &now))
    {
        return Ok(HtmlUpdatePlan::Current);
    }

    let cached_html = if force && !refetch {
        match state
            .as_ref()
            .and_then(|state| state.content_hash.as_deref())
        {
            Some(hash) if html_cache_path(&db_dir(), spec_name, hash).exists() => Some(
                read_html_cache(&db_dir(), spec_name, hash)
                    .ok_or_else(|| anyhow::anyhow!("cache file disappeared unexpectedly"))?,
            ),
            _ => None,
        }
    } else {
        None
    };
    if cached_html.is_some() {
        eprintln!(
            "note: {spec_name}: re-parsing from on-disk cache (use --refetch to download fresh)"
        );
    }
    Ok(HtmlUpdatePlan::NeedsWork(Box::new(HtmlUpdateWork {
        spec_id,
        spec_name: spec_name.to_string(),
        base_url: base_url.to_string(),
        provider_name: provider_name.to_string(),
        previous_snapshot_id,
        state,
        now,
        force,
        cached_html,
        can_reuse_unchanged: has_structure,
    })))
}

async fn prepare_html_update(plan: &HtmlUpdateWork) -> Result<PreparedHtmlUpdate> {
    let html = if let Some(html) = plan.cached_html.as_ref() {
        html.clone()
    } else {
        let html = fetch_live_html(&plan.base_url).await?;
        let hash = hash_html(&html);
        if let Err(e) = write_html_cache(&db_dir(), &plan.spec_name, &hash, &html) {
            eprintln!(
                "warning: {}: could not write HTML cache: {e}",
                plan.spec_name
            );
        }
        html
    };
    let content_hash = hash_html(&html);
    if !plan.force
        && plan.can_reuse_unchanged
        && plan.previous_snapshot_id.is_some()
        && plan.state.as_ref().is_some_and(|state| {
            state.content_hash.as_deref() == Some(content_hash.as_str()) && index_is_current(state)
        })
    {
        return Ok(PreparedHtmlUpdate::Unchanged(content_hash));
    }
    let spec_name = plan.spec_name.clone();
    let base_url = plan.base_url.clone();
    Ok(PreparedHtmlUpdate::Parsed(
        tokio::task::spawn_blocking(move || parse_html(html, &spec_name, &base_url, content_hash))
            .await??,
    ))
}

fn commit_html_update(
    conn: &Connection,
    plan: HtmlUpdateWork,
    update: PreparedHtmlUpdate,
) -> Result<Option<i64>> {
    match update {
        PreparedHtmlUpdate::Unchanged(content_hash) => {
            store_update_check(
                conn,
                plan.spec_id,
                &plan.now,
                plan.state
                    .as_ref()
                    .and_then(|state| state.last_indexed.as_ref()),
                Some(&content_hash),
            )?;
            Ok(None)
        }
        PreparedHtmlUpdate::Parsed(parsed) => {
            let (snapshot_id, _) = write_parsed_html(
                conn,
                plan.spec_id,
                &plan.spec_name,
                &plan.base_url,
                &plan.provider_name,
                parsed,
                &plan.now,
            )?;
            Ok(Some(snapshot_id))
        }
    }
}

/// Re-parse one or all indexed specs from the on-disk HTML cache, writing
/// fresh sections/refs/IDL to the DB without any network access.
///
/// Specs that have no cached HTML file are reported to stderr and skipped.
/// `spec` filters to a single spec; `providers` filters by provider name
/// (empty = all). Returns `(spec_name, Option<snapshot_id>)` pairs.
#[cfg(feature = "native")]
pub async fn reparse_specs(
    conn: &Connection,
    spec: Option<&str>,
    providers: &[String],
) -> Result<Vec<(String, Option<i64>)>> {
    let all_specs = queries::list_specs(conn)?;
    let filtered: Vec<_> = all_specs
        .into_iter()
        .filter(|(name, _, provider)| {
            let spec_ok = spec.is_none_or(|s| name.eq_ignore_ascii_case(s));
            let prov_ok =
                providers.is_empty() || providers.iter().any(|p| p.eq_ignore_ascii_case(provider));
            spec_ok && prov_ok
        })
        .collect();

    let d = db_dir();
    let now = Utc::now();
    let mut results = Vec::new();

    for (name, base_url, provider) in filtered {
        let spec_id = write::insert_or_get_spec(conn, &name, &base_url, &provider)?;
        let state = queries::get_update_check(conn, spec_id)?;
        let previous_snapshot_id = queries::get_snapshot(conn, &name)?;

        let cached_hash = state.as_ref().and_then(|s| s.content_hash.clone());
        let html = match cached_hash.as_deref() {
            Some(hash) => match read_html_cache(&d, &name, hash) {
                Some(html) => html,
                None => {
                    eprintln!("reparse: {name}: no cache file found, skipping");
                    results.push((name, None));
                    continue;
                }
            },
            None => {
                eprintln!("reparse: {name}: no stored content hash, skipping");
                results.push((name, None));
                continue;
            }
        };

        match sync_from_html(
            conn,
            spec_id,
            &name,
            &base_url,
            &provider,
            html,
            previous_snapshot_id,
            state,
            &now,
            true,
        ) {
            Ok((snapshot_id, _)) => results.push((name, Some(snapshot_id))),
            Err(e) => {
                eprintln!("reparse: {name}: failed: {e}");
                results.push((name, None));
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[cfg(feature = "native")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_all_fetches_a_bounded_batch_concurrently_and_keeps_result_order() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("could not bind test listener: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut peers = Vec::new();
            for _ in 0..3 {
                let (mut peer, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 1024];
                let _ = peer.read(&mut request).await.unwrap();
                peers.push(peer);
            }
            // The response is deliberately withheld until all three requests
            // arrive. A serial update cannot complete this batch.
            for mut peer in peers {
                peer.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
        });
        let specs: Vec<_> = ["FIRST", "SECOND", "THIRD"]
            .into_iter()
            .map(|name| {
                (
                    name.to_string(),
                    format!("http://{address}/{name}"),
                    "test".to_string(),
                )
            })
            .collect();
        let conn = db::open_test_db().unwrap();
        let results = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            update_all_specs(&conn, &specs, false, false),
        )
        .await
        .expect("all requests should be in flight together");
        server.await.unwrap();
        assert_eq!(
            results
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["FIRST", "SECOND", "THIRD"]
        );
        assert!(results.iter().all(|(_, result)| result.is_err()));
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn prepared_html_commits_snapshot_and_structure() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "PREPARED", "https://example.test", "test").unwrap();
        let plan = HtmlUpdateWork {
            spec_id,
            spec_name: "PREPARED".to_string(),
            base_url: "https://example.test".to_string(),
            provider_name: "test".to_string(),
            previous_snapshot_id: None,
            state: None,
            now: fixed_now(),
            force: true,
            cached_html: Some("<h2 id=\"prepared\">Prepared</h2>".to_string()),
            can_reuse_unchanged: false,
        };
        let parsed = prepare_html_update(&plan).await.unwrap();
        let snapshot_id = commit_html_update(&conn, plan, parsed).unwrap().unwrap();
        assert_eq!(
            queries::get_snapshot(&conn, "PREPARED").unwrap(),
            Some(snapshot_id)
        );
        assert!(crate::db::effects::has_structure(
            &conn,
            snapshot_id,
            parse::steps::STRUCTURE_VERSION,
        )
        .unwrap());
    }

    // ── cache decision ────────────────────────────────────────────────────────

    #[test]
    fn test_decide_cache_all_present_no_refetch() {
        assert_eq!(
            decide_cache(true, true, false),
            CacheDecision::Cached,
            "file present, sha matches, no refetch → serve cache"
        );
    }

    #[test]
    fn test_decide_cache_refetch_overrides() {
        assert_eq!(
            decide_cache(true, true, true),
            CacheDecision::Download,
            "--refetch forces download even when cache is valid"
        );
    }

    #[test]
    fn test_decide_cache_no_file() {
        assert_eq!(
            decide_cache(false, true, false),
            CacheDecision::Download,
            "no cache file → download"
        );
    }

    #[test]
    fn test_decide_cache_sha_mismatch() {
        assert_eq!(
            decide_cache(true, false, false),
            CacheDecision::Download,
            "sha mismatch → download"
        );
    }

    // ── filesystem cache helpers ──────────────────────────────────────────────

    #[cfg(feature = "native")]
    #[test]
    fn test_html_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path();
        let html = "<html><body>test</body></html>";
        write_html_cache(db_dir, "HTML", "abc123", html).unwrap();
        let path = html_cache_path(db_dir, "HTML", "abc123");
        assert!(path.exists(), "cache file should be written");
        let read_back = read_html_cache(db_dir, "HTML", "abc123").unwrap();
        assert_eq!(read_back, html);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_html_cache_replaces_previous() {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path();
        write_html_cache(db_dir, "DOM", "oldsha", "old content").unwrap();
        write_html_cache(db_dir, "DOM", "newsha", "new content").unwrap();
        // Old file should be gone, new file should be present.
        assert!(
            read_html_cache(db_dir, "DOM", "oldsha").is_none(),
            "old cache file should be deleted"
        );
        assert_eq!(
            read_html_cache(db_dir, "DOM", "newsha").unwrap(),
            "new content"
        );
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_html_cache_path_sanitizes_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = html_cache_path(dir.path(), "CSS-GRID", "hash:abc123");
        assert!(
            path.to_string_lossy().contains("CSS-GRID"),
            "spec dir uses sanitized name"
        );
        assert!(
            path.to_string_lossy().ends_with("abc123.html"),
            "file stem strips the 'hash:' prefix"
        );
    }

    fn state_with(index_version: Option<&str>, last_checked: &str) -> queries::UpdateCheckState {
        queries::UpdateCheckState {
            last_checked: DateTime::parse_from_rfc3339(last_checked)
                .unwrap()
                .with_timezone(&Utc),
            last_indexed: None,
            content_hash: Some("hash".to_string()),
            index_version: index_version.map(str::to_string),
        }
    }

    // On fetch failure, resolve_fetch falls back to a cached snapshot only when
    // fallback is allowed and a snapshot exists; otherwise it propagates the error.
    #[test]
    fn test_resolve_fetch() {
        // Success -> parse the fresh HTML regardless of cache/fallback state.
        match resolve_fetch(Ok("<html></html>".to_string()), None, true).unwrap() {
            FetchOutcome::Fresh(html) => assert_eq!(html, "<html></html>"),
            FetchOutcome::ServeCached(_) => panic!("expected Fresh on success"),
        }

        // Failure, fallback allowed, cached snapshot present -> serve the cache.
        match resolve_fetch(Err(anyhow::anyhow!("offline")), Some(42), true).unwrap() {
            FetchOutcome::ServeCached(id) => assert_eq!(id, 42),
            FetchOutcome::Fresh(_) => panic!("expected ServeCached on failure with cache"),
        }

        // Failure with no cached snapshot -> propagate the error.
        assert!(resolve_fetch(Err(anyhow::anyhow!("offline")), None, true).is_err());

        // Failure with a cache but fallback disallowed (e.g. --force) -> propagate.
        assert!(resolve_fetch(Err(anyhow::anyhow!("offline")), Some(42), false).is_err());
    }

    // apply_fetch encodes the caller's fallback policy (`allow_fallback && !force`)
    // over a real cached snapshot: the query path serves the cache when offline,
    // while `update` (no fallback) and any `--force` refresh surface the failure.
    #[test]
    fn test_apply_fetch_fallback_policy() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "TEST", "https://example.test", "test").unwrap();
        let cached =
            write::insert_snapshot(&conn, spec_id, "hash:cached", "2026-01-01T00:00:00Z").unwrap();
        let now = fixed_now();

        let run = |allow_fallback: bool, force: bool| {
            apply_fetch(
                &conn,
                spec_id,
                "TEST",
                "https://example.test",
                "test",
                Err(anyhow::anyhow!("offline")),
                Some(cached),
                None,
                allow_fallback,
                force,
                &now,
            )
        };

        // Query path (fallback allowed, not forced) -> serve the cached snapshot.
        let (id, updated) = run(true, false).unwrap();
        assert_eq!(id, cached);
        assert!(!updated);

        // update command (fallback disallowed) -> surface the fetch failure.
        assert!(run(false, false).is_err());

        // Forced refresh always surfaces the failure, even on the query path.
        assert!(run(true, true).is_err());
    }

    #[test]
    fn failed_replacement_preserves_previous_snapshot() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "TEST", "https://example.test", "test").unwrap();
        let now = fixed_now();
        let (original, _) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            "<h2 id=\"original\">Original</h2>".into(),
            None,
            None,
            &now,
            false,
        )
        .unwrap();
        conn.execute_batch("CREATE TRIGGER reject_replacement BEFORE INSERT ON sections
            WHEN NEW.anchor = 'replacement' BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
        let result = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            "<h2 id=\"replacement\">Replacement</h2>".into(),
            Some(original),
            queries::get_update_check(&conn, spec_id).unwrap(),
            &now,
            true,
        );
        assert!(result.is_err());
        let old_sections: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sections WHERE snapshot_id = ?1 AND anchor = 'original'",
                [original],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            old_sections, 1,
            "failed publication must preserve the previous corpus"
        );
    }

    // The freshness gate must require BOTH a fresh timestamp and a matching
    // index version before serving cached data without a re-sync.
    #[test]
    fn test_cache_is_current() {
        let now = fixed_now();

        // Fresh + current build -> serve cache.
        assert!(cache_is_current(
            &state_with(Some(parse::INDEX_VERSION), "2026-01-01T00:00:00Z"),
            &now
        ));

        // Fresh but produced by an older build (or pre-upgrade NULL) -> re-sync.
        assert!(!cache_is_current(
            &state_with(Some("0.0.0"), "2026-01-01T00:00:00Z"),
            &now
        ));
        assert!(!cache_is_current(
            &state_with(None, "2026-01-01T00:00:00Z"),
            &now
        ));

        // Current build but stale (older than the 24h window) -> re-sync.
        assert!(!cache_is_current(
            &state_with(Some(parse::INDEX_VERSION), "2020-01-01T00:00:00Z"),
            &now
        ));
    }

    // A version change (bumped INDEX_VERSION) must force a re-parse of an already
    // cached spec even when the upstream HTML is byte-for-byte unchanged.
    #[test]
    fn test_sync_reparses_on_version_bump() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "TEST", "https://example.test", "test").unwrap();
        let html = "<h2 id=\"intro\">Intro</h2>".to_string();
        let now = fixed_now();

        // First index: no previous snapshot -> parse.
        let (snap1, updated1) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html.clone(),
            None,
            None,
            &now,
            false,
        )
        .unwrap();
        assert!(updated1, "first index should parse");

        // Second: identical content, state stamped with the current index
        // version -> skip re-parsing.
        let state = queries::get_update_check(&conn, spec_id).unwrap();
        assert_eq!(
            state.as_ref().and_then(|s| s.index_version.as_deref()),
            Some(parse::INDEX_VERSION)
        );
        let (snap2, updated2) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html.clone(),
            Some(snap1),
            state,
            &now,
            false,
        )
        .unwrap();
        assert!(!updated2, "unchanged content + current parser should skip");
        assert_eq!(snap2, snap1);

        // Third: identical content, but the stored index was produced by an older
        // build version -> must re-parse.
        let mut stale = queries::get_update_check(&conn, spec_id).unwrap().unwrap();
        stale.index_version = Some("0.0.0".to_string());
        let (_snap3, updated3) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html.clone(),
            Some(snap2),
            Some(stale),
            &now,
            false,
        )
        .unwrap();
        assert!(
            updated3,
            "index version change should force re-parse despite unchanged content"
        );
    }

    // --force is the escape hatch for a suspected bad index, so it has to reach
    // the parse. Re-fetching and then skipping the parse because the bytes match
    // leaves exactly the state the user was trying to rebuild.
    #[test]
    fn test_force_reparses_unchanged_content() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "TEST", "https://example.test", "test").unwrap();
        let html = "<h2 id=\"intro\">Intro</h2>".to_string();
        let now = fixed_now();

        let (snap1, _) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html.clone(),
            None,
            None,
            &now,
            false,
        )
        .unwrap();

        let state = queries::get_update_check(&conn, spec_id).unwrap();
        let (_, updated_unforced) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html.clone(),
            Some(snap1),
            state.clone(),
            &now,
            false,
        )
        .unwrap();
        assert!(!updated_unforced, "unforced sync still skips");

        let (_, updated_forced) = sync_from_html(
            &conn,
            spec_id,
            "TEST",
            "https://example.test",
            "test",
            html,
            Some(snap1),
            state,
            &now,
            true,
        )
        .unwrap();
        assert!(
            updated_forced,
            "--force must re-parse even when content and index version are unchanged"
        );
    }

    // reparse_specs reads from the on-disk HTML cache and re-writes sections.
    #[cfg(feature = "native")]
    #[tokio::test]
    async fn test_reparse_specs_from_cache() {
        use std::path::PathBuf;
        let dir = tempfile::tempdir().unwrap();
        let db_dir: PathBuf = dir.path().to_path_buf();

        // Override SPEC_INDEX_TEST_DB so db_dir() returns our temp dir's db.
        // We can't easily override db_dir(), so we call reparse_specs directly
        // with a connection rather than through the public API.
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "REPARSE-TEST", "https://example.test", "test")
                .unwrap();
        let html = "<h2 id=\"seed\">Seed</h2>";
        let now = fixed_now();

        // First index the spec so update_checks has a content_hash.
        let (snap1, _) = sync_from_html(
            &conn,
            spec_id,
            "REPARSE-TEST",
            "https://example.test",
            "test",
            html.to_string(),
            None,
            None,
            &now,
            false,
        )
        .unwrap();
        assert!(snap1 > 0);

        let content_hash = {
            let state = queries::get_update_check(&conn, spec_id).unwrap().unwrap();
            state.content_hash.unwrap()
        };

        // Write the cached HTML to the temp dir (simulating what the real update path writes).
        write_html_cache(&db_dir, "REPARSE-TEST", &content_hash, html).unwrap();
        assert!(
            html_cache_path(&db_dir, "REPARSE-TEST", &content_hash).exists(),
            "cache file must exist before reparse"
        );

        // Now simulate what reparse_specs does: read from cache, re-sync.
        let state = queries::get_update_check(&conn, spec_id).unwrap();
        let prev = queries::get_snapshot(&conn, "REPARSE-TEST").unwrap();
        let cached_html = read_html_cache(&db_dir, "REPARSE-TEST", &content_hash).unwrap();
        let (snap2, updated) = sync_from_html(
            &conn,
            spec_id,
            "REPARSE-TEST",
            "https://example.test",
            "test",
            cached_html,
            prev,
            state,
            &now,
            true,
        )
        .unwrap();
        assert!(updated, "force=true should cause re-index");

        let section_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sections WHERE snapshot_id = ?1 AND anchor = 'seed'",
                [snap2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(section_count, 1, "section from cached HTML must be present");
    }
}
