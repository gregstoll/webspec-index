//! Per-snapshot state model rows (spec §9.2).
use crate::state::{derive_occurrence_counts, derive_sites, Owner, StateSpec};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Every per-snapshot state table (spec §9.2). The purge, child-delete and
/// export-prune lists iterate this; none of these tables reference each other.
pub const STATE_TABLES: [&str; 9] = [
    "state_models",
    "state_types",
    "state_type_edges",
    "state_fields",
    "state_field_owners",
    "state_members",
    "state_sites",
    "state_occurrence_counts",
    "state_coverage",
];

/// Create state tables and indexes if they do not yet exist (spec §9.2, §16.1).
/// Called from `run_migrations` alongside `effects::initialize`.
pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS state_models (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            representation_version TEXT NOT NULL,
            payload BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS state_types (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            type_key TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            anchor TEXT NOT NULL,
            role TEXT NOT NULL,
            PRIMARY KEY (snapshot_id, type_key, anchor)
        );
        CREATE INDEX IF NOT EXISTS idx_state_types_name ON state_types(name);
        CREATE TABLE IF NOT EXISTS state_type_edges (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            sub_key TEXT NOT NULL,
            super_key TEXT NOT NULL,
            basis TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS state_fields (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            anchor TEXT NOT NULL,
            name TEXT NOT NULL,
            names_json TEXT NOT NULL,
            owners_json TEXT NOT NULL,
            owner_basis TEXT,
            field_basis TEXT NOT NULL,
            type_json TEXT NOT NULL,
            initial_json TEXT,
            decl_section TEXT,
            decl_text TEXT,
            issues_json TEXT NOT NULL DEFAULT '[]',
            PRIMARY KEY (snapshot_id, anchor)
        );
        CREATE TABLE IF NOT EXISTS state_field_owners (
            snapshot_id INTEGER NOT NULL,
            anchor TEXT NOT NULL,
            owner_key TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_state_field_owners ON state_field_owners(owner_key);
        CREATE TABLE IF NOT EXISTS state_members (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            anchor TEXT NOT NULL,
            name TEXT NOT NULL,
            names_json TEXT NOT NULL,
            set_key TEXT NOT NULL,
            decl_section TEXT NOT NULL,
            decl_text TEXT NOT NULL,
            PRIMARY KEY (snapshot_id, anchor)
        );
        CREATE TABLE IF NOT EXISTS state_sites (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            site_id TEXT NOT NULL,
            class TEXT NOT NULL,
            target_spec TEXT,
            target_anchor TEXT,
            op TEXT NOT NULL,
            subject_anchor TEXT NOT NULL,
            context TEXT NOT NULL,
            role TEXT,
            constructed TEXT,
            step_path TEXT,
            step_id TEXT,
            receiver TEXT NOT NULL,
            target_text TEXT NOT NULL,
            value_text TEXT,
            text TEXT NOT NULL,
            basis TEXT NOT NULL,
            segment_id TEXT,
            body_id TEXT,
            span_start INTEGER,
            span_end INTEGER,
            PRIMARY KEY (snapshot_id, site_id)
        );
        CREATE INDEX IF NOT EXISTS idx_state_sites_snapshot ON state_sites(snapshot_id);
        CREATE INDEX IF NOT EXISTS idx_state_sites_target ON state_sites(target_spec, target_anchor);
        CREATE INDEX IF NOT EXISTS idx_state_sites_subject ON state_sites(snapshot_id, subject_anchor);
        CREATE TABLE IF NOT EXISTS state_occurrence_counts (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            target_spec TEXT NOT NULL,
            target_anchor TEXT NOT NULL,
            class TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (snapshot_id, target_spec, target_anchor, class)
        );
        CREATE TABLE IF NOT EXISTS state_coverage (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            coverage_json TEXT NOT NULL
        );",
    )?;

    // §16.1: add amendment columns to state_sites in case the table was
    // created by an earlier build without them (no-op on a fresh database).
    for (col, ty) in [
        ("segment_id", "TEXT"),
        ("body_id", "TEXT"),
        ("span_start", "INTEGER"),
        ("span_end", "INTEGER"),
    ] {
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('state_sites') WHERE name=?1",
            [col],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if !exists {
            conn.execute(
                &format!("ALTER TABLE state_sites ADD COLUMN {col} {ty}"),
                [],
            )?;
        }
    }

    Ok(())
}

/// Check whether a state model for `snapshot_id` with the given
/// `representation_version` already exists (spec §9.3).
pub fn has_state_model(
    conn: &Connection,
    snapshot_id: i64,
    representation_version: &str,
) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM state_models \
         WHERE snapshot_id=?1 AND representation_version=?2)",
        params![snapshot_id, representation_version],
        |r| r.get(0),
    )?)
}

/// Load and deserialize the stored state model for `snapshot_id`.
/// Returns `None` when no model row exists.
pub fn load_state_model(conn: &Connection, snapshot_id: i64) -> Result<Option<StateSpec>> {
    let result: Option<String> = conn
        .query_row(
            "SELECT payload FROM state_models WHERE snapshot_id=?1",
            params![snapshot_id],
            |r| {
                crate::db::effects::decode_payload(r.get_ref(0)?)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            },
        )
        .optional()?;
    Ok(result.map(|json| serde_json::from_str(&json)).transpose()?)
}

/// Delete and re-insert every state row for `snapshot_id` (spec §9.3).
/// Idempotent: calling twice yields the same rows as calling once.
pub fn store_state(conn: &Connection, snapshot_id: i64, state: &StateSpec) -> Result<()> {
    // Delete old rows for this snapshot from every state table.
    for table in STATE_TABLES {
        conn.execute(
            &format!("DELETE FROM {table} WHERE snapshot_id=?1"),
            params![snapshot_id],
        )?;
    }

    // state_models — full payload, deflate-compressed.
    let json = serde_json::to_string(state)?;
    let payload = crate::db::effects::encode_payload(&json);
    conn.execute(
        "INSERT INTO state_models(snapshot_id, representation_version, payload) \
         VALUES (?1, ?2, ?3)",
        params![snapshot_id, &state.representation_version, payload],
    )?;

    // state_types — one row per anchor; a type with no anchors gets one row
    // with anchor='' and role='none' so name lookups still find it.
    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO state_types(snapshot_id, type_key, name, kind, anchor, role) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for ty in &state.model.types {
            let key = ty.key.to_string();
            let kind = serde_json::to_value(&ty.kind)?
                .as_str()
                .unwrap_or("")
                .to_string();
            if ty.anchors.is_empty() {
                stmt.execute(params![snapshot_id, key, ty.name, kind, "", "none"])?;
            } else {
                for anchor in &ty.anchors {
                    let role = anchor_role_str(&anchor.role);
                    let anchor_str = format!("{}#{}", anchor.target.spec, anchor.target.anchor);
                    stmt.execute(params![snapshot_id, key, ty.name, kind, anchor_str, role])?;
                }
            }
        }
    }

    // state_type_edges
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO state_type_edges(snapshot_id, sub_key, super_key, basis) \
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for ty in &state.model.types {
            let sub_key = ty.key.to_string();
            for edge in &ty.supertypes {
                let basis = super_basis_str(&edge.basis);
                stmt.execute(params![
                    snapshot_id,
                    sub_key,
                    edge.target.to_string(),
                    basis
                ])?;
            }
        }
    }

    // state_fields + state_field_owners
    {
        let mut field_stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO state_fields \
             (snapshot_id, anchor, name, names_json, owners_json, owner_basis, \
              field_basis, type_json, initial_json, decl_section, decl_text, issues_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        let mut owner_stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO state_field_owners(snapshot_id, anchor, owner_key) \
             VALUES (?1, ?2, ?3)",
        )?;
        for field in &state.model.fields {
            let anchor = format!("{}#{}", field.anchor.spec, field.anchor.anchor);
            let (owners_json, owner_basis) = match &field.owner {
                Owner::Known { types, basis } => (
                    serde_json::to_string(types)?,
                    Some(owner_basis_str(basis).to_string()),
                ),
                Owner::Unknown { hint } => (
                    serde_json::to_string(&serde_json::json!({"unknown": hint}))?,
                    None,
                ),
            };
            let field_basis = serde_json::to_value(&field.field_basis)?
                .as_str()
                .unwrap_or("")
                .to_string();
            let type_json = serde_json::to_string(&field.declared_type)?;
            let initial_json = field
                .initial
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let (decl_section, decl_text) = match &field.declaration {
                Some(d) => (Some(d.section_anchor.as_str()), Some(d.text.as_str())),
                None => (None, None),
            };
            // Collect issue codes for this field (by anchor match).
            let issue_codes: Vec<String> = state
                .issues
                .iter()
                .filter(|issue| issue.anchor.as_deref() == Some(&field.anchor.anchor))
                .map(|issue| {
                    serde_json::to_value(issue.code)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                })
                .collect();
            let issues_json = serde_json::to_string(&issue_codes)?;
            let names_json = serde_json::to_string(&field.names)?;
            field_stmt.execute(params![
                snapshot_id,
                anchor,
                field.name,
                names_json,
                owners_json,
                owner_basis,
                field_basis,
                type_json,
                initial_json,
                decl_section,
                decl_text,
                issues_json
            ])?;
            // state_field_owners
            if let Owner::Known { types, .. } = &field.owner {
                for owner_ref in types {
                    owner_stmt.execute(params![snapshot_id, anchor, owner_ref.key.to_string()])?;
                }
            }
        }
    }

    // state_members
    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO state_members \
             (snapshot_id, anchor, name, names_json, set_key, decl_section, decl_text) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for member in &state.model.members {
            let anchor = format!("{}#{}", member.anchor.spec, member.anchor.anchor);
            let names_json = serde_json::to_string(&member.names)?;
            stmt.execute(params![
                snapshot_id,
                anchor,
                member.name,
                names_json,
                member.set.to_string(),
                member.declaration.section_anchor,
                member.declaration.text
            ])?;
        }
    }

    // state_sites — from derive_sites (A12).
    // After A12 lands this calls crate::state::derive_sites(state).
    insert_sites(conn, snapshot_id, state)?;

    // state_occurrence_counts — from derive_occurrence_counts (A12).
    insert_occurrence_counts(conn, snapshot_id, state)?;

    // state_coverage
    conn.execute(
        "INSERT OR REPLACE INTO state_coverage(snapshot_id, coverage_json) VALUES (?1, ?2)",
        params![snapshot_id, serde_json::to_string(&state.coverage)?],
    )?;

    Ok(())
}

/// Insert state_sites rows.
/// Wired up to `crate::state::derive_sites` after A12 lands.
fn insert_sites(conn: &Connection, snapshot_id: i64, state: &StateSpec) -> Result<()> {
    insert_sites_impl(conn, snapshot_id, state)
}

/// Insert state_occurrence_counts rows.
/// Wired up to `crate::state::derive_occurrence_counts` after A12 lands.
fn insert_occurrence_counts(conn: &Connection, snapshot_id: i64, state: &StateSpec) -> Result<()> {
    insert_occurrence_counts_impl(conn, snapshot_id, state)
}

fn insert_sites_impl(conn: &Connection, snapshot_id: i64, state: &StateSpec) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO state_sites \
         (snapshot_id, site_id, class, target_spec, target_anchor, op, \
          subject_anchor, context, role, constructed, step_path, step_id, \
          receiver, target_text, value_text, text, basis, \
          segment_id, body_id, span_start, span_end) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
    )?;
    for site in derive_sites(state) {
        stmt.execute(params![
            snapshot_id,
            site.site_id,
            site.class.as_str(),
            site.target.as_ref().map(|t| t.spec.as_str()),
            site.target.as_ref().map(|t| t.anchor.as_str()),
            site.op,
            format!("{}#{}", site.subject.spec, site.subject.anchor),
            site.context,
            site.role,
            site.constructed,
            site.step_path,
            site.step_id,
            site.receiver,
            site.target_text,
            site.value_text,
            site.text,
            site.basis,
            site.segment_id,
            site.body_id,
            site.span_start.map(|v| v as i64),
            site.span_end.map(|v| v as i64),
        ])?;
    }
    Ok(())
}

fn insert_occurrence_counts_impl(
    conn: &Connection,
    snapshot_id: i64,
    state: &StateSpec,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO state_occurrence_counts \
         (snapshot_id, target_spec, target_anchor, class, count) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (target, class, count) in derive_occurrence_counts(state) {
        stmt.execute(params![
            snapshot_id,
            target.spec,
            target.anchor,
            class,
            count as i64,
        ])?;
    }
    Ok(())
}

fn anchor_role_str(role: &crate::state::AnchorRole) -> &'static str {
    use crate::state::{AnchorRole, ConceptAliasBasis};
    match role {
        AnchorRole::Defining => "defining",
        AnchorRole::Partial => "partial",
        AnchorRole::ConceptAlias(ConceptAliasBasis::NameMatch) => "concept_alias:name_match",
        AnchorRole::ConceptAlias(ConceptAliasBasis::Evidence) => "concept_alias:evidence",
        AnchorRole::ElementDefinition => "element_definition",
    }
}

fn super_basis_str(basis: &crate::state::SuperBasis) -> String {
    use crate::state::SuperBasis;
    match basis {
        SuperBasis::IdlInheritance => "idl_inheritance".to_string(),
        SuperBasis::IdlIncludes => "idl_includes".to_string(),
        SuperBasis::Override { rule_id } => format!("override:{rule_id}"),
    }
}

fn owner_basis_str(basis: &crate::state::OwnerBasis) -> &'static str {
    use crate::state::OwnerBasis;
    match basis {
        OwnerBasis::DfnFor => "dfn_for",
        OwnerBasis::DeclarationSentence => "declaration_sentence",
        OwnerBasis::PropertyList => "property_list",
        OwnerBasis::StructItems => "struct_items",
        OwnerBasis::Override { .. } => "override",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::initialize_schema(&conn).unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn initialize_creates_all_state_tables() {
        let conn = conn();
        for table in STATE_TABLES {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} missing");
        }
    }

    #[test]
    fn initialize_is_idempotent() {
        let conn = conn();
        // Calling initialize twice must not fail.
        initialize(&conn).unwrap();
    }

    #[test]
    fn has_state_model_returns_false_when_no_row() {
        let conn = conn();
        let spec_id = crate::db::write::insert_or_get_spec(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
        )
        .unwrap();
        let snapshot =
            crate::db::write::insert_snapshot(&conn, spec_id, "hash:t", "2026-09-25").unwrap();
        assert!(!has_state_model(&conn, snapshot, "1").unwrap());
    }

    #[test]
    fn load_state_model_returns_none_when_no_row() {
        let conn = conn();
        let spec_id = crate::db::write::insert_or_get_spec(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
        )
        .unwrap();
        let snapshot =
            crate::db::write::insert_snapshot(&conn, spec_id, "hash:t", "2026-09-25").unwrap();
        assert!(load_state_model(&conn, snapshot).unwrap().is_none());
    }

    #[test]
    fn store_and_reload_round_trips_and_version_gate_works() {
        let conn = conn();
        let spec_id = crate::db::write::insert_or_get_spec(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
        )
        .unwrap();
        let snapshot =
            crate::db::write::insert_snapshot(&conn, spec_id, "hash:t", "2026-09-25").unwrap();
        let state = crate::state::testing::extract_html(crate::state::testing::MINI, "HTML");
        store_state(&conn, snapshot, &state).unwrap();
        assert!(has_state_model(&conn, snapshot, crate::state::STATE_VERSION).unwrap());
        assert!(!has_state_model(&conn, snapshot, "0").unwrap());
        assert_eq!(load_state_model(&conn, snapshot).unwrap().unwrap(), state);
        let sites: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM state_sites WHERE snapshot_id=?1",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sites >= 3);
        // Second store must replace, never duplicate.
        store_state(&conn, snapshot, &state).unwrap();
        let again: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM state_sites WHERE snapshot_id=?1",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sites, again, "store replaces, never duplicates");
    }
}
