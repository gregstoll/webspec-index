//! Markdown renderings of state query results (§10.2–§10.5).
use crate::state::model::{
    AnchorRole, InfraKind, InitialValue, OwnerBasis, Primitive, StateIssueCode, TypeExpr, TypeKey,
    TypeRef,
};
use crate::state::query::{
    Coverage, FieldListEntry, FieldRow, FoundOn, InheritedFields, MemberRow, OwnerInfo, SiteInfo,
    StateCoverageResult, StateFieldListResult, StateFieldResult, StateMemberResult, StateResponse,
    StateStatus, StateTypeResult,
};
use std::fmt::Write;

pub fn response(response: &StateResponse) -> String {
    match response {
        StateResponse::Field(result) => field(result),
        StateResponse::Type(result) => type_view(result),
        StateResponse::Member(result) => member(result),
        StateResponse::Fields(result) => field_list(result),
    }
}

fn field(result: &StateFieldResult) -> String {
    let info = &result.field;
    let mut out = format!("## {}#{} — {}", info.spec, info.anchor, info.name);
    if let Some(found_on) = found_on_suffix(result.found_on.as_ref()) {
        let _ = write!(out, "  ({found_on})");
    }
    out.push_str("\n\n");

    if result
        .status
        .issues
        .contains(&StateIssueCode::FieldNotDeclared)
    {
        match &result.reflects {
            Some(reflects) => {
                let _ = writeln!(
                    out,
                    "- Not a declared field.\n- Reflects: the `{}` content attribute ({}) — {}",
                    reflects.name,
                    reflects
                        .content_attribute
                        .as_deref()
                        .unwrap_or("no content attribute definition"),
                    reflects.basis
                );
            }
            None => out.push_str("- Not a declared field; sites target this anchor.\n"),
        }
    } else {
        let mut owner = format!(
            "- Owner: {}",
            owners(&info.owners, info.owner_hint.as_deref())
        );
        let basis = info.owner_basis.as_ref().map(basis_label);
        match (&basis, &info.declaration) {
            (Some(basis), Some(decl)) => {
                let _ = write!(owner, " — {basis}, declared in {}:", decl.section);
            }
            (None, Some(decl)) => {
                let _ = write!(owner, " — declared in {}:", decl.section);
            }
            (Some(basis), None) => {
                let _ = write!(owner, " — {basis}");
            }
            (None, None) => {}
        }
        out.push_str(&owner);
        out.push('\n');
        if let Some(decl) = &info.declaration {
            let _ = writeln!(out, "  “{}”", decl.text);
        }
        let _ = writeln!(
            out,
            "- Type: {} · Initial: {}",
            type_expr(&info.declared_type, true),
            initial(info.initial.as_ref(), true)
        );
    }
    out.push('\n');

    let counts = &result.status.counts;
    site_group(&mut out, "Writes", counts.writes, &result.writes);
    if let Some(inits) = &result.inits {
        site_group(&mut out, "Initializations", counts.inits, inits);
    }
    unclassified_group(
        &mut out,
        result.unclassified.count,
        &result.unclassified.items,
    );
    if counts.possible_unlinked > 0 {
        site_group(
            &mut out,
            "Possible unlinked writes",
            counts.possible_unlinked,
            &result.possible_unlinked,
        );
    }
    if counts.declared > 0 {
        site_group(
            &mut out,
            "Declared writes",
            counts.declared,
            &result.declared,
        );
    }

    let _ = writeln!(
        out,
        "Coverage: may · {} — {} unclassified, {} possible unlinked writes. Reads are not listed\n({} occurrences).",
        coverage_label(&result.status),
        counts.unclassified,
        counts.possible_unlinked,
        counts.reads
    );
    out
}

fn member(result: &StateMemberResult) -> String {
    let info = &result.member;
    let set_name = info
        .set_name
        .clone()
        .unwrap_or_else(|| fallback_name(&info.set));
    let mut out = format!("## {}#{} — {}\n\n", info.spec, info.anchor, info.name);
    let _ = writeln!(
        out,
        "- Member of: `{set_name}` ({}), declared in {}:\n  “{}”\n",
        info.set, info.declaration.section, info.declaration.text
    );
    site_group(&mut out, "Adds", result.adds.len() as u32, &result.adds);
    site_group(
        &mut out,
        "Removes",
        result.removes.len() as u32,
        &result.removes,
    );
    unclassified_group(
        &mut out,
        result.unclassified.count,
        &result.unclassified.items,
    );
    let _ = writeln!(
        out,
        "Coverage: may · {} — {} unclassified. Reads are not listed\n({} occurrences).",
        coverage_label(&result.status),
        result.status.counts.unclassified,
        result.status.counts.reads
    );
    out
}

fn type_view(result: &StateTypeResult) -> String {
    let mut out = format!("## {} ({})\n", result.name, result.key);
    if !result.anchors.is_empty() {
        let anchors: Vec<String> = result
            .anchors
            .iter()
            .map(|a| format!("{}#{} ({})", a.spec, a.anchor, role_label(&a.role)))
            .collect();
        let _ = writeln!(out, "Anchors: {}", anchors.join(", "));
    }
    if !result.supertypes.is_empty() {
        let _ = writeln!(out, "Supertypes: {}", result.supertypes.join(" → "));
    }
    if !result.includes.is_empty() {
        let mut by_spec: Vec<(&str, Vec<&str>)> = Vec::new();
        for include in &result.includes {
            match by_spec.iter_mut().find(|(spec, _)| *spec == include.spec) {
                Some((_, names)) => names.push(&include.name),
                None => by_spec.push((&include.spec, vec![&include.name])),
            }
        }
        let groups: Vec<String> = by_spec
            .iter()
            .map(|(spec, names)| format!("{} ({spec})", names.join(", ")))
            .collect();
        let _ = writeln!(out, "Includes: {}", groups.join("; "));
    }

    let truncated = |label: &str| result.truncated.get(label).copied().unwrap_or(0);
    if !result.members.is_empty() {
        out.push_str("\n### Members\n");
        member_table(&mut out, &result.members, truncated("members"));
    }
    if !result.fields.is_empty() || result.members.is_empty() {
        out.push_str("\n### Fields (declared)\n");
        field_table(&mut out, &result.fields, truncated("fields"), false);
    }
    if !result.dfn_for_only.is_empty() {
        out.push_str("\n### Fields with `data-dfn-for` but no recognized declaration\n");
        field_table(
            &mut out,
            &result.dfn_for_only,
            truncated("dfn_for_only"),
            false,
        );
    }
    for InheritedFields {
        from,
        name,
        fields,
        dfn_for_only,
    } in &result.inherited
    {
        if !fields.is_empty() {
            let _ = writeln!(out, "\n### Inherited from {name}");
            field_table(
                &mut out,
                fields,
                truncated(&format!("inherited:{from}")),
                false,
            );
        }
        if !dfn_for_only.is_empty() {
            let _ = writeln!(
                out,
                "\n### Inherited from {name}: `data-dfn-for` but no recognized declaration"
            );
            field_table(
                &mut out,
                dfn_for_only,
                truncated(&format!("inherited_dfn_for_only:{from}")),
                false,
            );
        }
    }
    let _ = writeln!(
        out,
        "\nCoverage: may · {} — {} unclassified in own fields.",
        coverage_label(&result.status),
        result.status.counts.unclassified
    );
    out
}

fn field_list(result: &StateFieldListResult) -> String {
    let mut out = format!("## {} — {} fields\n\n", result.selector, result.total);
    let rows: Vec<FieldRow> = result.fields.iter().map(|e| e.row.clone()).collect();
    if rows.is_empty() {
        out.push_str("(none)\n");
    } else {
        field_table(
            &mut out,
            &rows,
            result.total.saturating_sub(rows.len() as u32),
            true,
        );
    }
    for entry in &result.fields {
        list_entry(&mut out, entry);
    }
    let counts = &result.status.counts;
    let _ = writeln!(
        out,
        "\nCoverage: may · {} — {} unclassified in listed fields.",
        coverage_label(&result.status),
        counts.unclassified
    );
    out
}

fn list_entry(out: &mut String, entry: &FieldListEntry) {
    let row = &entry.row;
    let _ = write!(out, "\n### {}#{} {}", row.spec, row.anchor, row.name);
    if let Some(found_on) = found_on_suffix(entry.found_on.as_ref()) {
        let _ = write!(out, "  ({found_on})");
    }
    out.push('\n');
    if !entry.owners.is_empty() {
        let _ = writeln!(out, "Owner: {}", owners(&entry.owners, None));
    }
    for (label, count, sites) in [
        ("Writes", row.writes, &entry.writes),
        ("Initializations", row.inits, &entry.inits),
        ("Unclassified", row.unclassified, &entry.unclassified),
    ] {
        if sites.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{label} ({count}):");
        for site in sites {
            site_line(out, site);
        }
        more(out, count, sites.len());
    }
}

fn site_group(out: &mut String, label: &str, count: u32, sites: &[SiteInfo]) {
    let _ = writeln!(out, "### {label} ({count})");
    for site in sites {
        site_line(out, site);
    }
    more(out, count, sites.len());
    out.push('\n');
}

fn unclassified_group(out: &mut String, count: u32, items: &[SiteInfo]) {
    if count == 0 {
        return;
    }
    if items.is_empty() {
        let _ = writeln!(
            out,
            "### Unclassified ({count})\n- {count} occurrences; rerun with --unclassified\n"
        );
    } else {
        site_group(out, "Unclassified", count, items);
    }
}

fn more(out: &mut String, count: u32, listed: usize) {
    let hidden = count.saturating_sub(listed as u32);
    if hidden > 0 {
        let _ = writeln!(out, "- … {hidden} more; raise --limit");
    }
}

fn site_line(out: &mut String, site: &SiteInfo) {
    let _ = write!(out, "- {}#{}", site.spec, site.subject);
    let prose = site.context == "prose";
    match (&site.step_path, prose) {
        (Some(path), _) => {
            let _ = write!(out, ":{path}");
        }
        (None, true) => match &site.role {
            Some(role) => {
                let _ = write!(out, " ({role} steps, prose)");
            }
            None => out.push_str(" (prose)"),
        },
        (None, false) => {}
    }
    out.push_str(" — ");
    out.push_str(&site.text);
    if site.receiver == "opaque" {
        out.push_str(" (receiver: opaque)");
    }
    out.push('\n');
    if site.context == "branch_label" {
        if let Some(value) = &site.value {
            let _ = writeln!(out, "  {} → {value}", site.target);
        }
    }
}

fn found_on_suffix(found_on: Option<&FoundOn>) -> Option<String> {
    let found_on = found_on.filter(|f| f.path.len() > 1)?;
    let owner = found_on.path.last()?;
    Some(format!("found on {owner}: {}", found_on.path.join(" → ")))
}

fn owners(owners: &[OwnerInfo], hint: Option<&str>) -> String {
    if owners.is_empty() {
        return match hint {
            Some(hint) => format!("unknown (\"{hint}\")"),
            None => "unknown".to_string(),
        };
    }
    owners
        .iter()
        .map(|owner| {
            let name = owner
                .name
                .clone()
                .unwrap_or_else(|| fallback_name(&owner.r#type));
            if owner.indexed {
                format!("`{name}` ({})", owner.r#type)
            } else {
                format!("`{name}` ({}, spec not indexed)", owner.r#type)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn fallback_name(key: &str) -> String {
    match TypeKey::parse(key) {
        Some(TypeKey::Idl(name)) => name,
        Some(TypeKey::Anchor(target)) => target.anchor,
        None => key.to_string(),
    }
}

fn basis_label(basis: &OwnerBasis) -> String {
    match basis {
        OwnerBasis::DfnFor => "data-dfn-for".into(),
        OwnerBasis::DeclarationSentence => "declaration sentence".into(),
        OwnerBasis::PropertyList => "property list".into(),
        OwnerBasis::StructItems => "struct items".into(),
        OwnerBasis::Override { rule_id } => format!("override {rule_id}"),
    }
}

/// Short label for the raw `FieldRow.basis` column value.
fn basis_short(basis: &str) -> &'static str {
    match basis {
        "dfn_for" => "dfn-for",
        "declaration_sentence" => "sentence",
        "property_list" => "list",
        "struct_items" => "struct",
        b if b.starts_with("override") => "override",
        _ => "—",
    }
}

fn role_label(role: &AnchorRole) -> &'static str {
    match role {
        AnchorRole::Defining => "defining",
        AnchorRole::Partial => "partial",
        AnchorRole::ConceptAlias(_) => "concept alias",
        AnchorRole::ElementDefinition => "element definition",
    }
}

fn coverage_label(status: &StateStatus) -> &'static str {
    match status.coverage {
        Coverage::Complete => "complete",
        Coverage::Partial => "partial",
    }
}

/// Render the §10.5 coverage view for one spec's snapshot.
pub fn coverage(result: &StateCoverageResult) -> String {
    let c = &result.counters;
    let mut out = format!("## State coverage — {}\n\n", result.spec);

    out.push_str("| Rule | Concept dfns |\n|---|---:|\n");
    for (rule, count) in &c.owner_by_rule {
        let _ = writeln!(out, "| {rule} | {count} |");
    }
    out.push('\n');

    out.push_str("| Written fields | Count |\n|---|---:|\n");
    let _ = writeln!(out, "| Total | {} |", c.written_fields);
    for (basis, count) in &c.written_fields_owned {
        let _ = writeln!(out, "| {} | {count} |", basis);
    }
    let resolved = c
        .written_fields
        .saturating_sub(c.written_fields_unresolved.len() as u32);
    if c.written_fields > 0 {
        let pct = resolved as f64 / c.written_fields as f64 * 100.0;
        let _ = writeln!(
            out,
            "\nResolved: {resolved} / {} ({pct:.1}%)",
            c.written_fields
        );
    }
    out.push('\n');

    out.push_str("| Form | Count |\n|---|---:|\n");
    for (form, count) in &c.statements {
        let _ = writeln!(out, "| {form} | {count} |");
    }
    let set_total: u32 = c
        .statements
        .iter()
        .filter(|(k, _)| k.starts_with("set:to:") || k.starts_with("mutate:map_set:"))
        .map(|(_, v)| *v)
        .sum();
    let set_structured: u32 = c
        .statements
        .iter()
        .filter(|(k, _)| {
            (k.starts_with("set:to:field:") || k.starts_with("mutate:map_set:"))
                && !k.contains(":pronoun:")
        })
        .map(|(_, v)| *v)
        .sum();
    let set_pct = if set_total > 0 {
        set_structured as f64 / set_total as f64 * 100.0
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "\nSet statements with a structured target: {set_structured} / {set_total} ({set_pct:.1}%)"
    );
    out.push('\n');

    out.push_str("| Class | Count |\n|---|---:|\n");
    let classified_total: u32 = c.occurrences.values().sum();
    for (class, count) in &c.occurrences {
        let _ = writeln!(out, "| {class} | {count} |");
    }
    let unclassified_n = c.unclassified_review.len() as u32;
    let total_with_unclassified = classified_total + unclassified_n;
    if total_with_unclassified > 0 {
        let pct = unclassified_n as f64 / total_with_unclassified as f64 * 100.0;
        let _ = writeln!(out, "\nUnclassified: {pct:.1}%");
    }
    out.push('\n');

    out.push_str("| Prose | Count |\n|---|---:|\n");
    let _ = writeln!(out, "| Sources | {} |", c.prose_sources);
    let _ = writeln!(out, "| Callouts excluded | {} |", c.prose_callouts_excluded);
    let _ = writeln!(out, "| Mentions | {} |", c.prose_mentions);
    out.push('\n');

    let n = c.unclassified_review.len();
    let _ = writeln!(out, "### Unclassified review ({n})");
    for item in &c.unclassified_review {
        let step = item
            .step_path
            .as_deref()
            .map(|s| format!(":{s}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {}{} — {} — {}",
            item.subject, step, item.target, item.text
        );
    }

    out
}

/// `full` adds the type key to nominal types; tables show the text only.
fn type_expr(ty: &TypeExpr, full: bool) -> String {
    match ty {
        TypeExpr::Unknown => "—".into(),
        TypeExpr::Primitive(primitive) => primitive_word(*primitive).into(),
        TypeExpr::Nominal { ty, text } if full => {
            let key = match ty {
                TypeRef::Known(key) => key.to_string(),
                TypeRef::Unresolved(target) => format!("{}#{}", target.spec, target.anchor),
            };
            format!("{text} ({key})")
        }
        TypeExpr::Nominal { text, .. } => text.clone(),
        TypeExpr::Infra { kind, args } => {
            let kind = infra_word(*kind);
            let args: Vec<String> = args.iter().map(|arg| type_expr(arg, full)).collect();
            match args.as_slice() {
                [] => kind.into(),
                [key, value] if kind.ends_with("map") => format!("{kind} from {key} to {value}"),
                _ => format!("{kind} of {}", args.join(", ")),
            }
        }
        TypeExpr::Union(members) => members
            .iter()
            .map(|member| type_expr(member, full))
            .collect::<Vec<_>>()
            .join(" or "),
        TypeExpr::Null => "null".into(),
        TypeExpr::Enumerated(values) => values
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", "),
        TypeExpr::Opaque { text } => text.clone(),
    }
}

fn primitive_word(primitive: Primitive) -> &'static str {
    match primitive {
        Primitive::Boolean => "boolean",
        Primitive::String => "string",
        Primitive::Number => "number",
        Primitive::Integer => "integer",
        Primitive::ByteSequence => "byte sequence",
        Primitive::ScalarValueString => "scalar value string",
    }
}

fn infra_word(kind: InfraKind) -> &'static str {
    match kind {
        InfraKind::List => "list",
        InfraKind::OrderedSet => "ordered set",
        InfraKind::OrderedMap => "ordered map",
        InfraKind::Map => "map",
        InfraKind::Tuple => "tuple",
        InfraKind::Struct => "struct",
    }
}

/// `full` marks opaque initial values; tables show the text only.
fn initial(value: Option<&InitialValue>, full: bool) -> String {
    match value {
        None => "—".into(),
        Some(InitialValue::Literal { text, .. }) => text.clone(),
        Some(InitialValue::Empty { .. }) => "empty".into(),
        Some(InitialValue::New { ty, .. }) => format!("a new {}", type_expr(ty, full)),
        Some(InitialValue::Unset { .. }) => "unset".into(),
        Some(InitialValue::Opaque { text }) if full => format!("{text} (opaque)"),
        Some(InitialValue::Opaque { text }) => text.clone(),
    }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn field_table(out: &mut String, rows: &[FieldRow], hidden: u32, with_unclassified: bool) {
    if with_unclassified {
        out.push_str("| Field | Type | Initial | Basis | Writes | Inits | Unclassified |\n|---|---|---|---|---:|---:|---:|\n");
    } else {
        out.push_str(
            "| Field | Type | Initial | Basis | Writes | Inits |\n|---|---|---|---|---:|---:|\n",
        );
    }
    for row in rows {
        let _ = write!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            cell(&format!("{}#{} {}", row.spec, row.anchor, row.name)),
            cell(&type_expr(&row.declared_type, false)),
            cell(&initial(row.initial.as_ref(), false)),
            basis_short(&row.basis),
            row.writes,
            row.inits
        );
        if with_unclassified {
            let _ = write!(out, " {} |", row.unclassified);
        }
        out.push('\n');
    }
    more_rows(out, hidden);
}

fn member_table(out: &mut String, rows: &[MemberRow], hidden: u32) {
    out.push_str("| Member | Adds | Removes |\n|---|---:|---:|\n");
    for row in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            cell(&format!("{}#{} {}", row.spec, row.anchor, row.name)),
            row.adds,
            row.removes
        );
    }
    more_rows(out, hidden);
}

fn more_rows(out: &mut String, hidden: u32) {
    if hidden > 0 {
        let _ = writeln!(out, "({hidden} more rows)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::query::{query, StateQueryOptions};
    use crate::state::testing::query_fixture_db as both;

    #[test]
    fn field_view_matches_the_contract_layout() {
        let conn = both();
        let r = query(
            &conn,
            "HTML#is-initial-about:blank",
            &StateQueryOptions::default(),
        )
        .unwrap();
        let md = response(&r);
        assert_eq!(md, "## HTML#is-initial-about:blank — is initial `about:blank`\n\n\
- Owner: `Document` (idl:Document) — declaration sentence, declared in HTML#the-document-object:\n  \
“Each Document has an is initial about:blank, which is a boolean, initially false.”\n\
- Type: boolean · Initial: false\n\n\
### Writes (1)\n\
- HTML#document-open-steps:1 — Set *document*'s is initial `about:blank` to false.\n\n\
### Initializations (0)\n\n\
Coverage: may · complete — 0 unclassified, 0 possible unlinked writes. Reads are not listed\n(0 occurrences).\n");
    }

    #[test]
    fn type_view_has_header_rows_and_inherited_table() {
        let conn = both();
        let md = response(&query(&conn, "Document", &StateQueryOptions::default()).unwrap());
        assert!(md.starts_with("## Document (idl:Document)\n"));
        assert!(md.contains("| Field | Type | Initial | Basis | Writes | Inits |\n|---|---|---|---|---:|---:|\n| HTML#is-initial-about:blank is initial `about:blank` | boolean | false | sentence | 1 | 0 |"));
        assert!(md.contains("### Inherited from Node\n| Field |"));
    }

    #[test]
    fn inherited_field_shows_its_path_and_counts_unclassified() {
        let conn = both();
        let options = StateQueryOptions::default();
        let md = response(&query(&conn, "Element.node document", &options).unwrap());
        assert!(md.starts_with(
            "## DOM#concept-node-document — node document  (found on Node: Element → Node)\n"
        ));
        assert!(md.contains("\n- Owner: `Node` (idl:Node) — data-dfn-for, declared in DOM#"));
        assert!(md.contains(
            "### Unclassified (1)\n- 1 occurrences; rerun with --unclassified\n\nCoverage: may · partial"
        ));

        let options = StateQueryOptions {
            unclassified: true,
            include_inits: false,
            ..options
        };
        let md = response(&query(&conn, "Element.node document", &options).unwrap());
        assert!(md.contains("### Unclassified (1)\n- HTML#document-open-steps:3 — Add *s*"));
        assert!(!md.contains("### Initializations"));
    }

    #[test]
    fn member_view_splits_adds_and_removes() {
        let conn = both();
        let md = response(
            &query(
                &conn,
                "HTML#sandboxed-navigation-browsing-context-flag",
                &StateQueryOptions::default(),
            )
            .unwrap(),
        );
        assert!(md.contains("- Member of: `sandboxing flag set` (HTML#sandboxing-flag-set)"));
        assert!(md.contains("### Adds (1)\n- HTML#parse-a-sandboxing-directive:1 — Set"));
        assert!(md.contains("### Removes (1)\n- HTML#parse-a-sandboxing-directive:2 — Unset"));
    }

    #[test]
    fn field_list_has_a_table_then_sites_per_field() {
        let conn = both();
        let md = response(&query(&conn, "HTML#*about*", &StateQueryOptions::default()).unwrap());
        assert!(md.starts_with("## HTML#*about* — 1 fields\n\n| Field | Type | Initial | Basis | Writes | Inits | Unclassified |\n"));
        assert!(md.contains(
            "\n### HTML#is-initial-about:blank is initial `about:blank`\nOwner: `Document` (idl:Document)\nWrites (1):\n- HTML#document-open-steps:1 — "
        ));
    }

    #[test]
    fn capped_type_table_says_how_many_rows_are_hidden() {
        let conn = both();
        let options = StateQueryOptions {
            limit: Some(0),
            ..Default::default()
        };
        let md = response(&query(&conn, "Document", &options).unwrap());
        assert!(md.contains("|---|---|---|---|---:|---:|\n(1 more rows)\n"));
    }

    #[test]
    fn type_and_initial_wording() {
        let nominal = TypeExpr::Nominal {
            ty: TypeRef::Known(TypeKey::Idl("Document".into())),
            text: "document".into(),
        };
        let list = TypeExpr::Infra {
            kind: InfraKind::List,
            args: vec![nominal.clone()],
        };
        assert_eq!(type_expr(&list, true), "list of document (idl:Document)");
        assert_eq!(type_expr(&list, false), "list of document");
        assert_eq!(
            type_expr(
                &TypeExpr::Union(vec![
                    TypeExpr::Primitive(Primitive::ByteSequence),
                    TypeExpr::Null
                ]),
                true
            ),
            "byte sequence or null"
        );
        assert_eq!(
            type_expr(&TypeExpr::Enumerated(vec!["a".into(), "b".into()]), true),
            "\"a\", \"b\""
        );
        assert_eq!(type_expr(&TypeExpr::Unknown, true), "—");
        let opaque = InitialValue::Opaque {
            text: "set upon creation".into(),
        };
        assert_eq!(initial(Some(&opaque), true), "set upon creation (opaque)");
        assert_eq!(initial(Some(&opaque), false), "set upon creation");
        let new = InitialValue::New {
            ty: nominal,
            text: String::new(),
        };
        assert_eq!(initial(Some(&new), true), "a new document (idl:Document)");
        assert_eq!(initial(None, true), "—");
        assert_eq!(owners(&[], Some("such")), "unknown (\"such\")".to_string());
        let unindexed = OwnerInfo {
            r#type: "idl:Window".into(),
            name: None,
            via: crate::state::model::OwnerVia::Link,
            indexed: false,
        };
        assert_eq!(
            owners(&[unindexed], None),
            "`Window` (idl:Window, spec not indexed)"
        );
        assert_eq!(
            basis_label(&OwnerBasis::Override {
                rule_id: "html/x".into()
            }),
            "override html/x"
        );
    }

    #[test]
    fn prose_and_branch_label_lines() {
        use crate::state::testing::{db_with, DL_HTML, PROSE_DOM};
        let conn = db_with(&[("DOM", PROSE_DOM), ("HTML", DL_HTML)]);
        let md = response(
            &query(
                &conn,
                "DOM#stop-propagation-flag",
                &StateQueryOptions::default(),
            )
            .unwrap(),
        );
        assert!(
            md.contains("- DOM#dom-event-stoppropagation (method steps, prose) — … set this’s stop propagation flag.\n"),
            "{md}"
        );
        let md = response(
            &query(
                &conn,
                "HTML#is-initial-about:blank",
                &StateQueryOptions::default(),
            )
            .unwrap(),
        );
        assert!(
            md.contains("- HTML#creating-a-new-browsing-context:1 — Let *document* be a new `Document`, with:\n  is initial `about:blank` → true\n"),
            "{md}"
        );
    }

    #[test]
    fn reflection_lines() {
        let conn = crate::state::testing::db_with(&[("HTML", crate::state::reflect::REFLECT_HTML)]);
        let render = |selector: &str| {
            response(&query(&conn, selector, &StateQueryOptions::default()).unwrap())
        };
        let md = render("HTML#attr-hyperlink-target");
        assert!(
            md.contains("### Declared writes (1)\n- HTML#dom-a-target — The target IDL attribute reflects the target content attribute.\n"),
            "{md}"
        );
        let md = render("HTML#dom-a-target");
        assert!(
            md.contains("- Not a declared field.\n- Reflects: the `target` content attribute (HTML#attr-hyperlink-target) — [Reflect]\n"),
            "{md}"
        );
        assert!(!md.contains("### Declared writes"), "{md}");
    }

    #[test]
    fn coverage_view_renders_the_section_5_tables() {
        let conn = crate::state::testing::query_fixture_db();
        let c = crate::state::query::coverage(&conn, "HTML").unwrap();
        let md = coverage(&c);
        assert!(md.starts_with("## State coverage — HTML\n"), "{md}");
        assert!(md.contains("| Rule | Concept dfns |\n|---|---:|\n"), "{md}");
        assert!(
            md.contains("Set statements with a structured target: 2 / 2 (100.0%)"),
            "{md}"
        );
        assert!(md.contains("### Unclassified review (2)"), "{md}");
        assert_eq!(
            crate::state::query::coverage(&conn, "NOPE")
                .unwrap_err()
                .code,
            crate::state::query::StateErrorCode::SpecNotIndexed
        );
    }
}
