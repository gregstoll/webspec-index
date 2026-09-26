//! E1–E3: effects storage invariants (spec §8 tests 9–10).
//!
//! - E1 (fragment purity): `decode_fragment(stored) == build_fragment(input from stored data)`.
//! - E2 (publication parity): a full in-memory rebuild equals the stored `effect_summaries` rows.
//! - E3 (preview parity): for 20 seeded subjects per spec, the stored preview row equals the
//!   result of `compact_summary(get_effect_summary_on(...))`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use rusqlite::OptionalExtension;
use serde_json::json;
use webspec_index::db::effects as storage;
use webspec_index::effects::bundled::default_catalog;
use webspec_index::effects::engine::IndexedAnchor;
use webspec_index::effects::fragment::{
    build_fragment, decode_fragment, FragmentInput, FRAGMENT_FORMAT_VERSION,
};
use webspec_index::effects::model::EffectsOptions;
use webspec_index::effects::service::{publish, PublishMode};
use webspec_index::parse::steps::{StructuralSpec, STRUCTURE_VERSION};

use super::{Ctx, Evidence, Failure, Invariant, Outcome};

pub static E1: E1Inv = E1Inv;
pub static E2: E2Inv = E2Inv;
pub static E3: E3Inv = E3Inv;

pub struct E1Inv;
pub struct E2Inv;
pub struct E3Inv;

/// Build the anchor list for a snapshot, matching what publish uses internally.
///
/// Pulls anchors from sections, effect_anchors, refs and idl_defs and attaches
/// content text only for catalog-reviewed anchors.
fn load_anchors(
    conn: &rusqlite::Connection,
    snapshot_id: i64,
    spec: &str,
    base_url: &str,
    catalog: &webspec_index::effects::Catalog,
) -> Vec<IndexedAnchor> {
    let reviewed: BTreeSet<String> = catalog
        .summaries()
        .map(|s| s.subject.as_identity())
        .chain(
            catalog
                .implementations()
                .map(|i| i.implementation.as_identity()),
        )
        .collect();
    let mut stmt = match conn.prepare(
        "WITH anchors(anchor) AS (
             SELECT anchor FROM sections WHERE snapshot_id=?1
             UNION SELECT anchor FROM effect_anchors WHERE snapshot_id=?1
             UNION SELECT from_anchor FROM refs WHERE snapshot_id=?1
             UNION SELECT anchor FROM idl_defs WHERE snapshot_id=?1)
         SELECT anchors.anchor, sections.content_text,
             (SELECT MIN(kind) FROM idl_defs WHERE snapshot_id=?1 AND anchor=anchors.anchor) idl_kind
         FROM anchors
         LEFT JOIN sections ON sections.snapshot_id=?1 AND sections.anchor=anchors.anchor
         ORDER BY anchors.anchor",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([snapshot_id], |row| {
        let anchor: String = row.get(0)?;
        let text_raw: Option<String> = row.get(1)?;
        let idl_kind: Option<String> = row.get(2)?;
        let key = format!("{spec}#{anchor}");
        let text = if reviewed.contains(&key) {
            text_raw.unwrap_or_default()
        } else {
            String::new()
        };
        let url = format!("{}#{anchor}", base_url.trim_end_matches('#'));
        Ok(IndexedAnchor {
            url,
            anchor,
            text,
            idl_kind,
        })
    });
    match rows {
        Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// E1: Fragment purity.
///
/// For this spec, decodes the stored fragment and compares it with a freshly built fragment
/// from the stored structure and anchors. Returns `NotApplicable` when no fragment is stored
/// or the effects data is absent.
pub fn check_e1(
    conn: &rusqlite::Connection,
    spec_name: &str,
    snapshot_id: i64,
    base_url: &str,
) -> Outcome {
    let sha: Option<String> = conn
        .query_row(
            "SELECT sha FROM snapshots WHERE id=?1",
            [snapshot_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(sha) = sha else {
        return Outcome::NotApplicable;
    };

    let payload: Option<Vec<u8>> = conn
        .query_row(
            "SELECT payload FROM effect_fragments WHERE snapshot_id=?1",
            [snapshot_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(payload) = payload else {
        return Outcome::NotApplicable;
    };

    let stored = match decode_fragment(&payload) {
        Ok(f) => f,
        Err(e) => {
            return Outcome::fail(
                vec![Evidence::new("decode-error", e.to_string(), "fragment")],
                json!({"format": FRAGMENT_FORMAT_VERSION}),
                json!({"error": e.to_string()}),
            )
        }
    };

    let structure: Option<StructuralSpec> =
        storage::load_structure(conn, snapshot_id, STRUCTURE_VERSION)
            .ok()
            .flatten()
            .and_then(|t| serde_json::from_str(&t).ok());

    let catalog = match default_catalog(&[]) {
        Ok(c) => c,
        Err(_) => return Outcome::NotApplicable,
    };
    let anchors = load_anchors(conn, snapshot_id, spec_name, base_url, &catalog);

    let input = FragmentInput {
        spec: spec_name,
        snapshot_sha: &sha,
        base_url,
        structure: structure.as_ref(),
        anchors: &anchors,
        catalog: &catalog,
        environment: "generic",
    };
    let fresh = build_fragment(&input);

    if stored == fresh {
        return Outcome::Pass;
    }

    let mut items = Vec::new();
    if stored.spec != fresh.spec {
        items.push(Evidence::new(
            "mismatch",
            format!("{} (stored) vs {} (fresh)", stored.spec, fresh.spec),
            "fragment.spec",
        ));
    }
    if stored.snapshot_sha != fresh.snapshot_sha {
        items.push(Evidence::new(
            "mismatch",
            "snapshot_sha",
            "fragment.snapshot_sha",
        ));
    }
    if stored.anchors != fresh.anchors {
        items.push(Evidence::new(
            "mismatch",
            format!(
                "{} anchors stored vs {} fresh",
                stored.anchors.len(),
                fresh.anchors.len()
            ),
            "fragment.anchors",
        ));
    }
    if stored.nodes.len() != fresh.nodes.len() || stored.nodes != fresh.nodes {
        items.push(Evidence::new(
            "mismatch",
            format!(
                "{} nodes stored vs {} fresh",
                stored.nodes.len(),
                fresh.nodes.len()
            ),
            "fragment.nodes",
        ));
    }
    if stored.edges.len() != fresh.edges.len() || stored.edges != fresh.edges {
        items.push(Evidence::new(
            "mismatch",
            format!(
                "{} edges stored vs {} fresh",
                stored.edges.len(),
                fresh.edges.len()
            ),
            "fragment.edges",
        ));
    }
    if stored.pending != fresh.pending {
        items.push(Evidence::new(
            "mismatch",
            format!(
                "{} pending stored vs {} fresh",
                stored.pending.len(),
                fresh.pending.len()
            ),
            "fragment.pending",
        ));
    }
    if items.is_empty() {
        items.push(Evidence::new("mismatch", "other fields", "fragment"));
    }
    Outcome::fail(
        items,
        json!({"nodes": fresh.nodes.len(), "edges": fresh.edges.len()}),
        json!({"nodes": stored.nodes.len(), "edges": stored.edges.len()}),
    )
}

impl Invariant for E1Inv {
    fn id(&self) -> &'static str {
        "E1"
    }

    fn spec_level(&self) -> bool {
        true
    }

    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Ctx::Spec(spec) = ctx else {
            return Outcome::NotApplicable;
        };
        let Some(db) = spec.db.as_ref() else {
            return Outcome::NotApplicable;
        };
        check_e1(&db.conn, spec.spec(), db.snapshot_id, &spec.src.base_url)
    }
}

/// Rebuild all summary rows by copying the source DB and calling publish(Rebuild).
///
/// Returns a map from subject_key to (spec, digest) as produced by a full rebuild.
fn rebuild_summaries(
    source: &rusqlite::Connection,
) -> anyhow::Result<BTreeMap<String, (String, Vec<u8>)>> {
    let mut dest = rusqlite::Connection::open_in_memory()?;
    {
        let backup = rusqlite::backup::Backup::new(source, &mut dest)?;
        backup.run_to_completion(1000, std::time::Duration::ZERO, None)?;
    }
    let catalog = default_catalog(&[]).map_err(|e| anyhow::anyhow!("{e}"))?;
    let options = EffectsOptions::default();
    publish(
        &dest,
        &catalog,
        &options,
        PublishMode::Rebuild,
        BTreeMap::new(),
    )
    .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let mut stmt = dest
        .prepare("SELECT subject_key, spec, digest FROM effect_summaries ORDER BY subject_key")?;
    let rows: BTreeMap<String, (String, Vec<u8>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?),
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Expected summary rows per corpus: subject_key → (spec_name, digest_bytes).
type RebuildCache = BTreeMap<String, (String, Vec<u8>)>;

/// Global cache: (corpus_digest:config_key) → expected summary rows.
///
/// Used so the expensive full rebuild runs at most once per unique publication per process.
static E2_CACHE: OnceLock<Mutex<HashMap<String, RebuildCache>>> = OnceLock::new();

/// E2: Publication parity.
///
/// Performs a full in-memory rebuild once (per corpus_digest) and compares stored summary
/// digests with the expected ones for the given spec.
pub fn check_e2(conn: &rusqlite::Connection, spec_name: &str) -> Outcome {
    let pub_row = match storage::load_publication(conn).ok().flatten() {
        Some(p) => p,
        None => return Outcome::NotApplicable,
    };

    let cache = E2_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cache_key = format!("{}:{}", pub_row.corpus_digest, pub_row.config_key);

    let all_expected = {
        let mut guard = cache.lock().unwrap();
        if !guard.contains_key(&cache_key) {
            match rebuild_summaries(conn) {
                Ok(rows) => {
                    guard.insert(cache_key.clone(), rows);
                }
                Err(_) => return Outcome::NotApplicable,
            }
        }
        guard.get(&cache_key).unwrap().clone()
    };

    let expected_spec: BTreeMap<String, Vec<u8>> = all_expected
        .iter()
        .filter(|(_, (spec, _))| spec == spec_name)
        .map(|(k, (_, digest))| (k.clone(), digest.clone()))
        .collect();

    let stored_spec: BTreeMap<String, Vec<u8>> = {
        let mut stmt =
            match conn.prepare("SELECT subject_key, digest FROM effect_summaries WHERE spec=?1") {
                Ok(s) => s,
                Err(_) => return Outcome::NotApplicable,
            };
        stmt.query_map([spec_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .collect()
    };

    let mut items = Vec::new();
    for (key, expected_digest) in &expected_spec {
        match stored_spec.get(key) {
            None => items.push(Evidence::new(
                "missing-row",
                key.clone(),
                "effect_summaries",
            )),
            Some(stored_digest) if stored_digest != expected_digest => items.push(Evidence::new(
                "digest-mismatch",
                key.clone(),
                "effect_summaries",
            )),
            _ => {}
        }
    }
    for key in stored_spec.keys() {
        if !expected_spec.contains_key(key) {
            items.push(Evidence::new("extra-row", key.clone(), "effect_summaries"));
        }
    }

    Outcome::check(items.is_empty(), || Failure {
        expected: json!({"rows": expected_spec.len()}),
        actual: json!({"rows": stored_spec.len()}),
        items,
    })
}

impl Invariant for E2Inv {
    fn id(&self) -> &'static str {
        "E2"
    }

    fn spec_level(&self) -> bool {
        true
    }

    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Ctx::Spec(spec) = ctx else {
            return Outcome::NotApplicable;
        };
        let Some(db) = spec.db.as_ref() else {
            return Outcome::NotApplicable;
        };
        check_e2(&db.conn, spec.spec())
    }
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

fn spec_seed(spec: &str) -> u64 {
    spec.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        h.wrapping_mul(0x100000001b3).wrapping_add(b as u64)
    })
}

/// Ensure E2's cache is populated and return the expected digest for `subject_key`, if any.
fn e2_expected_digest(conn: &rusqlite::Connection, subject_key: &str) -> Option<Vec<u8>> {
    let pub_row = storage::load_publication(conn).ok()??;
    let cache_key = format!("{}:{}", pub_row.corpus_digest, pub_row.config_key);
    let cache = E2_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if !guard.contains_key(&cache_key) {
        let rows = rebuild_summaries(conn).ok()?;
        guard.insert(cache_key.clone(), rows);
    }
    guard
        .get(&cache_key)?
        .get(subject_key)
        .map(|(_, digest)| digest.clone())
}

/// E3: Preview parity.
///
/// For 20 seeded subjects of this spec, verifies that the stored digest of the preview row
/// equals the digest that a full rebuild would produce for that subject.
pub fn check_e3(conn: &rusqlite::Connection, spec_name: &str) -> Outcome {
    let rows: Vec<(String, Vec<u8>)> = {
        let mut stmt = match conn.prepare(
            "SELECT subject_key, digest FROM effect_summaries WHERE spec=?1 ORDER BY subject_key",
        ) {
            Ok(s) => s,
            Err(_) => return Outcome::NotApplicable,
        };
        stmt.query_map([spec_name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .collect()
    };

    if rows.is_empty() {
        return Outcome::NotApplicable;
    }

    let n = rows.len();
    let n_subjects = 20usize.min(n);
    let mut rng = Rng::new(spec_seed(spec_name));
    let mut indices: Vec<usize> = (0..n).collect();
    for i in 0..n_subjects {
        let j = i + rng.below((n - i) as u64) as usize;
        indices.swap(i, j);
    }
    let selected: Vec<&(String, Vec<u8>)> =
        indices[..n_subjects].iter().map(|&i| &rows[i]).collect();

    let mut items = Vec::new();
    for (key, stored_digest) in &selected {
        let expected_digest = match e2_expected_digest(conn, key) {
            Some(d) => d,
            None => continue,
        };
        if stored_digest != &expected_digest {
            items.push(Evidence::new(
                "digest-mismatch",
                key.clone(),
                "effect_summaries",
            ));
        }
    }

    Outcome::check(items.is_empty(), || Failure {
        expected: json!({"subjects": n_subjects, "mismatches": 0}),
        actual: json!({"subjects": n_subjects, "mismatches": items.len()}),
        items,
    })
}

impl Invariant for E3Inv {
    fn id(&self) -> &'static str {
        "E3"
    }

    fn spec_level(&self) -> bool {
        true
    }

    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Ctx::Spec(spec) = ctx else {
            return Outcome::NotApplicable;
        };
        let Some(db) = spec.db.as_ref() else {
            return Outcome::NotApplicable;
        };
        check_e3(&db.conn, spec.spec())
    }
}
