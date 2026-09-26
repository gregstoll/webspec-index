// Fetch orchestration: coordinate HTML fetching, parsing, and database writes
pub mod freshness;
pub mod github;
pub mod itu;
pub(crate) mod pipeline;
pub mod pr;
pub mod snapshot;
pub mod tc39_pr;
#[doc(hidden)]
pub mod testing;
pub mod whatpr;

use crate::db::snapshot_diff::{self, SnapshotRows};
use crate::db::{queries, write};
use crate::parse;
use crate::parse::markdown::{decode_memo, MarkdownMemo};
use anyhow::Result;
use chrono::{DateTime, Utc};
use freshness::{check_batch, effective_url, fetch_origin, Candidate, Freshness, FreshnessOptions};
use pipeline::{ParseJob, ParsedHtml};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CHECK_INTERVAL_HOURS: i64 = 24;
/// Specs planned, checked and parsed together by `update`; bounds the fetched
/// documents held in memory at once.
const UPDATE_WINDOW: usize = 64;
const UPDATE_MAX_IN_FLIGHT: usize = 16;
const USER_AGENT: &str = concat!("webspec-index/", env!("CARGO_PKG_VERSION"));

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

/// `<db file dir>/html` from `conn.path()`; `None` for in-memory connections,
/// which never touch a cache.
pub(crate) fn html_cache_dir_for(conn: &Connection) -> Option<PathBuf> {
    let path = conn.path().filter(|path| !path.is_empty())?;
    Some(Path::new(path).parent()?.join("html"))
}

/// Path for a cached HTML snapshot under the HTML cache directory
/// `cache_dir`: `<cache_dir>/<sanitized_spec>/<identity>.html`
///
/// `identity` is the upstream identity string (commit sha or `hash:<hex>`).
/// The portion after the last `:` is used so `hash:abc123` becomes `abc123.html`.
pub fn html_cache_path(cache_dir: &Path, spec_name: &str, identity: &str) -> PathBuf {
    let dir = cache_dir.join(sanitize_for_fs(spec_name));
    let file_stem = identity.rsplit(':').next().unwrap_or(identity);
    dir.join(format!("{}.html", sanitize_for_fs(file_stem)))
}

/// Write HTML to the on-disk cache for `spec_name` with the given identity,
/// deleting any previous snapshot file in that spec's cache directory first
/// (one file per spec).
pub(crate) fn write_html_cache(
    cache_dir: &Path,
    spec_name: &str,
    identity: &str,
    html: &str,
) -> anyhow::Result<()> {
    use std::fs;
    let spec_dir = cache_dir.join(sanitize_for_fs(spec_name));
    fs::create_dir_all(&spec_dir)?;
    // Delete old snapshots for this spec.
    for entry in fs::read_dir(&spec_dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "html") {
            let _ = fs::remove_file(entry.path());
        }
    }
    let target = html_cache_path(cache_dir, spec_name, identity);
    fs::write(&target, html)?;
    Ok(())
}

/// Read cached HTML for the given spec and identity. Returns `None` when the
/// file does not exist.
pub(crate) fn read_html_cache(cache_dir: &Path, spec_name: &str, identity: &str) -> Option<String> {
    let path = html_cache_path(cache_dir, spec_name, identity);
    std::fs::read_to_string(&path).ok()
}

/// The cached HTML of `spec_name` stored under `content_hash` in `conn`'s cache.
fn cached_html_for(conn: &Connection, spec_name: &str, content_hash: &str) -> Option<String> {
    read_html_cache(&html_cache_dir_for(conn)?, spec_name, content_hash)
}

fn cache_html_for(conn: &Connection, spec_name: &str, content_hash: &str, html: &str) {
    let Some(cache_dir) = html_cache_dir_for(conn) else {
        return;
    };
    if let Err(e) = write_html_cache(&cache_dir, spec_name, content_hash, html) {
        eprintln!("warning: {spec_name}: could not write HTML cache: {e}");
    }
}

fn is_fresh(last_checked: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
    now.signed_duration_since(*last_checked).num_hours() < CHECK_INTERVAL_HOURS
}

/// Whether the cached index was produced by the current build. A new release
/// (bumped `INDEX_VERSION`) makes this false, forcing a re-parse even when the
/// upstream HTML is unchanged.
pub(crate) fn index_is_current(state: &queries::UpdateCheckState) -> bool {
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
    crate::hex::encode(&Sha256::digest(bytes))
}

fn hash_html(html: &str) -> String {
    hash_bytes(html.as_bytes())
}

/// What the last response said about the source document: its HTTP validators
/// and the hash of its body.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SourceValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub source_hash: Option<String>,
}

impl SourceValidators {
    fn stored(state: Option<&queries::UpdateCheckState>) -> Self {
        state.map_or_else(Self::default, |state| Self {
            etag: state.etag.clone(),
            last_modified: state.last_modified.clone(),
            source_hash: state.source_hash.clone(),
        })
    }
}

fn store_update_check(
    conn: &Connection,
    spec_id: i64,
    now: &DateTime<Utc>,
    last_indexed: Option<&DateTime<Utc>>,
    content_hash: Option<&str>,
    validators: &SourceValidators,
) -> Result<()> {
    let checked = now.to_rfc3339();
    let indexed = last_indexed.map(|t| t.to_rfc3339());
    write::record_update_check(
        conn,
        spec_id,
        &write::UpdateCheckRecord {
            last_checked: &checked,
            last_indexed: indexed.as_deref(),
            content_hash,
            index_version: Some(parse::INDEX_VERSION),
            etag: validators.etag.as_deref(),
            last_modified: validators.last_modified.as_deref(),
            source_hash: validators.source_hash.as_deref(),
        },
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
    validators: &SourceValidators,
) -> Result<(i64, bool)> {
    let content_hash = hash_html(&html);

    if let (Some(snapshot_id), Some(state)) = (previous_snapshot_id, state.as_ref()) {
        let content_unchanged = state.content_hash.as_deref() == Some(content_hash.as_str());
        // A forced refresh exists to rebuild a suspect index, so it must reach
        // the parser even when the bytes and the index version both match.
        if !force
            && content_unchanged
            && index_is_current(state)
            && snapshot_is_reusable(conn, snapshot_id)?
        {
            store_update_check(
                conn,
                spec_id,
                now,
                state.last_indexed.as_ref(),
                Some(&content_hash),
                validators,
            )?;
            return Ok((snapshot_id, false));
        }
    }

    let prepared = pipeline::parse_one(ParseJob {
        spec_name: spec_name.to_string(),
        base_url: base_url.to_string(),
        html: Arc::new(html),
        content_hash,
        previous_memo: previous_memo(conn, previous_snapshot_id)?,
        fragment: None,
    })?;
    write_parsed_html(
        conn,
        spec_id,
        spec_name,
        base_url,
        provider_name,
        prepared,
        now,
        validators,
    )
}

/// The markdown memo of `snapshot_id`'s last parse. A missing or undecodable
/// memo is empty: it only saves work, the parse is the same without it.
pub(crate) fn previous_memo(conn: &Connection, snapshot_id: Option<i64>) -> Result<MarkdownMemo> {
    let Some(snapshot_id) = snapshot_id else {
        return Ok(MarkdownMemo::new());
    };
    Ok(decode_stored_memo(write::load_memo(conn, snapshot_id)?))
}

fn decode_stored_memo(payload: Option<Vec<u8>>) -> MarkdownMemo {
    payload
        .and_then(|payload| decode_memo(&payload).ok())
        .unwrap_or_default()
}

/// Whether `snapshot_id` can be served without re-parsing: it must have both a
/// current structure (step IR) and a current state model (correct
/// `STATE_VERSION` and bundled catalog digest).
pub(crate) fn snapshot_is_reusable(conn: &Connection, snapshot_id: i64) -> Result<bool> {
    Ok(
        crate::db::effects::has_structure(conn, snapshot_id, parse::steps::STRUCTURE_VERSION)?
            && crate::db::state::has_state_model(
                conn,
                snapshot_id,
                &crate::state::extract::bundled_representation_version(),
            )?,
    )
}

/// Parse, index and store one HTML document into `conn`, returning the snapshot
/// id. Used by tests and examples that already hold the HTML string.
pub fn index_html(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider: &str,
    html: String,
) -> anyhow::Result<i64> {
    let prepared = pipeline::parse_one(ParseJob {
        spec_name: spec_name.to_string(),
        base_url: base_url.to_string(),
        content_hash: hash_html(&html),
        html: Arc::new(html),
        previous_memo: previous_memo(conn, queries::get_snapshot(conn, spec_name)?)?,
        fragment: None,
    })?;
    let spec_id = write::insert_or_get_spec(conn, spec_name, base_url, provider)?;
    let now = Utc::now();
    let (snapshot_id, _) = write_parsed_html(
        conn,
        spec_id,
        spec_name,
        base_url,
        provider,
        prepared,
        &now,
        &SourceValidators::default(),
    )?;
    Ok(snapshot_id)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_parsed_html(
    conn: &Connection,
    spec_id: i64,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    prepared: ParsedHtml,
    now: &DateTime<Utc>,
    validators: &SourceValidators,
) -> Result<(i64, bool)> {
    let ParsedHtml {
        content_hash,
        parsed,
        structure_json,
        state,
        memo,
        fragment,
    } = prepared;
    let synthetic_sha = format!("hash:{content_hash}");
    write::atomic_write(conn, |conn| {
        let commit_date = now.to_rfc3339();
        let snapshot_id = match queries::get_snapshot(conn, spec_name)? {
            Some(snapshot_id) => {
                snapshot_diff::replace_snapshot_in_place(
                    conn,
                    snapshot_id,
                    &synthetic_sha,
                    &commit_date,
                    SnapshotRows {
                        sections: &parsed.sections,
                        refs: &parsed.references,
                        idl: &parsed.idl_definitions,
                    },
                )?;
                snapshot_id
            }
            None => {
                let spec_id = write::insert_or_get_spec(conn, spec_name, base_url, provider_name)?;
                let snapshot_id =
                    write::insert_snapshot(conn, spec_id, &synthetic_sha, &commit_date)?;
                write::insert_sections_bulk(conn, snapshot_id, &parsed.sections)?;
                write::insert_refs_bulk(conn, snapshot_id, &parsed.references)?;
                write::insert_idl_defs_bulk(conn, snapshot_id, &parsed.idl_definitions)?;
                snapshot_id
            }
        };
        crate::db::effects::store_structure(
            conn,
            snapshot_id,
            parse::steps::STRUCTURE_VERSION,
            &structure_json,
        )?;
        crate::db::state::store_state(conn, snapshot_id, &state)?;
        write::store_memo(conn, snapshot_id, &memo)?;
        if let Some(fragment) = fragment {
            crate::db::effects::store_fragment(
                conn,
                &crate::db::effects::FragmentKey {
                    snapshot_id,
                    spec: spec_name.to_owned(),
                    indexed_at: conn.query_row(
                        "SELECT indexed_at FROM snapshots WHERE id=?1",
                        [snapshot_id],
                        |row| row.get(0),
                    )?,
                    config_key: fragment.config_key,
                },
                &fragment.payload,
            )?;
        }

        store_update_check(
            conn,
            spec_id,
            now,
            Some(now),
            Some(&content_hash),
            validators,
        )?;
        Ok((snapshot_id, true))
    })
}

fn is_respec_source(html: &str) -> bool {
    html.contains("respec-w3c") || html.contains("respec.js") || html.contains("/respec/")
}

async fn render_via_spec_generator(url: &str, origin: Option<&str>) -> Result<String> {
    let api_url = format!(
        "https://www.w3.org/publications/spec-generator/?type=respec&url={}",
        url::form_urlencoded::byte_serialize(url.as_bytes()).collect::<String>()
    );
    let html = fetch_text(&api_url, origin).await?;
    if html.trim_start().starts_with('{') {
        anyhow::bail!(
            "spec-generator returned error: {}",
            &html[..html.len().min(200)]
        );
    }
    Ok(html)
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// GET `url`, from the `WEBSPEC_FETCH_ORIGIN` override when set.
pub(crate) async fn fetch_raw_html(url: &str) -> Result<String> {
    fetch_text(url, fetch_origin().as_deref()).await
}

async fn fetch_text(url: &str, origin: Option<&str>) -> Result<String> {
    let response = http_client()
        .get(effective_url(url, origin))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch {}: HTTP {}", url, response.status());
    }
    Ok(response.text().await?)
}

/// The URL of a spec's document: `.html`/`.txt` URLs as given, anything else
/// as a directory.
fn document_url(base_url: &str) -> String {
    if base_url.ends_with(".html") || base_url.ends_with(".txt") {
        base_url.to_string()
    } else {
        format!("{}/", base_url.trim_end_matches('/'))
    }
}

/// The freshness check for a spec. `conditional` sends the stored validators
/// and source hash, so an unchanged source costs a `304` or a hash compare;
/// without it the document is downloaded unconditionally, with no time limit.
pub(crate) fn candidate_for(
    spec_id: i64,
    spec_name: &str,
    base_url: &str,
    provider: &str,
    state: Option<&queries::UpdateCheckState>,
    conditional: bool,
) -> Candidate {
    let validators = if conditional {
        SourceValidators::stored(state)
    } else {
        SourceValidators::default()
    };
    Candidate {
        spec_id,
        spec_name: spec_name.to_owned(),
        base_url: base_url.to_owned(),
        provider: provider.to_owned(),
        etag: validators.etag,
        last_modified: validators.last_modified,
        source_hash: validators.source_hash,
        time_limited: conditional,
    }
}

/// A fetched document to parse, already written to the HTML cache.
pub(crate) struct ChangedHtml {
    pub html: String,
    pub content_hash: String,
    pub validators: SourceValidators,
}

/// A note about a freshness result, for stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefreshNote {
    pub spec: String,
    pub message: String,
}

impl std::fmt::Display for RefreshNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: freshness check failed ({}); serving the cached snapshot",
            self.spec, self.message
        )
    }
}

#[derive(Default)]
pub(crate) struct AppliedFreshness {
    /// Set when the document changed and must be parsed.
    pub changed: Option<ChangedHtml>,
    pub notes: Vec<RefreshNote>,
}

impl AppliedFreshness {
    /// Print the notes to stderr and keep the changed document.
    pub(crate) fn print_notes(self) -> Option<ChangedHtml> {
        for note in &self.notes {
            eprintln!("note: {note}");
        }
        self.changed
    }
}

/// Store a freshness result. An unchanged source bumps `last_checked` and
/// stores the new validators; a changed one is written to the HTML cache and
/// returned for parsing; a failed check changes nothing and notes that the
/// cached snapshot is served, or is an error when there is none.
pub(crate) fn apply_freshness(
    conn: &Connection,
    candidate: &Candidate,
    result: Freshness,
    now: &DateTime<Utc>,
) -> Result<AppliedFreshness> {
    match result {
        Freshness::NotModified {
            etag,
            last_modified,
        }
        | Freshness::SameContent {
            etag,
            last_modified,
        } => {
            let state = queries::get_update_check(conn, candidate.spec_id)?;
            let state = state.as_ref();
            let validators = SourceValidators {
                etag,
                last_modified,
                source_hash: state.and_then(|state| state.source_hash.clone()),
            };
            store_update_check(
                conn,
                candidate.spec_id,
                now,
                state.and_then(|state| state.last_indexed.as_ref()),
                state.and_then(|state| state.content_hash.as_deref()),
                &validators,
            )?;
            Ok(AppliedFreshness::default())
        }
        Freshness::Changed {
            html,
            content_hash,
            source_hash,
            etag,
            last_modified,
        } => {
            cache_html_for(conn, &candidate.spec_name, &content_hash, &html);
            Ok(AppliedFreshness {
                changed: Some(ChangedHtml {
                    html,
                    content_hash,
                    validators: SourceValidators {
                        etag,
                        last_modified,
                        source_hash: Some(source_hash),
                    },
                }),
                notes: Vec::new(),
            })
        }
        Freshness::Failed(message) => {
            if queries::get_snapshot(conn, &candidate.spec_name)?.is_none() {
                anyhow::bail!("Failed to fetch {}: {message}", candidate.base_url);
            }
            Ok(AppliedFreshness {
                changed: None,
                notes: vec![RefreshNote {
                    spec: candidate.spec_name.clone(),
                    message,
                }],
            })
        }
    }
}

/// Whether a failed freshness check may serve the cached snapshot: only when
/// one exists, the caller allows it (the query path) and the refresh is not
/// forced. The `update` command disallows it so a failed fetch is reported
/// rather than hidden, and `--force` always surfaces failures.
fn may_serve_cached(previous_snapshot_id: Option<i64>, allow_fallback: bool, force: bool) -> bool {
    previous_snapshot_id.is_some() && allow_fallback && !force
}

/// Sync a spec, optionally falling back to the cached snapshot when the live
/// fetch fails (see [`may_serve_cached`]).
///
/// `refetch` bypasses the on-disk HTML cache even when `force` is set. When
/// `force` is true and `refetch` is false, a cached HTML file for the stored
/// content hash is used to re-parse without a network round-trip.
#[allow(clippy::too_many_arguments)]
async fn sync_known_spec(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
    force: bool,
    allow_fallback: bool,
    refetch: bool,
    options: &FreshnessOptions,
) -> Result<(i64, bool)> {
    if provider_name == "itu" {
        return itu::sync_known_spec_pdf(conn, spec_name, base_url, force, allow_fallback).await;
    }

    let spec_id = write::insert_or_get_spec(conn, spec_name, base_url, provider_name)?;
    let previous_snapshot_id = queries::get_snapshot(conn, spec_name)?;
    let state = queries::get_update_check(conn, spec_id)?;
    let now = Utc::now();
    let is_reusable = match previous_snapshot_id {
        Some(snapshot_id) => snapshot_is_reusable(conn, snapshot_id)?,
        None => false,
    };

    if !force && is_reusable && state.as_ref().is_some_and(|s| cache_is_current(s, &now)) {
        if let Some(snapshot_id) = previous_snapshot_id {
            return Ok((snapshot_id, false));
        }
    }

    if force && !refetch {
        if let Some(stored_hash) = state.as_ref().and_then(|s| s.content_hash.as_deref()) {
            if let Some(html) = cached_html_for(conn, spec_name, stored_hash) {
                eprintln!("note: {spec_name}: re-parsing from on-disk cache (use --refetch to download fresh)");
                let validators = SourceValidators::stored(state.as_ref());
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
                    &validators,
                );
            }
        }
    }

    let conditional = !force && is_reusable && state.as_ref().is_some_and(index_is_current);
    let candidate = candidate_for(
        spec_id,
        spec_name,
        base_url,
        provider_name,
        state.as_ref(),
        conditional,
    );
    let result = check_batch(std::slice::from_ref(&candidate), options)
        .await
        .pop()
        .expect("one result per candidate");
    if let Freshness::Failed(message) = &result {
        if !may_serve_cached(previous_snapshot_id, allow_fallback, force) {
            anyhow::bail!("Failed to fetch {base_url}: {message}");
        }
    }
    match apply_freshness(conn, &candidate, result, &now)?.print_notes() {
        Some(changed) => sync_from_html(
            conn,
            spec_id,
            spec_name,
            base_url,
            provider_name,
            changed.html,
            previous_snapshot_id,
            state,
            &now,
            force,
            &changed.validators,
        ),
        None => previous_snapshot_id
            .map(|snapshot_id| (snapshot_id, false))
            .ok_or_else(|| anyhow::anyhow!("{spec_name}: no snapshot to serve")),
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
        &FreshnessOptions::default(),
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
/// When refreshing, checks the source conditionally and re-indexes only if
/// its content changed.
pub async fn ensure_indexed(
    conn: &Connection,
    spec_name: &str,
    base_url: &str,
    provider_name: &str,
) -> Result<i64> {
    let (snapshot_id, _) = sync_known_spec(
        conn,
        spec_name,
        base_url,
        provider_name,
        false,
        true,
        false,
        &FreshnessOptions::default(),
    )
    .await?;
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
        &FreshnessOptions::default(),
    )
    .await?;
    Ok(updated.then_some(snapshot_id))
}

/// Update all specs in the registry.
/// Returns vector of (spec_name, Option<snapshot_id>) pairs.
///
/// Unforced, the specs outside the freshness window are checked with one
/// concurrent conditional-GET batch per window; a failed check keeps the
/// cached snapshot. `force` re-parses from the HTML cache, or downloads
/// unconditionally with `refetch` or when the cache is missing, and surfaces
/// failures.
pub async fn update_all_specs(
    conn: &Connection,
    specs: &[(String, String, String)], // (name, base_url, provider)
    force: bool,
    refetch: bool,
) -> Vec<(String, Result<Option<i64>>)> {
    let options = FreshnessOptions {
        max_in_flight: UPDATE_MAX_IN_FLIGHT,
        ..FreshnessOptions::default()
    };
    update_specs_with(conn, specs, force, refetch, &options).await
}

async fn update_specs_with(
    conn: &Connection,
    specs: &[(String, String, String)],
    force: bool,
    refetch: bool,
    options: &FreshnessOptions,
) -> Vec<(String, Result<Option<i64>>)> {
    let threads = pipeline::parse_threads();
    let mut results: Vec<Option<Result<Option<i64>>>> = specs.iter().map(|_| None).collect();
    let mut html_specs = Vec::new();
    for (index, (name, base_url, provider)) in specs.iter().enumerate() {
        // The PDF path has a separate parser and cache policy.
        if provider == "itu" {
            results[index] =
                Some(update_if_needed(conn, name, base_url, provider, force, refetch).await);
        } else {
            html_specs.push(index);
        }
    }

    for window in html_specs.chunks(UPDATE_WINDOW) {
        let mut cached = Vec::new();
        let mut to_check = Vec::new();
        for &index in window {
            let (name, base_url, provider) = &specs[index];
            match plan_html_update(conn, name, base_url, provider, force, refetch) {
                Ok(HtmlUpdatePlan::Current) => results[index] = Some(Ok(None)),
                Ok(HtmlUpdatePlan::NeedsWork(plan)) if plan.cached_html.is_some() => {
                    cached.push((index, plan));
                }
                Ok(HtmlUpdatePlan::NeedsWork(plan)) => to_check.push((index, plan)),
                Err(e) => results[index] = Some(Err(e)),
            }
        }

        let candidates: Vec<_> = to_check.iter().map(|(_, plan)| plan.candidate()).collect();
        let checked = check_batch(&candidates, options).await;
        let mut ready = Vec::with_capacity(cached.len() + to_check.len());
        for (index, mut plan) in cached {
            let html = plan.cached_html.take().expect("partitioned on cached_html");
            let validators = SourceValidators::stored(plan.state.as_ref());
            let content_hash = hash_html(&html);
            let prepared = prepare_html_update(&mut plan, html, content_hash, validators);
            ready.push((index, plan, prepared));
        }
        for (((index, mut plan), candidate), result) in
            to_check.into_iter().zip(&candidates).zip(checked)
        {
            let outcome = if plan.force {
                match result {
                    Freshness::Failed(message) => Err(anyhow::anyhow!(
                        "Failed to fetch {}: {message}",
                        plan.base_url
                    )),
                    result => apply_freshness(conn, candidate, result, &plan.now),
                }
            } else {
                apply_freshness(conn, candidate, result, &plan.now)
            };
            let prepared = match outcome.map(AppliedFreshness::print_notes) {
                Ok(Some(changed)) => prepare_html_update(
                    &mut plan,
                    changed.html,
                    changed.content_hash,
                    changed.validators,
                ),
                Ok(None) => PreparedHtml::Done(Ok(None)),
                Err(e) => PreparedHtml::Done(Err(e)),
            };
            ready.push((index, plan, prepared));
        }

        let mut ready = ready.into_iter().peekable();
        while ready.peek().is_some() {
            let mut jobs = Vec::new();
            let mut pending = Vec::new();
            for (index, plan, prepared) in ready.by_ref().take(pipeline::chunk_len(threads)) {
                let prepared = match prepared {
                    PreparedHtml::Parse(job, validators) => {
                        jobs.push(job);
                        PreparedHtml::Parsing(validators)
                    }
                    other => other,
                };
                pending.push((index, plan, prepared));
            }
            let mut parsed = parse_chunk(jobs, threads).await.into_iter();
            for (index, plan, prepared) in pending {
                results[index] = Some(match prepared {
                    PreparedHtml::Done(result) => result,
                    PreparedHtml::Unchanged(content_hash, validators) => {
                        commit_unchanged(conn, &plan, &content_hash, &validators)
                    }
                    PreparedHtml::Parsing(validators) => parsed
                        .next()
                        .expect("one parse result per job")
                        .and_then(|parsed| commit_parsed(conn, &plan, parsed, &validators)),
                    PreparedHtml::Parse(..) => unreachable!("queued for parsing above"),
                });
            }
        }
    }

    specs
        .iter()
        .zip(results)
        .map(|((name, _, _), result)| {
            (
                name.clone(),
                result.expect("every spec has an update result"),
            )
        })
        .collect()
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
    previous_memo: Option<Vec<u8>>,
}

impl HtmlUpdateWork {
    /// Whether the stored snapshot may stand when the fetched content hash
    /// equals the stored one.
    fn unchanged_content_reusable(&self) -> bool {
        !self.force
            && self.can_reuse_unchanged
            && self.previous_snapshot_id.is_some()
            && self.state.as_ref().is_some_and(index_is_current)
    }

    fn candidate(&self) -> Candidate {
        candidate_for(
            self.spec_id,
            &self.spec_name,
            &self.base_url,
            &self.provider_name,
            self.state.as_ref(),
            self.unchanged_content_reusable(),
        )
    }
}

enum HtmlUpdatePlan {
    Current,
    NeedsWork(Box<HtmlUpdateWork>),
}

enum PreparedHtml {
    Done(Result<Option<i64>>),
    Unchanged(String, SourceValidators),
    Parse(ParseJob, SourceValidators),
    Parsing(SourceValidators),
}

/// Parse `jobs` off the async runtime. Results are in job order.
pub(crate) async fn parse_chunk(jobs: Vec<ParseJob>, threads: usize) -> Vec<Result<ParsedHtml>> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let len = jobs.len();
    match tokio::task::spawn_blocking(move || pipeline::parse_jobs(jobs, threads)).await {
        Ok(results) => results,
        Err(e) => {
            let message = format!("parse worker failed: {e}");
            (0..len)
                .map(|_| Err(anyhow::anyhow!("{message}")))
                .collect()
        }
    }
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
    let is_reusable = match previous_snapshot_id {
        Some(snapshot_id) => snapshot_is_reusable(conn, snapshot_id)?,
        None => false,
    };
    if !force
        && is_reusable
        && state
            .as_ref()
            .is_some_and(|state| cache_is_current(state, &now))
    {
        return Ok(HtmlUpdatePlan::Current);
    }

    let cached_html = if force && !refetch {
        state
            .as_ref()
            .and_then(|state| state.content_hash.as_deref())
            .and_then(|hash| cached_html_for(conn, spec_name, hash))
    } else {
        None
    };
    let previous_memo = match previous_snapshot_id {
        Some(snapshot_id) => write::load_memo(conn, snapshot_id)?,
        None => None,
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
        can_reuse_unchanged: is_reusable,
        previous_memo,
    })))
}

/// Decide whether `html` (hashing to `content_hash`) needs a parse: content
/// equal to the stored snapshot's only refreshes the update check.
fn prepare_html_update(
    plan: &mut HtmlUpdateWork,
    html: String,
    content_hash: String,
    validators: SourceValidators,
) -> PreparedHtml {
    if plan.unchanged_content_reusable()
        && plan
            .state
            .as_ref()
            .is_some_and(|state| state.content_hash.as_deref() == Some(content_hash.as_str()))
    {
        return PreparedHtml::Unchanged(content_hash, validators);
    }
    PreparedHtml::Parse(
        ParseJob {
            spec_name: plan.spec_name.clone(),
            base_url: plan.base_url.clone(),
            html: Arc::new(html),
            content_hash,
            previous_memo: decode_stored_memo(plan.previous_memo.take()),
            fragment: None,
        },
        validators,
    )
}

fn commit_unchanged(
    conn: &Connection,
    plan: &HtmlUpdateWork,
    content_hash: &str,
    validators: &SourceValidators,
) -> Result<Option<i64>> {
    store_update_check(
        conn,
        plan.spec_id,
        &plan.now,
        plan.state
            .as_ref()
            .and_then(|state| state.last_indexed.as_ref()),
        Some(content_hash),
        validators,
    )?;
    Ok(None)
}

fn commit_parsed(
    conn: &Connection,
    plan: &HtmlUpdateWork,
    parsed: ParsedHtml,
    validators: &SourceValidators,
) -> Result<Option<i64>> {
    let (snapshot_id, _) = write_parsed_html(
        conn,
        plan.spec_id,
        &plan.spec_name,
        &plan.base_url,
        &plan.provider_name,
        parsed,
        &plan.now,
        validators,
    )?;
    Ok(Some(snapshot_id))
}

/// Re-parse one or all indexed specs from the on-disk HTML cache, writing
/// fresh sections/refs/IDL to the DB without any network access.
///
/// Specs that have no cached HTML file are reported to stderr and skipped.
/// `spec` filters to a single spec; `providers` filters by provider name
/// (empty = all). Returns `(spec_name, Option<snapshot_id>)` pairs.
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
    reparse_cached(conn, html_cache_dir_for(conn).as_deref(), &filtered).await
}

/// Re-parse `specs` from the HTML cache directory `cache_dir`, parsing each
/// chunk in parallel and writing its results in spec order before reading the
/// next.
async fn reparse_cached(
    conn: &Connection,
    cache_dir: Option<&Path>,
    specs: &[(String, String, String)],
) -> Result<Vec<(String, Option<i64>)>> {
    let now = Utc::now();
    let threads = pipeline::parse_threads();
    let mut results = Vec::with_capacity(specs.len());

    for chunk in specs.chunks(pipeline::chunk_len(threads)) {
        let mut jobs = Vec::new();
        let mut pending = Vec::with_capacity(chunk.len());
        for (name, base_url, provider) in chunk {
            let spec_id = write::insert_or_get_spec(conn, name, base_url, provider)?;
            let state = queries::get_update_check(conn, spec_id)?;
            let html = match state.as_ref().and_then(|s| s.content_hash.as_deref()) {
                Some(hash) => match cache_dir.and_then(|dir| read_html_cache(dir, name, hash)) {
                    Some(html) => html,
                    None => {
                        eprintln!("reparse: {name}: no cache file found, skipping");
                        pending.push(None);
                        continue;
                    }
                },
                None => {
                    eprintln!("reparse: {name}: no stored content hash, skipping");
                    pending.push(None);
                    continue;
                }
            };
            let memo = match queries::get_snapshot(conn, name)
                .and_then(|snapshot_id| previous_memo(conn, snapshot_id))
            {
                Ok(memo) => memo,
                Err(e) => {
                    eprintln!("reparse: {name}: failed: {e}");
                    pending.push(None);
                    continue;
                }
            };
            jobs.push(ParseJob {
                spec_name: name.clone(),
                base_url: base_url.clone(),
                content_hash: hash_html(&html),
                html: Arc::new(html),
                previous_memo: memo,
                fragment: None,
            });
            pending.push(Some((spec_id, SourceValidators::stored(state.as_ref()))));
        }

        let mut parsed = parse_chunk(jobs, threads).await.into_iter();
        for ((name, base_url, provider), spec_id) in chunk.iter().zip(pending) {
            let Some((spec_id, validators)) = spec_id else {
                results.push((name.clone(), None));
                continue;
            };
            let written = parsed
                .next()
                .expect("one parse result per job")
                .and_then(|prepared| {
                    write_parsed_html(
                        conn,
                        spec_id,
                        name,
                        base_url,
                        provider,
                        prepared,
                        &now,
                        &validators,
                    )
                });
            match written {
                Ok((snapshot_id, _)) => results.push((name.clone(), Some(snapshot_id))),
                Err(e) => {
                    eprintln!("reparse: {name}: failed: {e}");
                    results.push((name.clone(), None));
                }
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
        let mut plan = HtmlUpdateWork {
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
            previous_memo: None,
        };
        let html = plan.cached_html.take().unwrap();
        let content_hash = hash_html(&html);
        let PreparedHtml::Parse(job, validators) =
            prepare_html_update(&mut plan, html, content_hash, SourceValidators::default())
        else {
            panic!("a forced update parses");
        };
        let parsed = parse_chunk(vec![job], 1).await.pop().unwrap().unwrap();
        let snapshot_id = commit_parsed(&conn, &plan, parsed, &validators)
            .unwrap()
            .unwrap();
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

    #[cfg(feature = "native")]
    #[test]
    fn index_html_stores_state_and_missing_state_model_forces_reparse() {
        let conn = crate::db::open_test_db().unwrap();
        let html = crate::state::testing::MINI.to_string();
        let snapshot = index_html(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
            html,
        )
        .unwrap();
        let version = crate::state::extract::bundled_representation_version();
        assert!(crate::db::state::has_state_model(&conn, snapshot, &version).unwrap());
        conn.execute("DELETE FROM state_models", []).unwrap();
        assert!(!crate::db::state::has_state_model(&conn, snapshot, &version).unwrap());
        assert!(!snapshot_is_reusable(&conn, snapshot).unwrap());
    }

    #[cfg(feature = "native")]
    #[test]
    fn reindex_keeps_snapshot_id_and_reuses_the_memo() {
        let conn = crate::db::open_test_db().unwrap();
        let html = include_str!("../../tests/fixtures/effects/structure/wattsi.html").to_string();
        let first = index_html(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
            html.clone(),
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE section_deletes(anchor TEXT);
             CREATE TEMP TRIGGER count_section_deletes AFTER DELETE ON sections
             BEGIN INSERT INTO section_deletes VALUES (old.anchor); END;",
        )
        .unwrap();
        let edited = format!("{html}<h2 id=\"new-heading\">New</h2>");
        let second = index_html(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
            edited,
        )
        .unwrap();
        assert_eq!(first, second, "the snapshot is updated in place");
        assert!(crate::db::write::load_memo(&conn, second)
            .unwrap()
            .is_some());
        let deletes: i64 = conn
            .query_row("SELECT COUNT(*) FROM section_deletes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(deletes, 0, "unchanged sections are kept, not re-inserted");
        let new_heading: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sections WHERE snapshot_id=?1 AND anchor='new-heading'",
                [second],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_heading, 1);
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
            etag: None,
            last_modified: None,
            source_hash: None,
        }
    }

    // A failed check serves the cached snapshot only on the query path
    // (fallback allowed, not forced) and only when a snapshot exists.
    #[test]
    fn failed_check_serves_the_cache_only_on_the_unforced_query_path() {
        assert!(may_serve_cached(Some(42), true, false));
        assert!(!may_serve_cached(None, true, false));
        assert!(!may_serve_cached(Some(42), false, false));
        assert!(!may_serve_cached(Some(42), true, true));
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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
            &SourceValidators::default(),
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

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn parallel_reparse_equals_indexing_one_by_one() {
        use crate::db::snapshot_diff::tests::logical_rows;
        let specs = [
            (
                "HTML",
                "https://html.spec.whatwg.org/",
                "whatwg",
                include_str!("../../tests/fixtures/effects/structure/wattsi.html"),
            ),
            (
                "DOM",
                "https://dom.spec.whatwg.org/",
                "whatwg",
                include_str!("../../tests/fixtures/effects/structure/bikeshed.html"),
            ),
            (
                "ECMA-262",
                "https://tc39.es/ecma262/",
                "tc39",
                include_str!("../../tests/fixtures/effects/structure/ecmarkup.html"),
            ),
        ];
        let cache = tempfile::tempdir().unwrap();
        let reparsed = db::open_test_db().unwrap();
        let expected = db::open_test_db().unwrap();
        for (name, base_url, provider, html) in specs {
            // Seed a stub so the reparse replaces rows in place, then cache the
            // real document under the stub's content hash.
            let stub = format!("<h2 id=\"stub-{name}\">Stub</h2>");
            index_html(&reparsed, name, base_url, provider, stub.clone()).unwrap();
            write_html_cache(cache.path(), name, &hash_html(&stub), html).unwrap();
            index_html(&expected, name, base_url, provider, html.to_string()).unwrap();
        }
        let listed: Vec<_> = specs
            .iter()
            .map(|(name, base_url, provider, _)| {
                (name.to_string(), base_url.to_string(), provider.to_string())
            })
            .collect();

        let results = reparse_cached(&reparsed, Some(cache.path()), &listed)
            .await
            .unwrap();

        assert_eq!(
            results
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["HTML", "DOM", "ECMA-262"]
        );
        for ((name, snapshot), (spec, ..)) in results.iter().zip(specs) {
            assert_eq!(name, spec);
            let snapshot = snapshot.expect("every spec has cached HTML");
            let want = queries::get_snapshot(&expected, spec).unwrap().unwrap();
            let rows = logical_rows(&reparsed, snapshot);
            assert!(!rows.is_empty());
            assert_eq!(rows, logical_rows(&expected, want), "{spec}");
        }
    }

    fn seeded_candidate(conn: &Connection) -> Candidate {
        let html = "<h2 id=\"seed\">Seed</h2>".to_string();
        index_html(conn, "DOM", "https://dom.spec.whatwg.org/", "whatwg", html).unwrap();
        let spec_id =
            write::insert_or_get_spec(conn, "DOM", "https://dom.spec.whatwg.org/", "whatwg")
                .unwrap();
        candidate_for(
            spec_id,
            "DOM",
            "https://dom.spec.whatwg.org/",
            "whatwg",
            queries::get_update_check(conn, spec_id).unwrap().as_ref(),
            true,
        )
    }

    #[test]
    fn failed_freshness_changes_nothing_and_notes_the_cached_snapshot() {
        let conn = db::open_test_db().unwrap();
        let candidate = seeded_candidate(&conn);
        let before = queries::get_update_check(&conn, candidate.spec_id)
            .unwrap()
            .unwrap();

        let applied = apply_freshness(
            &conn,
            &candidate,
            Freshness::Failed("HTTP 503".into()),
            &Utc::now(),
        )
        .unwrap();

        assert!(applied.changed.is_none());
        assert_eq!(
            applied.notes,
            vec![RefreshNote {
                spec: "DOM".into(),
                message: "HTTP 503".into()
            }]
        );
        assert_eq!(
            applied.notes[0].to_string(),
            "DOM: freshness check failed (HTTP 503); serving the cached snapshot"
        );
        let after = queries::get_update_check(&conn, candidate.spec_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.last_checked, before.last_checked);
    }

    #[test]
    fn failed_freshness_without_a_snapshot_is_an_error() {
        let conn = db::open_test_db().unwrap();
        let spec_id =
            write::insert_or_get_spec(&conn, "DOM", "https://dom.spec.whatwg.org/", "whatwg")
                .unwrap();
        let candidate = candidate_for(
            spec_id,
            "DOM",
            "https://dom.spec.whatwg.org/",
            "whatwg",
            None,
            false,
        );
        let result = apply_freshness(
            &conn,
            &candidate,
            Freshness::Failed("HTTP 503".into()),
            &Utc::now(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn not_modified_bumps_last_checked_and_stores_validators() {
        let conn = db::open_test_db().unwrap();
        let candidate = seeded_candidate(&conn);
        let before = queries::get_update_check(&conn, candidate.spec_id)
            .unwrap()
            .unwrap();
        let later = before.last_checked + chrono::Duration::hours(25);

        let applied = apply_freshness(
            &conn,
            &candidate,
            Freshness::NotModified {
                etag: Some("W/\"1\"".into()),
                last_modified: None,
            },
            &later,
        )
        .unwrap();

        assert!(applied.changed.is_none());
        let after = queries::get_update_check(&conn, candidate.spec_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.last_checked, later);
        assert_eq!(after.etag.as_deref(), Some("W/\"1\""));
        assert_eq!(after.content_hash, before.content_hash);
        assert_eq!(after.last_indexed, before.last_indexed);
    }

    #[test]
    fn html_cache_follows_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_db_at(&dir.path().join("index.db")).unwrap();
        assert_eq!(html_cache_dir_for(&conn), Some(dir.path().join("html")));
        let candidate = seeded_candidate(&conn);
        let changed = Freshness::Changed {
            html: "<p>new</p>".into(),
            content_hash: "abc".into(),
            source_hash: "abc".into(),
            etag: None,
            last_modified: None,
        };
        apply_freshness(&conn, &candidate, changed, &Utc::now()).unwrap();
        assert!(html_cache_path(&dir.path().join("html"), "DOM", "abc").exists());

        let memory = db::open_test_db().unwrap();
        assert_eq!(html_cache_dir_for(&memory), None);
        let candidate = seeded_candidate(&memory);
        let changed = Freshness::Changed {
            html: "<p>new</p>".into(),
            content_hash: "abc".into(),
            source_hash: "abc".into(),
            etag: None,
            last_modified: None,
        };
        let applied = apply_freshness(&memory, &candidate, changed, &Utc::now()).unwrap();
        assert!(applied.changed.is_some(), "the document still gets parsed");
    }

    #[tokio::test]
    async fn update_checks_stale_specs_conditionally_and_parses_only_changes() {
        let stub = testing::HttpStub::start();
        let conn = db::open_test_db().unwrap();
        let specs: Vec<_> = ["dom", "infra"]
            .iter()
            .map(|host| {
                (
                    host.to_uppercase(),
                    format!("https://{host}.spec.whatwg.org/"),
                    "whatwg".to_string(),
                )
            })
            .collect();
        stub.put(
            "dom.spec.whatwg.org/",
            "<h2 id=\"d\">D</h2>",
            Some("\"d1\""),
            None,
        );
        stub.put(
            "infra.spec.whatwg.org/",
            "<h2 id=\"i\">I</h2>",
            Some("\"i1\""),
            None,
        );
        let options = FreshnessOptions {
            timeout: std::time::Duration::from_secs(2),
            max_in_flight: 4,
            origin: Some(stub.origin()),
        };
        let first = update_specs_with(&conn, &specs, false, false, &options).await;
        assert!(first.iter().all(|(_, r)| matches!(r, Ok(Some(_)))));

        conn.execute(
            "UPDATE update_checks SET last_checked = '2000-01-01T00:00:00+00:00'",
            [],
        )
        .unwrap();
        stub.put(
            "infra.spec.whatwg.org/",
            "<h2 id=\"i2\">I2</h2>",
            Some("\"i2\""),
            None,
        );
        let second = update_specs_with(&conn, &specs, false, false, &options).await;

        assert!(matches!(second[0].1, Ok(None)), "DOM answered 304");
        assert!(matches!(second[1].1, Ok(Some(_))), "INFRA changed");
        let requests = stub.requests();
        assert_eq!(requests.len(), 4);
        assert!(requests[2..]
            .iter()
            .all(|(_, if_none_match, _)| if_none_match.is_some()));
        let dom = write::insert_or_get_spec(&conn, "DOM", &specs[0].1, "whatwg").unwrap();
        let state = queries::get_update_check(&conn, dom).unwrap().unwrap();
        assert!(state.last_checked.timestamp() > 946_684_800);
        assert_eq!(state.etag.as_deref(), Some("\"d1\""));
    }

    #[tokio::test]
    async fn only_conditional_checks_are_time_limited() {
        let stub = testing::HttpStub::start();
        let conn = db::open_test_db().unwrap();
        let specs = vec![(
            "DOM".to_string(),
            "https://dom.spec.whatwg.org/".to_string(),
            "whatwg".to_string(),
        )];
        stub.put(
            "dom.spec.whatwg.org/",
            "<h2 id=\"d\">D</h2>",
            Some("\"d1\""),
            None,
        );
        stub.delay(
            "dom.spec.whatwg.org/",
            std::time::Duration::from_millis(600),
        );
        let options = FreshnessOptions {
            timeout: std::time::Duration::from_millis(200),
            max_in_flight: 4,
            origin: Some(stub.origin()),
        };

        let first = update_specs_with(&conn, &specs, false, false, &options).await;
        assert!(matches!(first[0].1, Ok(Some(_))), "{:?}", first[0].1);
        let forced = update_specs_with(&conn, &specs, true, true, &options).await;
        assert!(matches!(forced[0].1, Ok(Some(_))), "{:?}", forced[0].1);

        conn.execute(
            "UPDATE update_checks SET last_checked = '2000-01-01T00:00:00+00:00'",
            [],
        )
        .unwrap();
        let checked = update_specs_with(&conn, &specs, false, false, &options).await;
        assert!(matches!(checked[0].1, Ok(None)), "{:?}", checked[0].1);
        let dom = write::insert_or_get_spec(&conn, "DOM", &specs[0].1, "whatwg").unwrap();
        let state = queries::get_update_check(&conn, dom).unwrap().unwrap();
        assert_eq!(
            state.last_checked.timestamp(),
            946_684_800,
            "the timed-out check is not recorded"
        );
    }
}
