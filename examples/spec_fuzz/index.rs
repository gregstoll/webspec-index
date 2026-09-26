//! Read-only index access: census, gating (§5.2) and snapshot pinning (§5.3).
//!
//! Never calls `db::open_or_create_db`: that purges the whole index on a version mismatch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use webspec_index::model::{ParsedReference, ParsedSection, SectionType};
use webspec_index::parse::{steps::STRUCTURE_VERSION, INDEX_VERSION};

pub fn open_ro(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", path.display()))?;
    conn.busy_timeout(Duration::from_secs(60))?;
    Ok(conn)
}

pub fn default_db_path() -> PathBuf {
    webspec_index::db::get_db_path()
}

#[derive(Clone, Debug, Serialize)]
pub struct SpecEntry {
    pub name: String,
    pub base_url: String,
    pub provider: String,
    pub snapshot_id: i64,
    pub sha: String,
    pub cache_path: PathBuf,
    /// G4: a structure row for this build's `STRUCTURE_VERSION` exists.
    pub has_structure: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Untestable {
    pub name: String,
    pub reason: String,
    /// The index is mid-rebuild or stale for this spec (G2/G3), as opposed to never indexed.
    pub partial: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct Census {
    pub index_version: Option<String>,
    pub specs_total: usize,
    pub testable: Vec<SpecEntry>,
    pub untestable: Vec<Untestable>,
}

impl Census {
    pub fn partial(&self) -> Vec<&Untestable> {
        self.untestable.iter().filter(|u| u.partial).collect()
    }

    pub fn print(&self) {
        eprintln!(
            "index census: {} specs, {} testable, {} untestable ({} partial/stale), index_version {}",
            self.specs_total,
            self.testable.len(),
            self.untestable.len(),
            self.partial().len(),
            self.index_version.as_deref().unwrap_or("<none>"),
        );
        let no_structure = self.testable.iter().filter(|e| !e.has_structure).count();
        if no_structure > 0 {
            eprintln!(
                "  G4: {no_structure} testable specs have no structure for STRUCTURE_VERSION {STRUCTURE_VERSION}; S2/S3 skip them"
            );
        }
        let mut reasons: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for u in &self.untestable {
            let key = u.reason.split(':').next().unwrap_or(&u.reason);
            reasons.entry(key).or_default().push(&u.name);
        }
        for (r, names) in reasons {
            let shown: Vec<&str> = names.iter().take(8).copied().collect();
            let more = if names.len() > 8 {
                format!(" (+{})", names.len() - 8)
            } else {
                String::new()
            };
            eprintln!("  {r}: {} [{}{more}]", names.len(), shown.join(", "));
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whole-index check: refuse to run against an index built by another version.
pub fn check_index_version(conn: &Connection) -> Result<String> {
    let has_meta: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='meta'",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        bail!("the index has no meta table: it was never built or is being created");
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'index_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match v {
        Some(v) if v == INDEX_VERSION => Ok(v),
        Some(v) => bail!(
            "index was built by webspec-index {v}, this harness is {INDEX_VERSION}: \
             rebuild with this build (`webspec-index update --force`) or check out the matching revision"
        ),
        None => bail!("the index has no index_version: it is empty or being rebuilt"),
    }
}

pub fn census(conn: &Connection, db_dir: &Path) -> Result<Census> {
    let mut out = Census {
        index_version: check_index_version(conn).ok(),
        ..Default::default()
    };
    let mut stmt = conn.prepare(
        "SELECT sp.id, sp.name, sp.base_url, sp.provider,
                sn.id, sn.sha, sn.index_version,
                uc.index_version, uc.content_hash,
                (SELECT COUNT(*) FROM effect_structures es
                  WHERE es.snapshot_id = sn.id AND es.representation_version = ?1)
         FROM specs sp
         LEFT JOIN snapshots sn ON sn.spec_id = sp.id AND sn.pr_number IS NULL AND sn.sha LIKE 'hash:%'
         LEFT JOIN update_checks uc ON uc.spec_id = sp.id
         ORDER BY sp.name",
    )?;
    let rows = stmt.query_map([STRUCTURE_VERSION], |r| {
        Ok((
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
            r.get::<_, i64>(9)?,
        ))
    })?;
    for row in rows {
        let (name, base_url, provider, snapshot_id, sha, snap_v, uc_v, uc_hash, structures) = row?;
        out.specs_total += 1;
        let untestable = |reason: String, partial: bool| Untestable {
            name: name.clone(),
            reason,
            partial,
        };
        let (Some(snapshot_id), Some(sha)) = (snapshot_id, sha) else {
            out.untestable
                .push(untestable("G1 never indexed".into(), false));
            continue;
        };
        if provider == "itu" {
            out.untestable
                .push(untestable("unsupported: ITU PDF source".into(), false));
            continue;
        }
        let hex = sha.trim_start_matches("hash:").to_string();
        if snap_v.as_deref() != Some(INDEX_VERSION) || uc_v.as_deref() != Some(INDEX_VERSION) {
            out.untestable.push(untestable(
                format!(
                    "G3 version mismatch: snapshot {} / update_checks {} (harness {INDEX_VERSION})",
                    snap_v.as_deref().unwrap_or("-"),
                    uc_v.as_deref().unwrap_or("-")
                ),
                true,
            ));
            continue;
        }
        if uc_hash.as_deref() != Some(hex.as_str()) {
            out.untestable.push(untestable(
                "G2 update_checks.content_hash differs from snapshot sha".into(),
                true,
            ));
            continue;
        }
        let cache_path = webspec_index::fetch::html_cache_path(&db_dir.join("html"), &name, &sha);
        if !cache_path.exists() {
            out.untestable
                .push(untestable("G2 cache file missing".into(), true));
            continue;
        }
        out.testable.push(SpecEntry {
            name,
            base_url,
            provider,
            snapshot_id,
            sha,
            cache_path,
            has_structure: structures > 0,
        });
    }
    Ok(out)
}

/// Read and verify the cached HTML (G2).
pub fn read_cache(entry: &SpecEntry) -> Result<String> {
    let bytes = std::fs::read(&entry.cache_path)?;
    let hex = entry.sha.trim_start_matches("hash:");
    if sha256_hex(&bytes) != hex {
        bail!("G2 cache file hash differs from snapshot sha");
    }
    Ok(String::from_utf8(bytes)?)
}

pub fn current_sha(conn: &Connection, spec: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT s.sha FROM snapshots s JOIN specs sp ON s.spec_id = sp.id
             WHERE sp.name = ?1 AND s.pr_number IS NULL AND s.sha LIKE 'hash:%'",
            [spec],
            |r| r.get(0),
        )
        .optional()?)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefRow {
    pub from_anchor: String,
    pub to_spec: String,
    pub to_anchor: String,
    pub step_path: Option<String>,
    pub step_text: Option<String>,
    pub guard_path: Option<String>,
    pub call_site_id: Option<String>,
    pub kind: Option<String>,
}

impl RefRow {
    pub fn from_parsed(r: &ParsedReference) -> Self {
        RefRow {
            from_anchor: r.from_anchor.clone(),
            to_spec: r.to_spec.clone(),
            to_anchor: r.to_anchor.clone(),
            step_path: r.step_path.clone(),
            step_text: r.step_text.clone(),
            guard_path: webspec_index::db::encode_guard_path(&r.guard_path),
            call_site_id: r.call_site_id.clone(),
            kind: Some(r.kind.as_str().to_string()),
        }
    }
}

/// The stored rows D1 compares against.
pub struct Stored {
    pub sections: Vec<ParsedSection>,
    pub refs: Vec<RefRow>,
    pub structure_json: Option<String>,
}

pub fn load_stored(conn: &Connection, snapshot_id: i64) -> Result<Stored> {
    let mut stmt = conn.prepare(
        "SELECT anchor, title, content_text, section_type, parent_anchor, prev_anchor, next_anchor, depth, number
         FROM sections WHERE snapshot_id = ?1 ORDER BY id",
    )?;
    let sections = stmt
        .query_map([snapshot_id], |r| {
            let ty: String = r.get(3)?;
            Ok(ParsedSection {
                anchor: r.get(0)?,
                title: r.get(1)?,
                content_text: r.get(2)?,
                section_type: ty.parse::<SectionType>().unwrap_or(SectionType::Definition),
                parent_anchor: r.get(4)?,
                prev_anchor: r.get(5)?,
                next_anchor: r.get(6)?,
                depth: r.get(7)?,
                number: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(
        "SELECT from_anchor, to_spec, to_anchor, step_path, step_text, guard_path, call_site_id, kind
         FROM refs WHERE snapshot_id = ?1",
    )?;
    let refs = stmt
        .query_map([snapshot_id], |r| {
            Ok(RefRow {
                from_anchor: r.get(0)?,
                to_spec: r.get(1)?,
                to_anchor: r.get(2)?,
                step_path: r.get(3)?,
                step_text: r.get(4)?,
                guard_path: r.get(5)?,
                call_site_id: r.get(6)?,
                kind: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let structure_json =
        webspec_index::db::effects::load_structure(conn, snapshot_id, STRUCTURE_VERSION)?;
    Ok(Stored {
        sections,
        refs,
        structure_json,
    })
}
