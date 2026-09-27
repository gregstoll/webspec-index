//! Per-snapshot state model rows (spec §9.2).
use crate::state::slice::SliceIndex;
use crate::state::{derive_occurrence_counts, derive_sites, Owner, StateSpec};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Every per-snapshot state table (spec §9.2). The purge, child-delete and
/// export-prune lists iterate this; none of these tables reference each other.
pub const STATE_TABLES: [&str; 10] = [
    "state_models",
    "state_types",
    "state_type_edges",
    "state_fields",
    "state_field_owners",
    "state_members",
    "state_sites",
    "state_occurrence_counts",
    "state_coverage",
    "state_slices",
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
        CREATE INDEX IF NOT EXISTS idx_state_counts_target ON state_occurrence_counts(target_spec, target_anchor);
        CREATE INDEX IF NOT EXISTS idx_state_fields_anchor ON state_fields(anchor);
        CREATE INDEX IF NOT EXISTS idx_state_types_anchor ON state_types(anchor);
        CREATE INDEX IF NOT EXISTS idx_state_members_set ON state_members(set_key);
        CREATE INDEX IF NOT EXISTS idx_state_sites_opaque ON state_sites(snapshot_id) WHERE class = 'opaque_write';
        CREATE TABLE IF NOT EXISTS state_coverage (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            coverage_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS state_slices (snapshot_id INTEGER NOT NULL REFERENCES snapshots(id), anchor TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY (snapshot_id, anchor));",
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

/// Replace the slice rows of `snapshot_id` (spec §10). Call after `store_state`, which clears
/// every `STATE_TABLES` row of the snapshot.
pub fn store_slice_indexes(
    conn: &Connection,
    snapshot_id: i64,
    indexes: &[SliceIndex],
) -> Result<()> {
    conn.execute(
        "DELETE FROM state_slices WHERE snapshot_id=?1",
        params![snapshot_id],
    )?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO state_slices(snapshot_id, anchor, payload) VALUES (?1, ?2, ?3)",
    )?;
    for index in indexes {
        stmt.execute(params![
            snapshot_id,
            &index.anchor,
            serde_json::to_string(index)?
        ])?;
    }
    Ok(())
}

/// The slice index of the algorithm at `anchor` in `snapshot_id`, if one is stored.
pub fn load_slice_index(
    conn: &Connection,
    snapshot_id: i64,
    anchor: &str,
) -> Result<Option<SliceIndex>> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM state_slices WHERE snapshot_id=?1 AND anchor=?2",
            params![snapshot_id, anchor],
            |r| r.get(0),
        )
        .optional()?;
    Ok(payload.map(|p| serde_json::from_str(&p)).transpose()?)
}

/// Whether `snapshot_id` has any slice rows.
pub fn has_slice_rows(conn: &Connection, snapshot_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM state_slices WHERE snapshot_id=?1)",
        params![snapshot_id],
        |r| r.get(0),
    )?)
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
                Owner::Known { types, basis } => {
                    (serde_json::to_string(types)?, Some(owner_basis_str(basis)))
                }
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

fn owner_basis_str(basis: &crate::state::OwnerBasis) -> String {
    use crate::state::OwnerBasis;
    match basis {
        OwnerBasis::DfnFor => "dfn_for".to_string(),
        OwnerBasis::DeclarationSentence => "declaration_sentence".to_string(),
        OwnerBasis::PropertyList => "property_list".to_string(),
        OwnerBasis::StructItems => "struct_items".to_string(),
        OwnerBasis::Override { rule_id } => format!("override:{rule_id}"),
    }
}

/// A current snapshot that has a state model.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub id: i64,
    pub spec: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct StoredType {
    pub snapshot_id: i64,
    pub type_key: String,
    pub name: String,
    pub kind: String,
    pub anchor: String,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct StoredEdge {
    pub snapshot_id: i64,
    pub sub_key: String,
    pub super_key: String,
    pub basis: String,
}

#[derive(Debug, Clone)]
pub struct StoredField {
    pub snapshot_id: i64,
    pub anchor: String,
    pub name: String,
    pub names_json: String,
    pub owners_json: String,
    pub owner_basis: Option<String>,
    pub field_basis: String,
    pub type_json: String,
    pub initial_json: Option<String>,
    pub decl_section: Option<String>,
    pub decl_text: Option<String>,
    pub issues_json: String,
}

#[derive(Debug, Clone)]
pub struct StoredMember {
    pub snapshot_id: i64,
    pub anchor: String,
    pub name: String,
    pub set_key: String,
    pub decl_section: String,
    pub decl_text: String,
}

/// A `state_sites` row with the name of the spec whose snapshot stored it.
#[derive(Debug, Clone)]
pub struct StoredSite {
    pub spec: String,
    pub class: String,
    pub target_spec: Option<String>,
    pub target_anchor: Option<String>,
    pub op: String,
    pub subject_anchor: String,
    pub context: String,
    pub role: Option<String>,
    pub constructed: Option<String>,
    pub step_path: Option<String>,
    pub step_id: Option<String>,
    pub receiver: String,
    pub target_text: String,
    pub value_text: Option<String>,
    pub text: String,
    pub basis: String,
}

fn id_list(ids: &[i64]) -> String {
    if ids.is_empty() {
        return "NULL".to_string();
    }
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

fn placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Current (non-PR, content-hash) snapshots that have a state model.
pub fn state_snapshots(conn: &Connection) -> Result<Vec<StateSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, sp.name, sp.base_url FROM snapshots s JOIN specs sp ON sp.id = s.spec_id \
         JOIN state_coverage c ON c.snapshot_id = s.id \
         WHERE s.pr_number IS NULL AND s.sha LIKE 'hash:%' ORDER BY sp.name, s.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(StateSnapshot {
            id: row.get(0)?,
            spec: row.get(1)?,
            base_url: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether `spec` has a current (non-PR, content-hash) snapshot, with or without a state model.
pub fn has_current_snapshot(conn: &Connection, spec: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots s JOIN specs sp ON sp.id = s.spec_id \
         WHERE sp.name = ?1 AND s.pr_number IS NULL AND s.sha LIKE 'hash:%')",
        [spec],
        |row| row.get(0),
    )?)
}

pub fn spec_base_url(conn: &Connection, spec: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT base_url FROM specs WHERE name = ?1",
            [spec],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn load_types(conn: &Connection, snapshots: &[i64]) -> Result<Vec<StoredType>> {
    let sql = format!(
        "SELECT snapshot_id, type_key, name, kind, anchor, role FROM state_types \
         WHERE snapshot_id IN ({}) ORDER BY snapshot_id, type_key, anchor",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(StoredType {
            snapshot_id: row.get(0)?,
            type_key: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            anchor: row.get(4)?,
            role: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn load_edges(conn: &Connection, snapshots: &[i64]) -> Result<Vec<StoredEdge>> {
    let sql = format!(
        "SELECT snapshot_id, sub_key, super_key, basis FROM state_type_edges \
         WHERE snapshot_id IN ({}) ORDER BY snapshot_id, rowid",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(StoredEdge {
            snapshot_id: row.get(0)?,
            sub_key: row.get(1)?,
            super_key: row.get(2)?,
            basis: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const FIELD_COLUMNS: &str = "f.snapshot_id, f.anchor, f.name, f.names_json, f.owners_json, \
    f.owner_basis, f.field_basis, f.type_json, f.initial_json, f.decl_section, f.decl_text, f.issues_json";
const FIELD_COLUMN_COUNT: usize = 12;

fn stored_field(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredField> {
    Ok(StoredField {
        snapshot_id: row.get(0)?,
        anchor: row.get(1)?,
        name: row.get(2)?,
        names_json: row.get(3)?,
        owners_json: row.get(4)?,
        owner_basis: row.get(5)?,
        field_basis: row.get(6)?,
        type_json: row.get(7)?,
        initial_json: row.get(8)?,
        decl_section: row.get(9)?,
        decl_text: row.get(10)?,
        issues_json: row.get(11)?,
    })
}

/// The field `spec#anchor` (stored anchors are `SPEC#anchor`).
pub fn fields_by_anchor(
    conn: &Connection,
    snapshots: &[i64],
    spec: &str,
    anchor: &str,
) -> Result<Vec<StoredField>> {
    let sql = format!(
        "SELECT {FIELD_COLUMNS} FROM state_fields f WHERE f.anchor = ?1 AND +f.snapshot_id IN ({}) \
         ORDER BY f.snapshot_id",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([format!("{spec}#{anchor}")], stored_field)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Fields owned by any of `owner_keys`, each paired with the owner key that matched.
pub fn fields_by_owner_keys(
    conn: &Connection,
    snapshots: &[i64],
    owner_keys: &[String],
) -> Result<Vec<(String, StoredField)>> {
    let mut out = Vec::new();
    for chunk in owner_keys.chunks(500) {
        let sql = format!(
            "SELECT {FIELD_COLUMNS}, o.owner_key FROM state_field_owners o \
             JOIN state_fields f ON f.snapshot_id = o.snapshot_id AND f.anchor = o.anchor \
             WHERE o.owner_key IN ({}) AND +o.snapshot_id IN ({}) ORDER BY f.snapshot_id, f.anchor",
            placeholders(1, chunk.len()),
            id_list(snapshots)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
            Ok((row.get(FIELD_COLUMN_COUNT)?, stored_field(row)?))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// Fields whose anchor (without its `SPEC#` prefix) or name matches the SQL
/// `LIKE` pattern (ASCII case-insensitive).
pub fn fields_like(
    conn: &Connection,
    snapshots: &[i64],
    pattern: &str,
) -> Result<Vec<StoredField>> {
    let sql = format!(
        "SELECT {FIELD_COLUMNS} FROM state_fields f \
         WHERE (substr(f.anchor, instr(f.anchor, '#') + 1) LIKE ?1 OR f.name LIKE ?1) \
         AND f.snapshot_id IN ({}) \
         ORDER BY f.snapshot_id, f.name, f.anchor",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([pattern], stored_field)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const MEMBER_COLUMNS: &str = "snapshot_id, anchor, name, set_key, decl_section, decl_text";

fn stored_member(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMember> {
    Ok(StoredMember {
        snapshot_id: row.get(0)?,
        anchor: row.get(1)?,
        name: row.get(2)?,
        set_key: row.get(3)?,
        decl_section: row.get(4)?,
        decl_text: row.get(5)?,
    })
}

/// The set member `spec#anchor` (stored anchors are `SPEC#anchor`).
pub fn members_by_anchor(
    conn: &Connection,
    snapshots: &[i64],
    spec: &str,
    anchor: &str,
) -> Result<Vec<StoredMember>> {
    let sql = format!(
        "SELECT {MEMBER_COLUMNS} FROM state_members WHERE anchor = ?1 AND snapshot_id IN ({}) \
         ORDER BY snapshot_id",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([format!("{spec}#{anchor}")], stored_member)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn members_by_set_keys(
    conn: &Connection,
    snapshots: &[i64],
    set_keys: &[String],
) -> Result<Vec<StoredMember>> {
    let mut out = Vec::new();
    for chunk in set_keys.chunks(500) {
        let sql = format!(
            "SELECT {MEMBER_COLUMNS} FROM state_members WHERE set_key IN ({}) \
             AND +snapshot_id IN ({}) ORDER BY snapshot_id, name, anchor",
            placeholders(1, chunk.len()),
            id_list(snapshots)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), stored_member)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

const SITE_SELECT: &str = "SELECT sp.name, st.class, st.target_spec, st.target_anchor, st.op, \
    st.subject_anchor, st.context, st.role, st.constructed, st.step_path, st.step_id, st.receiver, \
    st.target_text, st.value_text, st.text, st.basis FROM state_sites st \
    JOIN snapshots s ON s.id = st.snapshot_id JOIN specs sp ON sp.id = s.spec_id";

fn stored_site(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSite> {
    Ok(StoredSite {
        spec: row.get(0)?,
        class: row.get(1)?,
        target_spec: row.get(2)?,
        target_anchor: row.get(3)?,
        op: row.get(4)?,
        subject_anchor: row.get(5)?,
        context: row.get(6)?,
        role: row.get(7)?,
        constructed: row.get(8)?,
        step_path: row.get(9)?,
        step_id: row.get(10)?,
        receiver: row.get(11)?,
        target_text: row.get(12)?,
        value_text: row.get(13)?,
        text: row.get(14)?,
        basis: row.get(15)?,
    })
}

/// `(?1, ?2), (?3, ?4), …` for `count` target pairs.
fn target_values(count: usize) -> String {
    (0..count)
        .map(|i| format!("(?{}, ?{})", 2 * i + 1, 2 * i + 2))
        .collect::<Vec<_>>()
        .join(",")
}

fn target_params(targets: &[(String, String)]) -> Vec<&str> {
    targets
        .iter()
        .flat_map(|(spec, anchor)| [spec.as_str(), anchor.as_str()])
        .collect()
}

/// Every site targeting `spec#anchor`, ordered by `(spec, subject, step_path)`.
pub fn sites_for_target(
    conn: &Connection,
    snapshots: &[i64],
    spec: &str,
    anchor: &str,
) -> Result<Vec<StoredSite>> {
    sites_for_targets(conn, snapshots, &[(spec.to_string(), anchor.to_string())])
}

/// Every site targeting one of `targets`, ordered by target, then
/// `(spec, subject, step_path)`. The `+` keeps the target index in use whether
/// or not planner statistics exist.
pub fn sites_for_targets(
    conn: &Connection,
    snapshots: &[i64],
    targets: &[(String, String)],
) -> Result<Vec<StoredSite>> {
    let mut out = Vec::new();
    for chunk in targets.chunks(400) {
        let sql = format!(
            "{SITE_SELECT} WHERE (st.target_spec, st.target_anchor) IN (VALUES {}) \
             AND +st.snapshot_id IN ({}) ORDER BY st.target_spec, st.target_anchor, sp.name, \
             st.subject_anchor, st.step_path, st.site_id",
            target_values(chunk.len()),
            id_list(snapshots)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(target_params(chunk)),
            stored_site,
        )?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// The `reflect` sites whose subject is `subject` (`SPEC#anchor`).
pub fn reflect_sites_for_subject(
    conn: &Connection,
    snapshots: &[i64],
    subject: &str,
) -> Result<Vec<StoredSite>> {
    let sql = format!(
        "{SITE_SELECT} WHERE st.snapshot_id IN ({}) AND st.subject_anchor = ?1 \
         AND st.class = 'declared' AND st.op = 'reflect' ORDER BY st.site_id",
        id_list(snapshots)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([subject], stored_site)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Number of sites of one class and op targeting `spec#anchor`.
#[derive(Debug, Clone)]
pub struct SiteCount {
    pub spec: String,
    pub anchor: String,
    pub class: String,
    pub op: String,
    pub count: u32,
}

/// Site counts per `(target_spec, target_anchor, class, op)` for the given targets.
pub fn site_counts_for_targets(
    conn: &Connection,
    snapshots: &[i64],
    targets: &[(String, String)],
) -> Result<Vec<SiteCount>> {
    let mut out = Vec::new();
    for chunk in targets.chunks(400) {
        let sql = format!(
            "SELECT target_spec, target_anchor, class, op, COUNT(*) FROM state_sites \
             WHERE (target_spec, target_anchor) IN (VALUES {}) AND +snapshot_id IN ({}) \
             GROUP BY target_spec, target_anchor, class, op",
            target_values(chunk.len()),
            id_list(snapshots)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(target_params(chunk)), |row| {
            Ok(SiteCount {
                spec: row.get(0)?,
                anchor: row.get(1)?,
                class: row.get(2)?,
                op: row.get(3)?,
                count: row.get(4)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// Total `read` occurrences of `spec#anchor`.
pub fn read_count(conn: &Connection, snapshots: &[i64], spec: &str, anchor: &str) -> Result<u32> {
    let sql = format!(
        "SELECT COALESCE(SUM(count), 0) FROM state_occurrence_counts \
         WHERE target_spec = ?1 AND target_anchor = ?2 AND class = 'read' AND +snapshot_id IN ({})",
        id_list(snapshots)
    );
    Ok(conn.query_row(&sql, [spec, anchor], |row| row.get(0))?)
}

/// `opaque_write` sites whose target text contains one of `names` (ASCII case-insensitive).
pub fn opaque_writes_like(
    conn: &Connection,
    snapshots: &[i64],
    names: &[String],
) -> Result<Vec<StoredSite>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let likes = (1..=names.len())
        .map(|i| format!("st.target_text LIKE ?{i} ESCAPE '\\'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!(
        "{SITE_SELECT} WHERE st.class = 'opaque_write' AND st.snapshot_id IN ({}) AND ({likes}) \
         ORDER BY sp.name, st.subject_anchor, st.step_path, st.site_id",
        id_list(snapshots)
    );
    let patterns: Vec<String> = names
        .iter()
        .map(|name| {
            let escaped = name
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        })
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(patterns), stored_site)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn section_title(conn: &Connection, snapshots: &[i64], anchor: &str) -> Result<Option<String>> {
    let sql = format!(
        "SELECT title FROM sections WHERE anchor = ?1 AND snapshot_id IN ({}) \
         AND title IS NOT NULL LIMIT 1",
        id_list(snapshots)
    );
    Ok(conn
        .query_row(&sql, [anchor], |row| row.get(0))
        .optional()?)
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
    fn slice_rows_round_trip_replace_and_are_deleted_with_the_snapshot() {
        let conn = crate::db::open_in_memory().unwrap();
        let html = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given <var>foo</var>:</p><ol><li><p>Let <var>a</var> be <var>foo</var>.</p></li></ol></div>"##;
        let snapshot = crate::state::testing::index_offline(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            html,
        )
        .unwrap();
        assert!(has_slice_rows(&conn, snapshot).unwrap());
        let index = load_slice_index(&conn, snapshot, "go").unwrap().unwrap();
        assert_eq!(index.vars, ["a", "foo"]);
        assert_eq!(load_slice_index(&conn, snapshot, "nope").unwrap(), None);
        store_slice_indexes(&conn, snapshot, std::slice::from_ref(&index)).unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM state_slices WHERE snapshot_id=?1",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "store replaces, never duplicates");
        assert!(STATE_TABLES.contains(&"state_slices"));
        let spec_id: i64 = conn
            .query_row(
                "SELECT spec_id FROM snapshots WHERE id=?1",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap();
        crate::db::write::delete_spec_data(&conn, spec_id).unwrap();
        assert!(!has_slice_rows(&conn, snapshot).unwrap());
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

    #[test]
    fn algorithm_sites_persist_amendment_columns() {
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

        // Algorithm sites that came from a classified statement must carry
        // non-NULL segment_id, body_id, and a valid byte span.
        let rows: Vec<(String, String, String, i64, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT site_id, segment_id, body_id, span_start, span_end \
                     FROM state_sites \
                     WHERE snapshot_id=?1 AND context='algorithm' \
                       AND segment_id IS NOT NULL AND span_start IS NOT NULL \
                     LIMIT 10",
                )
                .unwrap();
            stmt.query_map([snapshot], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        // MINI has at least one classified algorithm site (the set-to-false step).
        assert!(
            !rows.is_empty(),
            "expected algorithm sites with segment_id and span"
        );
        for (site_id, segment_id, body_id, span_start, span_end) in &rows {
            assert!(
                !segment_id.is_empty(),
                "segment_id must be non-empty for {site_id}"
            );
            assert!(
                !body_id.is_empty(),
                "body_id must be non-empty for {site_id}"
            );
            assert!(
                span_end >= span_start,
                "span_end must be >= span_start for {site_id}"
            );
        }
        // Verify span slices into the segment text stored in state_sites.text.
        // `text` is the full segment text; span_start..span_end is the statement within it.
        let (segment_id, span_start, span_end, text): (String, i64, i64, String) = conn
            .query_row(
                "SELECT segment_id, span_start, span_end, text FROM state_sites \
                 WHERE snapshot_id=?1 AND context='algorithm' AND span_start IS NOT NULL \
                 LIMIT 1",
                [snapshot],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let start = span_start as usize;
        let end = span_end as usize;
        assert!(
            end <= text.len() && !text[start..end].trim().is_empty(),
            "text[span_start..span_end] must be non-empty for segment {segment_id}"
        );
    }

    #[test]
    fn owner_basis_override_round_trips_with_rule_id() {
        use crate::state::model::{
            AnchorTarget, CoverageCounters, FieldBasis, FieldDef, ObjectModel, Owner, OwnerBasis,
            OwnerRef, OwnerVia, StateSpec, TypeExpr,
        };
        let conn = conn();
        let spec_id = crate::db::write::insert_or_get_spec(
            &conn,
            "HTML",
            "https://html.spec.whatwg.org/",
            "whatwg",
        )
        .unwrap();
        let snapshot =
            crate::db::write::insert_snapshot(&conn, spec_id, "hash:ov", "2026-09-25").unwrap();
        let field_anchor = AnchorTarget {
            spec: "HTML".to_string(),
            anchor: "test-field".to_string(),
        };
        let owner_type_key = crate::state::TypeKey::Anchor(AnchorTarget {
            spec: "HTML".to_string(),
            anchor: "some-type".to_string(),
        });
        let state = StateSpec {
            representation_version: crate::state::STATE_VERSION.to_string(),
            spec: "HTML".to_string(),
            snapshot_sha: "hash:ov".to_string(),
            model: ObjectModel {
                types: vec![],
                fields: vec![FieldDef {
                    anchor: field_anchor.clone(),
                    name: "testField".to_string(),
                    names: vec!["testField".to_string()],
                    owner: Owner::Known {
                        types: vec![OwnerRef {
                            key: owner_type_key.clone(),
                            via: OwnerVia::Declaration,
                        }],
                        basis: OwnerBasis::Override {
                            rule_id: "rule42".to_string(),
                        },
                    },
                    field_basis: FieldBasis::Declared,
                    declared_type: TypeExpr::Unknown,
                    initial: None,
                    declaration: None,
                }],
                members: vec![],
                reflections: vec![],
            },
            sources: vec![],
            statements: vec![],
            occurrences: vec![],
            prose_mentions: Default::default(),
            coverage: CoverageCounters::default(),
            issues: vec![],
            declared_sites: vec![],
            ..Default::default()
        };
        store_state(&conn, snapshot, &state).unwrap();
        let owner_basis: Option<String> = conn
            .query_row(
                "SELECT owner_basis FROM state_fields WHERE snapshot_id=?1",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            owner_basis.as_deref(),
            Some("override:rule42"),
            "OwnerBasis::Override must be stored as override:<rule_id>"
        );
    }
}
