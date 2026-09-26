// Re-index a trunk snapshot in place: rewrite only the rows that changed.
use super::write::{self, atomic_write};
use crate::model::{ParsedIdlDefinition, ParsedReference, ParsedSection};
use anyhow::Result;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct SnapshotRows<'a> {
    pub sections: &'a [ParsedSection],
    pub refs: &'a [ParsedReference],
    pub idl: &'a [ParsedIdlDefinition],
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub sections_updated: usize,
    pub sections_inserted: usize,
    pub sections_deleted: usize,
    pub ref_groups_rewritten: usize,
}

/// Update the snapshot row in place (sha, commit_date, indexed_at, index_version) and apply the row diff.
pub fn replace_snapshot_in_place(
    conn: &Connection,
    snapshot_id: i64,
    sha: &str,
    commit_date: &str,
    rows: SnapshotRows<'_>,
) -> Result<DiffStats> {
    atomic_write(conn, |tx| {
        let updated = tx.execute(
            "UPDATE snapshots SET sha=?1, commit_date=?2, indexed_at=?3, index_version=?4 WHERE id=?5",
            (
                sha,
                commit_date,
                chrono::Utc::now().to_rfc3339(),
                crate::parse::INDEX_VERSION,
                snapshot_id,
            ),
        )?;
        anyhow::ensure!(updated == 1, "snapshot {snapshot_id} does not exist");
        let mut stats = DiffStats::default();
        diff_sections(tx, snapshot_id, rows.sections, &mut stats)?;
        diff_refs(tx, snapshot_id, rows.refs, &mut stats)?;
        tx.execute("DELETE FROM idl_defs WHERE snapshot_id=?1", [snapshot_id])?;
        write::insert_idl_defs_bulk(tx, snapshot_id, rows.idl)?;
        Ok(stats)
    })
}

struct StoredSection {
    id: i64,
    title: Option<String>,
    content_text: Option<String>,
    section_type: String,
    parent_anchor: Option<String>,
    prev_anchor: Option<String>,
    next_anchor: Option<String>,
    depth: Option<i64>,
    number: Option<String>,
    ord: Option<i64>,
}

/// Title and content are FTS-indexed, so they are only written when they
/// change; the other columns are rewritten without touching the FTS index.
fn diff_sections(
    tx: &Connection,
    snapshot_id: i64,
    sections: &[ParsedSection],
    stats: &mut DiffStats,
) -> Result<()> {
    let mut stored: HashMap<String, StoredSection> = tx
        .prepare(
            "SELECT anchor, id, title, content_text, section_type, parent_anchor, prev_anchor,
                    next_anchor, depth, number, ord
             FROM sections WHERE snapshot_id=?1",
        )?
        .query_map([snapshot_id], |row| {
            Ok((
                row.get(0)?,
                StoredSection {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    content_text: row.get(3)?,
                    section_type: row.get(4)?,
                    parent_anchor: row.get(5)?,
                    prev_anchor: row.get(6)?,
                    next_anchor: row.get(7)?,
                    depth: row.get(8)?,
                    number: row.get(9)?,
                    ord: row.get(10)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut update_all = tx.prepare(
        "UPDATE sections SET title=?2, content_text=?3, section_type=?4, parent_anchor=?5,
             prev_anchor=?6, next_anchor=?7, depth=?8, number=?9, ord=?10
         WHERE id=?1",
    )?;
    let mut update_structure = tx.prepare(
        "UPDATE sections SET section_type=?2, parent_anchor=?3, prev_anchor=?4, next_anchor=?5,
             depth=?6, number=?7, ord=?8
         WHERE id=?1",
    )?;
    let mut seen = HashSet::new();
    let mut inserts = Vec::new();
    for (ord, section) in sections.iter().enumerate() {
        if !seen.insert(section.anchor.as_str()) {
            continue;
        }
        let ord = ord as i64;
        let depth = section.depth.map(i64::from);
        let section_type = section.section_type.as_str();
        let Some(old) = stored.remove(&section.anchor) else {
            inserts.push((ord, section));
            continue;
        };
        let text_changed = old.title != section.title || old.content_text != section.content_text;
        let structure_changed = old.section_type != section_type
            || old.parent_anchor != section.parent_anchor
            || old.prev_anchor != section.prev_anchor
            || old.next_anchor != section.next_anchor
            || old.depth != depth
            || old.number != section.number
            || old.ord != Some(ord);
        if text_changed {
            update_all.execute((
                old.id,
                &section.title,
                &section.content_text,
                section_type,
                &section.parent_anchor,
                &section.prev_anchor,
                &section.next_anchor,
                depth,
                &section.number,
                ord,
            ))?;
        } else if structure_changed {
            update_structure.execute((
                old.id,
                section_type,
                &section.parent_anchor,
                &section.prev_anchor,
                &section.next_anchor,
                depth,
                &section.number,
                ord,
            ))?;
        } else {
            continue;
        }
        stats.sections_updated += 1;
    }

    let mut delete = tx.prepare("DELETE FROM sections WHERE id=?1")?;
    for old in stored.values() {
        delete.execute([old.id])?;
    }
    stats.sections_deleted = stored.len();

    let mut insert = tx.prepare(write::INSERT_SECTION_SQL)?;
    for (ord, section) in &inserts {
        insert.execute((
            snapshot_id,
            &section.anchor,
            &section.title,
            &section.content_text,
            section.section_type.as_str(),
            &section.parent_anchor,
            &section.prev_anchor,
            &section.next_anchor,
            section.depth,
            &section.number,
            ord,
        ))?;
    }
    stats.sections_inserted = inserts.len();
    Ok(())
}

type StoredRef = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

fn same_ref(old: &StoredRef, new: &ParsedReference, ord: usize) -> bool {
    old.0 == new.to_spec
        && old.1 == new.to_anchor
        && old.2 == new.step_path
        && old.3 == new.step_text
        && old.4 == super::encode_guard_path(&new.guard_path)
        && old.5 == new.call_site_id
        && old.6.as_deref() == Some(new.kind.as_str())
        && old.7 == Some(ord as i64)
}

/// Refs are compared per `from_anchor` group; a group that differs in any row
/// or in order is rewritten whole.
fn diff_refs(
    tx: &Connection,
    snapshot_id: i64,
    refs: &[ParsedReference],
    stats: &mut DiffStats,
) -> Result<()> {
    let mut stored: HashMap<String, Vec<StoredRef>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT from_anchor, to_spec, to_anchor, step_path, step_text, guard_path,
                    call_site_id, kind, ord
             FROM refs WHERE snapshot_id=?1 ORDER BY from_anchor, ord",
        )?;
        let mut rows = stmt.query([snapshot_id])?;
        while let Some(row) = rows.next()? {
            let from_anchor: String = row.get(0)?;
            stored.entry(from_anchor).or_default().push((
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ));
        }
    }

    let mut groups: BTreeMap<&str, Vec<&ParsedReference>> = BTreeMap::new();
    for reference in refs {
        groups
            .entry(reference.from_anchor.as_str())
            .or_default()
            .push(reference);
    }

    let mut delete = tx.prepare("DELETE FROM refs WHERE snapshot_id=?1 AND from_anchor=?2")?;
    let mut insert = tx.prepare(write::INSERT_REF_SQL)?;
    for (from_anchor, group) in &groups {
        let old = stored.remove(*from_anchor);
        if let Some(old) = &old {
            let unchanged = old.len() == group.len()
                && old
                    .iter()
                    .zip(group)
                    .enumerate()
                    .all(|(ord, (old, new))| same_ref(old, new, ord));
            if unchanged {
                continue;
            }
            delete.execute((snapshot_id, from_anchor))?;
        }
        for (ord, reference) in group.iter().enumerate() {
            insert.execute((
                snapshot_id,
                &reference.from_anchor,
                &reference.to_spec,
                &reference.to_anchor,
                &reference.step_path,
                &reference.step_text,
                super::encode_guard_path(&reference.guard_path),
                &reference.call_site_id,
                reference.kind.as_str(),
                ord as i64,
            ))?;
        }
        stats.ref_groups_rewritten += 1;
    }
    for from_anchor in stored.keys() {
        delete.execute((snapshot_id, from_anchor))?;
    }
    stats.ref_groups_rewritten += stored.len();
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{RefKind, SectionType};

    const WORDS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

    struct Rows {
        sections: Vec<ParsedSection>,
        refs: Vec<ParsedReference>,
        idl: Vec<ParsedIdlDefinition>,
    }

    impl Rows {
        fn as_rows(&self) -> SnapshotRows<'_> {
            SnapshotRows {
                sections: &self.sections,
                refs: &self.refs,
                idl: &self.idl,
            }
        }
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn pick(rng: &mut u64, n: usize) -> usize {
        (xorshift(rng) % n as u64) as usize
    }

    fn text(rng: &mut u64) -> String {
        (0..=pick(rng, 3))
            .map(|_| WORDS[pick(rng, WORDS.len())])
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn maybe_text(rng: &mut u64) -> Option<String> {
        (pick(rng, 5) != 0).then(|| text(rng))
    }

    fn maybe_anchor(rng: &mut u64, n: usize) -> Option<String> {
        (pick(rng, 3) != 0).then(|| format!("s{}", pick(rng, n)))
    }

    fn random_section(rng: &mut u64, anchor: String, n: usize) -> ParsedSection {
        const TYPES: [SectionType; 5] = [
            SectionType::Heading,
            SectionType::Algorithm,
            SectionType::Definition,
            SectionType::Idl,
            SectionType::Prose,
        ];
        ParsedSection {
            anchor,
            title: maybe_text(rng),
            content_text: maybe_text(rng),
            section_type: TYPES[pick(rng, TYPES.len())],
            parent_anchor: maybe_anchor(rng, n),
            prev_anchor: maybe_anchor(rng, n),
            next_anchor: maybe_anchor(rng, n),
            depth: (pick(rng, 2) == 0).then(|| 2 + pick(rng, 5) as u8),
            number: (pick(rng, 2) == 0).then(|| format!("{}.{}", pick(rng, 9), pick(rng, 9))),
        }
    }

    fn random_ref(rng: &mut u64, sections: &[ParsedSection]) -> ParsedReference {
        const KINDS: [RefKind; 4] = [RefKind::Step, RefKind::Note, RefKind::Idl, RefKind::Prose];
        ParsedReference {
            from_anchor: sections[pick(rng, sections.len())].anchor.clone(),
            to_spec: ["T", "O"][pick(rng, 2)].to_string(),
            to_anchor: WORDS[pick(rng, WORDS.len())].to_string(),
            step_path: (pick(rng, 2) == 0).then(|| format!("{}", pick(rng, 5))),
            step_text: maybe_text(rng),
            guard_path: (0..pick(rng, 3)).map(|_| text(rng)).collect(),
            call_site_id: (pick(rng, 2) == 0).then(|| format!("c{}", pick(rng, 4))),
            kind: KINDS[pick(rng, KINDS.len())],
        }
    }

    fn random_idl(rng: &mut u64, anchor: String) -> ParsedIdlDefinition {
        ParsedIdlDefinition {
            name: WORDS[pick(rng, WORDS.len())].to_string(),
            owner: (pick(rng, 2) == 0).then(|| "Owner".to_string()),
            kind: ["method", "attribute"][pick(rng, 2)].to_string(),
            canonical_name: format!("Owner.{anchor}"),
            idl_text: maybe_text(rng),
            anchor,
        }
    }

    fn random_rows(rng: &mut u64) -> Rows {
        let n = 5 + pick(rng, 36);
        let sections: Vec<_> = (0..n)
            .map(|i| random_section(rng, format!("s{i}"), n))
            .collect();
        let refs = (0..pick(rng, 3 * n))
            .map(|_| random_ref(rng, &sections))
            .collect();
        let idl = (0..pick(rng, 6))
            .map(|i| random_idl(rng, format!("i{i}")))
            .collect();
        Rows {
            sections,
            refs,
            idl,
        }
    }

    fn mutate(old: &Rows, rng: &mut u64) -> Rows {
        let mut sections = old.sections.clone();
        let mut refs = old.refs.clone();
        let mut idl = old.idl.clone();
        let mut fresh = 0;
        for _ in 0..=pick(rng, 5) {
            let n = sections.len();
            let at = pick(rng, n);
            match pick(rng, 10) {
                0 => sections[at].title = maybe_text(rng),
                1 => sections[at].content_text = maybe_text(rng),
                2 => {
                    fresh += 1;
                    let section = random_section(rng, format!("n{fresh}"), n);
                    sections.insert(pick(rng, n + 1), section);
                }
                3 if n > 1 => {
                    sections.remove(at);
                }
                4 => {
                    fresh += 1;
                    sections[at].anchor = format!("n{fresh}");
                }
                5 => {
                    let section = sections.remove(at);
                    sections.insert(pick(rng, n), section);
                }
                6 => {
                    let reference = random_ref(rng, &sections);
                    refs.insert(pick(rng, refs.len() + 1), reference);
                }
                7 if !refs.is_empty() => {
                    refs.remove(pick(rng, refs.len()));
                }
                8 => match idl.len() {
                    0 => idl.push(random_idl(rng, "i0".to_string())),
                    len => idl[pick(rng, len)].idl_text = maybe_text(rng),
                },
                _ => {
                    sections[at].parent_anchor = maybe_anchor(rng, n);
                    sections[at].depth = None;
                }
            }
        }
        Rows {
            sections,
            refs,
            idl,
        }
    }

    fn seed_snapshot(conn: &Connection, rows: &Rows) -> i64 {
        let spec = write::insert_or_get_spec(conn, "T", "https://t.test/", "test").unwrap();
        let snapshot = write::insert_snapshot(conn, spec, "hash:old", "2026-09-25").unwrap();
        write::insert_sections_bulk(conn, snapshot, &rows.sections).unwrap();
        write::insert_refs_bulk(conn, snapshot, &rows.refs).unwrap();
        write::insert_idl_defs_bulk(conn, snapshot, &rows.idl).unwrap();
        snapshot
    }

    pub(crate) fn logical_rows(conn: &Connection, snapshot: i64) -> Vec<String> {
        let mut out = Vec::new();
        for sql in [
            "SELECT anchor, title, content_text, section_type, parent_anchor, prev_anchor, next_anchor, depth, number, ord FROM sections WHERE snapshot_id=?1 ORDER BY anchor",
            "SELECT from_anchor, ord, to_spec, to_anchor, step_path, step_text, guard_path, call_site_id, kind FROM refs WHERE snapshot_id=?1 ORDER BY from_anchor, ord",
            "SELECT anchor, kind, ord, name, owner, canonical_name, idl_text FROM idl_defs WHERE snapshot_id=?1 ORDER BY anchor, kind",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let n = stmt.column_count();
            let rows = stmt
                .query_map([snapshot], |row| {
                    Ok((0..n)
                        .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap();
            out.extend(rows.map(Result::unwrap));
        }
        out
    }

    fn fts(conn: &Connection, word: &str) -> Vec<String> {
        conn.prepare("SELECT s.anchor FROM sections_fts JOIN sections s ON s.id = sections_fts.rowid WHERE sections_fts MATCH ?1 ORDER BY s.anchor")
            .unwrap()
            .query_map([word], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn applying_the_diff_equals_a_fresh_insert() {
        for seed in 1..=64u64 {
            let mut rng = seed;
            let old = random_rows(&mut rng);
            let new = mutate(&old, &mut rng);
            let a = crate::db::open_test_db().unwrap();
            let snapshot_a = seed_snapshot(&a, &old);
            replace_snapshot_in_place(&a, snapshot_a, "hash:new", "2026-09-26", new.as_rows())
                .unwrap();
            let b = crate::db::open_test_db().unwrap();
            let snapshot_b = seed_snapshot(&b, &new);
            assert_eq!(
                logical_rows(&a, snapshot_a),
                logical_rows(&b, snapshot_b),
                "seed {seed}"
            );
            for word in WORDS {
                assert_eq!(fts(&a, word), fts(&b, word), "seed {seed} word {word}");
            }
            a.execute(
                "INSERT INTO sections_fts(sections_fts) VALUES('integrity-check')",
                [],
            )
            .unwrap();
            let sha: String = a
                .query_row("SELECT sha FROM snapshots WHERE id=?1", [snapshot_a], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(sha, "hash:new");
        }
    }

    #[test]
    fn unchanged_rows_are_left_alone() {
        let mut rng = 7;
        let rows = random_rows(&mut rng);
        let conn = crate::db::open_test_db().unwrap();
        let snapshot = seed_snapshot(&conn, &rows);
        let stats =
            replace_snapshot_in_place(&conn, snapshot, "hash:new", "2026-09-26", rows.as_rows())
                .unwrap();
        assert_eq!(stats, DiffStats::default());
    }
}
