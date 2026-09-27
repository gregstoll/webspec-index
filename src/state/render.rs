//! Markdown renderings of state query results (§10.2–§10.5).
use crate::state::bind::{issue_code, BindingIssue, Confidence};
use crate::state::ir::{Expr, Hop, Root};
use crate::state::model::{
    AnchorRole, InfraKind, InitialValue, Literal, OwnerBasis, Param, Passing, Primitive,
    ReturnBasis, Signature, StateIssueCode, TemplatePiece, TypeExpr, TypeKey, TypeRef,
};
use crate::state::query::{
    Coverage, FieldListEntry, FieldRow, FoundOn, InheritedFields, MemberRow, OwnerInfo, SiteInfo,
    StateCoverageResult, StateFieldListResult, StateFieldResult, StateMemberResult, StateResponse,
    StateStatus, StateTypeResult,
};
use crate::state::query_algorithm::{CallView, StateAlgorithmResult};
use std::fmt::Write;

pub fn response(response: &StateResponse) -> String {
    match response {
        StateResponse::Field(result) => field(result),
        StateResponse::Type(result) => type_view(result),
        StateResponse::Member(result) => member(result),
        StateResponse::Fields(result) => field_list(result),
        StateResponse::Algorithm(result) => algorithm(result),
    }
}

pub fn algorithm(result: &StateAlgorithmResult) -> String {
    let mut out = format!("## {} — {} (algorithm)\n\n", result.algorithm, result.name);
    let signature = result.signature.as_ref();
    match (&result.template, signature) {
        (Some(template), _) => {
            let _ = writeln!(out, "Call template: {template}");
        }
        (None, Some(sig)) => {
            let _ = writeln!(out, "Call template: none ({} signature)", sig.form.as_str());
        }
        (None, None) => out.push_str("Call template: none (no signature)\n"),
    }
    if let Some(this) = signature.and_then(|sig| sig.this.as_ref()) {
        let _ = writeln!(out, "This: {}", type_expr(this, false));
    }
    out.push('\n');

    if let Some(sig) = signature.filter(|sig| !sig.params.is_empty()) {
        out.push_str("| # | Parameter | Type | Passing | Default |\n|---|---|---|---|---|\n");
        for (i, param) in sig.params.iter().enumerate() {
            let ty = if param.type_text.is_empty() {
                "—".to_string()
            } else {
                param.type_text.replace("-or-", " or ")
            };
            let default = match (&param.default, param.optional) {
                (Some(expr), _) => expr_text(expr),
                (None, true) => "optional".into(),
                (None, false) => "required".into(),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                i + 1,
                cell(&param.name),
                cell(&ty),
                cell(&passing(sig, param)),
                cell(&default)
            );
        }
        out.push('\n');
    }
    if let Some(sig) = signature {
        match &sig.returns {
            Some(returns) => {
                let basis = match returns.basis {
                    ReturnBasis::Intro => "stated in the intro",
                    ReturnBasis::Idl => "IDL",
                    ReturnBasis::Ecmarkup => "ecmarkup",
                    ReturnBasis::ReturnStatements => "from return statements",
                };
                let _ = writeln!(
                    out,
                    "Returns: {} ({basis}).\n",
                    type_expr(&returns.ty, false)
                );
            }
            None => out.push_str("Returns: not stated.\n\n"),
        }
    }

    let total: u32 = result.statements.values().sum();
    let mut kinds: Vec<(&String, &u32)> = result.statements.iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
    if kinds.is_empty() {
        let _ = writeln!(out, "Statements: {total}");
    } else {
        let _ = writeln!(out, "Statements: {total} — {}", kinds.join(", "));
    }
    let _ = writeln!(
        out,
        "Opaque statements: {} · unbound calls: {}\nCoverage: may · {}",
        result.opaque_count,
        result.unbound_calls,
        coverage_label(&result.status)
    );

    if let Some(opaque) = &result.opaque {
        let _ = writeln!(out, "\n### Opaque statements ({})", opaque.len());
        for item in opaque {
            let _ = writeln!(
                out,
                "- {}— {}",
                step_prefix(item.step_path.as_deref()),
                item.text
            );
        }
    }
    if let Some(calls) = &result.calls {
        let _ = writeln!(out, "\n### Calls ({})", calls.len());
        for call in calls {
            let _ = writeln!(
                out,
                "- {}{} ({}) — {}",
                step_prefix(call.step_path.as_deref()),
                call.callee_name,
                call.callee,
                confidence_label(call.confidence)
            );
            let args = call_args(call);
            if !args.is_empty() {
                let _ = writeln!(out, "  {args}");
            }
            if !call.issues.is_empty() {
                let _ = writeln!(out, "  issues: {}", issues(&call.issues));
            }
        }
    }
    if let Some(callers) = &result.callers {
        let _ = writeln!(out, "\n### Callers ({})", callers.total);
        for group in &callers.groups {
            let _ = writeln!(out, "- {}", group.caller);
            for call in &group.calls {
                let mut line = format!(
                    "  - {}— {}",
                    step_prefix(call.step_path.as_deref()),
                    confidence_label(call.confidence)
                );
                let args = call_args(call);
                if !args.is_empty() {
                    let _ = write!(line, " · {args}");
                }
                if !call.issues.is_empty() {
                    let _ = write!(line, " · issues: {}", issues(&call.issues));
                }
                let _ = writeln!(out, "{line}");
            }
        }
        if callers.more > 0 {
            let _ = writeln!(out, "({} more callers; raise --limit)", callers.more);
        }
    }
    out
}

/// How an argument reaches `param`: its template position, name or receiver.
fn passing(sig: &Signature, param: &Param) -> String {
    match &param.passing {
        Passing::Positional { index } => {
            let pieces = sig.template.as_ref().map_or(&[][..], |t| &t.pieces[..]);
            let before = pieces
                .iter()
                .position(|p| *p == TemplatePiece::Slot(*index))
                .and_then(|at| at.checked_sub(1))
                .map(|at| &pieces[at]);
            match before {
                Some(TemplatePiece::Literal(words)) => format!("positional, after \"{words}\""),
                _ => "positional".into(),
            }
        }
        Passing::Named => match &param.anchor {
            Some(anchor) => format!("named ({}#{})", anchor.spec, anchor.anchor),
            None => "named".into(),
        },
        Passing::Receiver => "receiver".into(),
    }
}

fn step_prefix(step_path: Option<&str>) -> String {
    step_path.map_or_else(String::new, |step| format!(":{step} "))
}

fn confidence_label(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Partial => "partial",
        Confidence::Unbound => "unbound",
    }
}

/// `param ← value · …`, then how many arguments were left to their defaults.
fn call_args(call: &CallView) -> String {
    let mut parts: Vec<String> = call
        .args
        .iter()
        .map(|arg| format!("{} ← {}", arg.param, arg.value))
        .collect();
    if call.defaulted > 0 {
        let named = if call.defaulted_named { "named " } else { "" };
        let noun = if call.defaulted == 1 {
            "argument"
        } else {
            "arguments"
        };
        parts.push(format!("{} {named}{noun} defaulted", call.defaulted));
    }
    parts.join(" · ")
}

fn issues(issues: &[BindingIssue]) -> String {
    issues
        .iter()
        .map(|issue| match issue {
            BindingIssue::MissingRequiredArgument(detail)
            | BindingIssue::UnknownNamedArgument(detail)
            | BindingIssue::OpaqueArgument(detail)
            | BindingIssue::TrailingText(detail) => format!("{}({detail})", issue_code(issue)),
            _ => issue_code(issue).to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
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
    site_group(&mut out, "Adds", result.adds.count, &result.adds.items);
    site_group(
        &mut out,
        "Removes",
        result.removes.count,
        &result.removes.items,
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
    let occurrence_total: u32 = c.occurrences.values().sum();
    for (class, count) in &c.occurrences {
        let _ = writeln!(out, "| {class} | {count} |");
    }
    if occurrence_total > 0 {
        let unclassified_n = c.occurrences.get("unclassified").copied().unwrap_or(0);
        let pct = unclassified_n as f64 / occurrence_total as f64 * 100.0;
        let _ = writeln!(out, "\nUnclassified: {pct:.1}%");
    }
    out.push('\n');

    out.push_str("| Prose | Count |\n|---|---:|\n");
    let _ = writeln!(out, "| Sources | {} |", c.prose_sources);
    let _ = writeln!(out, "| Callouts excluded | {} |", c.prose_callouts_excluded);
    let _ = writeln!(out, "| Mentions | {} |", c.prose_mentions);
    out.push('\n');

    let n = c.unclassified_review.len();
    let _ = writeln!(out, "### Unclassified review ({n}, all targets)");
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
        TypeExpr::Idl { text } => text.clone(),
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

/// An expression as prose: variables `*x*`, paths `*x*'s field`, literals as
/// written, enum values ``"`v`"``. Expressions carry no link text, so a link
/// root prints its target anchor.
pub fn expr_text(expr: &Expr) -> String {
    match expr {
        Expr::Var(name) => format!("*{name}*"),
        Expr::This => "this".into(),
        Expr::Literal(literal) => match literal {
            Literal::Null => "null".into(),
            Literal::Bool(true) => "true".into(),
            Literal::Bool(false) => "false".into(),
            Literal::Undefined => "undefined".into(),
            Literal::Failure => "failure".into(),
            Literal::Number(text) => text.clone(),
            Literal::String(text) if text.is_empty() => "the empty string".into(),
            Literal::String(text) => format!("\"{text}\""),
        },
        Expr::EnumValue { text, .. } => format!("\"`{text}`\""),
        Expr::Path(path) => {
            let mut hops = path.hops.iter().map(|hop| match hop {
                Hop::Field { visible_text, .. } => visible_text.clone(),
                Hop::CodeMember { name } => format!("`{name}`"),
                Hop::Slot { name } => format!("[[{name}]]"),
            });
            let mut out = match &path.root {
                Root::Var(name) => format!("*{name}*"),
                Root::This => "this".into(),
                Root::Link { link_id, target } => target
                    .as_ref()
                    .map_or_else(|| link_id.clone(), |t| t.anchor.clone()),
                Root::Opaque { text } => text.clone(),
                Root::Implicit => match hops.next() {
                    Some(hop) => format!("the {hop}"),
                    None => "the".into(),
                },
            };
            for hop in hops {
                out.push_str("'s ");
                out.push_str(&hop);
            }
            if let Some(subscript) = &path.subscript {
                out.push('[');
                out.push_str(&expr_text(subscript));
                out.push(']');
            }
            out
        }
        Expr::List(items) => format!(
            "« {} »",
            items.iter().map(expr_text).collect::<Vec<_>>().join(", ")
        ),
        Expr::New { ty, .. } => match ty {
            Some(TypeRef::Known(key)) => format!("a new {}", fallback_name(&key.to_string())),
            Some(TypeRef::Unresolved(target)) => format!("a new {}", target.anchor),
            None => "a new".into(),
        },
        Expr::Call(_) => "(call)".into(),
        Expr::AlgorithmRef { .. } => "(algorithm)".into(),
        Expr::Conditional { .. } => "(conditional)".into(),
        Expr::Opaque { text } => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::query::{query, StateQueryOptions};
    use crate::state::testing::query_fixture_db as both;

    #[test]
    fn expr_text_forms() {
        use crate::parse::steps::AnchorTarget;
        use crate::state::ir::Path;
        let field = |text: &str| Hop::Field {
            link_id: "l".into(),
            target: None,
            visible_text: text.into(),
        };
        let path = Expr::Path(Path {
            root: Root::Var("d".into()),
            hops: vec![
                field("node document"),
                Hop::Slot {
                    name: "Realm".into(),
                },
            ],
            subscript: Some(Box::new(Expr::Var("k".into()))),
        });
        let implicit = Expr::Path(Path {
            root: Root::Implicit,
            hops: vec![
                field("URL"),
                Hop::CodeMember {
                    name: "href".into(),
                },
            ],
            subscript: None,
        });
        let cases = [
            (Expr::Literal(Literal::Null), "null"),
            (Expr::Literal(Literal::Bool(false)), "false"),
            (Expr::Literal(Literal::Number("0".into())), "0"),
            (
                Expr::Literal(Literal::String(String::new())),
                "the empty string",
            ),
            (Expr::Literal(Literal::String("s".into())), "\"s\""),
            (
                Expr::EnumValue {
                    text: "auto".into(),
                    target: None,
                },
                "\"`auto`\"",
            ),
            (Expr::This, "this"),
            (path, "*d*'s node document's [[Realm]][*k*]"),
            (implicit, "the URL's `href`"),
            (
                Expr::List(vec![Expr::Var("a".into()), Expr::This]),
                "« *a*, this »",
            ),
            (
                Expr::New {
                    ty: Some(TypeRef::Unresolved(AnchorTarget {
                        spec: "DOM".into(),
                        anchor: "concept-event".into(),
                    })),
                    init: None,
                },
                "a new concept-event",
            ),
            (Expr::Call("c".into()), "(call)"),
            (Expr::Opaque { text: "it".into() }, "it"),
        ];
        for (expr, text) in cases {
            assert_eq!(expr_text(&expr), text);
        }
    }

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
    fn algorithm_view_has_template_params_and_counts() {
        use crate::state::testing::{db_with, NAV_HTML};
        let conn = db_with(&[("HTML", NAV_HTML)]);
        let md = response(&query(&conn, "HTML#navigate", &StateQueryOptions::default()).unwrap());
        assert_eq!(md, "## HTML#navigate — navigate (algorithm)\n\n\
Call template: navigate {navigable} to {url} using {sourceDocument}, with named arguments\n\n\
| # | Parameter | Type | Passing | Default |\n|---|---|---|---|---|\n\
| 1 | navigable | navigable | positional | required |\n\
| 2 | url | URL | positional, after \"to\" | required |\n\
| 3 | sourceDocument | `Document` or null | positional, after \"using\" | null |\n\
| 4 | exceptionsEnabled | boolean | named (HTML#exceptions-enabled) | false |\n\
| 5 | historyHandling | `NavigationHistoryBehavior` | named (HTML#navigation-hh) | \"`auto`\" |\n\
| 6 | referrerPolicy | referrer policy | named (HTML#navigation-referrer-policy) | the empty string |\n\n\
Returns: not stated.\n\n\
Statements: 5 — if 1, in_parallel 1, let 1, return 1, set 1\n\
Opaque statements: 0 · unbound calls: 0\n\
Coverage: may · complete\n");
    }

    #[test]
    fn algorithm_view_calls_callers_and_opaque_sections() {
        use crate::state::testing::{db_with, NAV_HTML};
        let conn = db_with(&[("HTML", NAV_HTML)]);
        let options = StateQueryOptions {
            calls: true,
            ..StateQueryOptions::default()
        };
        let md = response(&query(&conn, "HTML#location-object-navigate", &options).unwrap());
        assert!(md.contains("### Calls (1)\n- :4 navigate (HTML#navigate) — exact\n  navigable ← *navigable* · url ← *url* · sourceDocument ← *sourceDocument* · exceptionsEnabled ← true · historyHandling ← *historyHandling* · 1 named argument defaulted\n"), "{md}");
        let options = StateQueryOptions {
            callers: true,
            ..StateQueryOptions::default()
        };
        let md = response(&query(&conn, "HTML#navigate", &options).unwrap());
        assert!(
            md.contains("### Callers (1)\n- HTML#location-object-navigate\n  - :4 — exact · navigable ← *navigable* ·"),
            "{md}"
        );
        assert!(!md.contains("more callers"), "{md}");
        let options = StateQueryOptions {
            callers: true,
            limit: Some(0),
            ..StateQueryOptions::default()
        };
        let md = response(&query(&conn, "HTML#navigate", &options).unwrap());
        assert!(
            md.contains("### Callers (1)\n(1 more callers; raise --limit)\n"),
            "{md}"
        );

        let other = r##"<div data-algorithm=""><p>To <dfn id="x">x</dfn> given a <var>n</var>:</p><ol><li><p><a href="https://dom.spec.whatwg.org/#concept-node-remove">Remove</a> <var>n</var>.</p></li><li><p>Add <var>n</var> to the list of reloaded things.</p></li></ol></div>"##;
        let conn = db_with(&[("HTML", other)]);
        let options = StateQueryOptions {
            calls: true,
            opaque: true,
            ..StateQueryOptions::default()
        };
        let md = response(&query(&conn, "HTML#x", &options).unwrap());
        assert!(
            md.contains("Opaque statements: 1 · unbound calls: 1\nCoverage: may · partial\n"),
            "{md}"
        );
        assert!(
            md.contains(
                "### Opaque statements (1)\n- :2 — Add *n* to the list of reloaded things\n"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "- :1 Remove (DOM#concept-node-remove) — unbound\n  issues: no_signature, missing_spec\n"
            ),
            "{md}"
        );
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
        assert!(
            md.contains("### Unclassified review (2, all targets)"),
            "{md}"
        );
        assert_eq!(
            crate::state::query::coverage(&conn, "NOPE")
                .unwrap_err()
                .code,
            crate::state::query::StateErrorCode::SpecNotIndexed
        );
    }
}
