//! Compact human renderings of the shared effect model.
use super::catalog::Catalog;
use super::model::*;

/// Presentation categories for the editor, independent of rule-specific payloads.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EditorCategory {
    Async,
    Events,
    Script,
    ScriptContext,
    Other,
}

impl EditorCategory {
    pub fn of(kind: &str) -> Self {
        match kind {
            kind if kind.starts_with("scheduling.") => Self::Async,
            kind if kind.starts_with("event.") => Self::Events,
            kind if kind.starts_with("script.") => Self::Script,
            "execution.context" => Self::ScriptContext,
            _ => Self::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Async => "May do async work",
            Self::Events => "May fire events",
            Self::Script => "May run script",
            Self::ScriptContext => "May change script context",
            Self::Other => "Other possible effects",
        }
    }
}

pub fn editor_categories(result: &EffectSummaryResult) -> Vec<EditorCategory> {
    result
        .effects
        .iter()
        .map(|effect| EditorCategory::of(&effect.kind))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Separate headlines from prepared content so native hovers can toggle each
/// effect without parsing Markdown or issuing another analysis request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EditorEffectDetails {
    pub headline: String,
    pub markdown: String,
}

pub fn editor_effect_details(
    result: &ExplainEffectsResult,
    category: Option<EditorCategory>,
    catalog: Option<&Catalog>,
) -> Vec<EditorEffectDetails> {
    result.effects.iter()
        .filter(|effect| category.is_none_or(|c| EditorCategory::of(&effect.kind) == c))
        .map(|effect| {
            let mut markdown = String::new();
            render_effect_sites(&mut markdown, "Known endpoints", effect, "", false);
            markdown.push_str("\n**Trace and conditions**\n\nRepresentative path; conditions and path feasibility are unchecked.\n\n");
            if let Some(explanation) = result.explanations.iter().find(|item| item.effect_id == effect.id) {
                for witness in &explanation.witnesses { render_witness(&mut markdown, witness); }
            }
            EditorEffectDetails { headline: effect_label(effect, catalog), markdown }
        }).collect()
}

/// Render only the selected category, using paths prepared during indexing.
pub fn editor_details_markdown(
    result: &ExplainEffectsResult,
    category: Option<EditorCategory>,
    catalog: Option<&Catalog>,
) -> String {
    let mut out = format!("# {}\n\n{}\n\nOne representative path per effect. Conditions and path feasibility are unchecked.\n",
        category.map_or("Possible effects", EditorCategory::label), code_span(&subject_text(&result.subject)));
    for effect in result
        .effects
        .iter()
        .filter(|effect| category.is_none_or(|c| EditorCategory::of(&effect.kind) == c))
    {
        out.push_str(&format!(
            "\n## {}\n\n",
            escape(&effect_label(effect, catalog))
        ));
        render_effect_sites(&mut out, "Known endpoints", effect, "", false);
        out.push('\n');
        if let Some(explanation) = result
            .explanations
            .iter()
            .find(|item| item.effect_id == effect.id)
        {
            out.push_str("<details>\n<summary>Trace and conditions</summary>\n\n");
            for witness in &explanation.witnesses {
                render_witness(&mut out, witness);
            }
            out.push_str("</details>\n\n");
        }
    }
    if matches!(
        result.effects_status,
        EffectsStatus::Ready {
            coverage: Coverage::Partial,
            ..
        }
    ) {
        out.push_str(
            "\nAdditional effects may exist; analysis does not resolve every invocation.\n",
        );
    }
    out
}

fn parameter(effect: &EffectSummary, key: &str) -> Option<String> {
    effect.params.get(key)?.as_ref().map(|value| match value {
        EffectValue::String(value) => value.clone(),
        EffectValue::Boolean(value) => value.to_string(),
        EffectValue::Integer(value) => value.to_string(),
    })
}

pub fn effect_label(effect: &EffectSummary, catalog: Option<&Catalog>) -> String {
    let label = match effect.kind.as_str() {
        "event.fire" => parameter(effect, "name")
            .map(|name| format!("fire {name} event"))
            .unwrap_or_else(|| "fire an event".into()),
        "event.dispatch" => parameter(effect, "name")
            .map(|name| format!("dispatch {name} event"))
            .unwrap_or_else(|| "dispatch an event".into()),
        "scheduling.enqueue" => match parameter(effect, "queue").as_deref() {
            Some("microtask") => "queue a microtask".into(),
            Some("task") => parameter(effect, "source")
                .map(|source| format!("queue a task on the {source}"))
                .unwrap_or_else(|| "queue a task".into()),
            Some("traversal") => "enqueue steps on the session history traversal queue".into(),
            Some(queue) => format!("enqueue steps on the {queue}"),
            None => "schedule work".into(),
        },
        "scheduling.parallel" => "run steps in parallel".into(),
        "scheduling.promise-continuation" => "schedule promise continuations".into(),
        "script.invoke" => match parameter(effect, "mechanism").as_deref() {
            Some("callback-function") => "run author code through callbacks".into(),
            Some("user-object-operation") => {
                "run author code through user object operations".into()
            }
            Some("classic-script") => "run author code through classic scripts".into(),
            Some("module-script") => "run author code through module scripts".into(),
            _ => "run author code".into(),
        },
        "script.opportunity" => "allow scripts to run".into(),
        "execution.context" => match parameter(effect, "operation").as_deref() {
            Some("cleanup-script") => "clean up after running script".into(),
            Some("prepare-script") => "prepare to run script".into(),
            Some("cleanup-callback") => "clean up after running a callback".into(),
            Some("prepare-callback") => "prepare to run a callback".into(),
            Some(operation) => operation.to_owned(),
            None => "change execution context".into(),
        },
        _ => {
            let mut label = catalog
                .and_then(|catalog| catalog.effects.get(&effect.kind))
                .map(|definition| definition.label.clone())
                .unwrap_or_else(|| effect.kind.clone());
            let args: Vec<_> = effect
                .params
                .keys()
                .filter_map(|key| parameter(effect, key).map(|value| format!("{key}={value}")))
                .collect();
            if !args.is_empty() {
                label.push_str(&format!(" ({})", args.join(", ")));
            }
            label
        }
    };
    format!(
        "may {}",
        label.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.into();
    }
    if limit == 0 {
        return String::new();
    }
    text.chars()
        .take(limit - 1)
        .chain(std::iter::once('…'))
        .collect()
}

/// All limits count Unicode scalar values, never bytes. The checkmark is owned by the editor.
pub fn compact_badges(
    effects: &[EffectSummary],
    max_badges: usize,
    max_chars: usize,
    catalog: Option<&Catalog>,
) -> String {
    let mut output = String::new();
    let mut shown = 0;
    for effect in effects.iter().take(max_badges) {
        let remaining = effects.len() - shown - 1;
        let overflow = if remaining > 0 {
            format!(" [+{remaining}]")
        } else {
            String::new()
        };
        let separator = usize::from(!output.is_empty());
        let room = max_chars
            .saturating_sub(output.chars().count() + separator + overflow.chars().count() + 2);
        if room < 5 {
            break;
        }
        if separator != 0 {
            output.push(' ');
        }
        output.push('[');
        output.push_str(&shorten(&effect_label(effect, catalog), room));
        output.push(']');
        shown += 1;
    }
    if shown < effects.len() {
        let overflow = format!("[+{}]", effects.len() - shown);
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(&overflow);
    }
    shorten(&output, max_chars)
}

/// Prepared summary for editor hovers: no trace work, internal IDs, or CLI instructions.
/// No text is preferable to implying that an empty partial analysis proves safety.
pub fn editor_summary_markdown(result: &EffectSummaryResult, catalog: Option<&Catalog>) -> String {
    if result.effects.is_empty() {
        return String::new();
    }
    let mut out = String::from("### Possible effects\n\n");
    for effect in result.effects.iter().take(6) {
        out.push_str(&format!("- {}", escape(&effect_label(effect, catalog))));
        if let Some(location) = &effect.location {
            out.push_str(&format!(" in {}", location_label(location)));
        }
        if effect.additional_locations > 0 {
            out.push_str(&format!(" (+{} other sites)", effect.additional_locations));
        }
        out.push('\n');
    }
    if result.effects.len() > 6 {
        out.push_str(&format!(
            "\n{} more effect groups.\n",
            result.effects.len() - 6
        ));
    }
    if matches!(
        result.effects_status,
        EffectsStatus::Ready {
            coverage: Coverage::Partial,
            ..
        }
    ) {
        out.push_str("\nAdditional effects may exist.\n");
    }
    out
}

fn subject_text(subject: &Subject) -> String {
    let mut label = format!("{}#{}", subject.spec, subject.anchor);
    if let Some(path) = &subject.step_path {
        label.push_str(&format!(
            " step {}",
            path.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        ));
    } else if let Some(body) = &subject.body_id {
        label.push_str(&format!(" body {body}"));
    }
    label
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn status_text(status: &EffectsStatus) -> String {
    match status {
        EffectsStatus::Ready {
            coverage: Coverage::Partial,
            ..
        } => "Partial analysis: additional effects may exist.",
        EffectsStatus::Ready { .. } => "",
        EffectsStatus::Pending { .. } => "Analysis is pending.",
        EffectsStatus::Unavailable { issues, .. } if issues.contains(&IssueCode::SnapshotChanged) => {
            "Effects are out of date after an index update. Run `webspec-index effects --all` to rebuild them."
        }
        EffectsStatus::Unavailable { .. } => "Analysis is unavailable.",
        EffectsStatus::Error { .. } => "Analysis failed.",
        EffectsStatus::Disabled { .. } => "Analysis is disabled.",
    }
    .to_owned()
}

pub fn summary_markdown(result: &EffectSummaryResult, catalog: Option<&Catalog>) -> String {
    render_summary(result, catalog, true)
}

fn code_span(text: &str) -> String {
    let fence = "`".repeat(text.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn shell_arg(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/#".contains(c))
    {
        if text.contains('#') {
            format!("\"{text}\"")
        } else {
            text.to_owned()
        }
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn effects_command(result: &EffectSummaryResult) -> String {
    let subject = &result.subject;
    let mut command = format!(
        "webspec-index effects {}",
        shell_arg(&format!("{}#{}", subject.spec, subject.anchor))
    );
    if let Some(id) = &subject.step_id {
        command.push_str(&format!(" --step-id {}", shell_arg(id)));
    } else if let Some(path) = &subject.step_path {
        command.push_str(&format!(
            " --step {}",
            path.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        ));
    } else if let Some(id) = &subject.body_id {
        command.push_str(&format!(" --body-id {}", shell_arg(id)));
    }
    if let Some(manifest) = &result.input_manifest {
        if manifest.environment != "web" {
            command.push_str(&format!(
                " --environment {}",
                shell_arg(&manifest.environment)
            ));
        }
    }
    command
}

fn named_effect_group(kind: &str) -> Option<&'static str> {
    match kind {
        "event.fire" => Some("May fire events"),
        "event.dispatch" => Some("May dispatch events"),
        "script.invoke" => Some("May run author code"),
        _ => None,
    }
}

fn location_label(location: &EffectLocation) -> String {
    let mut label = format!("{}#{}", location.spec, location.anchor);
    if let Some(path) = &location.step_path {
        label.push(':');
        label.push_str(
            &path
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("."),
        );
    }
    code_span(&label)
}

fn render_effect_sites(
    out: &mut String,
    label: &str,
    effect: &EffectSummary,
    indent: &str,
    inline_single: bool,
) {
    let sites: Vec<_> = effect
        .location
        .iter()
        .chain(&effect.other_locations)
        .take(MAX_SUMMARY_LOCATIONS)
        .collect();
    let total = u64::from(effect.location.is_some()) + effect.additional_locations;
    let hidden = total.saturating_sub(sites.len() as u64);
    let mut locations: Vec<(&EffectLocation, usize)> = Vec::new();
    for site in sites {
        if let Some((_, count)) = locations.iter_mut().find(|(other, _)| {
            other.spec == site.spec
                && other.anchor == site.anchor
                && other.step_path == site.step_path
        }) {
            *count += 1;
        } else {
            locations.push((site, 1));
        }
    }
    let display = |(location, count): (&EffectLocation, usize)| {
        let label = location_label(location);
        if count > 1 {
            format!("{label} ({count} sites)")
        } else {
            label
        }
    };
    out.push_str(&format!("{indent}- {label}"));
    if locations.is_empty() {
        out.push('\n');
    } else if inline_single && locations.len() == 1 {
        out.push_str(&format!(" in {}", display(locations[0])));
        if hidden > 0 {
            out.push_str(&format!(
                " (+{hidden} other {})",
                if hidden == 1 { "site" } else { "sites" }
            ));
        }
        out.push('\n');
    } else {
        out.push_str(" in\n");
        for location in locations {
            out.push_str(&format!("{indent}  - {}\n", display(location)));
        }
        if hidden > 0 {
            out.push_str(&format!(
                "{indent}  - +{hidden} other {}\n",
                if hidden == 1 { "site" } else { "sites" }
            ));
        }
    }
}

fn render_effect_groups(out: &mut String, effects: &[EffectSummary], catalog: Option<&Catalog>) {
    // Preserve the analyzer's priority order. Event names and author-code
    // mechanisms share headings; distinct scheduling mechanisms stay separate.
    let mut groups: Vec<Vec<&EffectSummary>> = Vec::new();
    for effect in effects {
        if let Some(group) = groups.iter_mut().find(|group| {
            group[0].kind == effect.kind
                && (named_effect_group(&effect.kind).is_some() || group[0].params == effect.params)
        }) {
            group.push(effect);
        } else {
            groups.push(vec![effect]);
        }
    }
    for group in groups {
        if let Some(heading) = named_effect_group(&group[0].kind) {
            out.push_str(&format!("- {heading}\n"));
            for effect in group {
                let label = if effect.kind == "script.invoke" {
                    match parameter(effect, "mechanism").as_deref() {
                        Some("callback-function") => "callbacks".into(),
                        Some("classic-script") => "classic scripts".into(),
                        Some("module-script") => "module scripts".into(),
                        Some("user-object-operation") => "user-object operations".into(),
                        Some(other) => other.to_owned(),
                        None => "unknown invocation mechanism".into(),
                    }
                } else {
                    parameter(effect, "name").unwrap_or_else(|| "unknown event name".into())
                };
                render_effect_sites(out, &escape(&label), effect, "  ", true);
            }
        } else {
            let effect = group[0];
            let label = effect_label(effect, catalog);
            let label = format!("May {}", label.strip_prefix("may ").unwrap_or(&label));
            render_effect_sites(out, &escape(&label), effect, "", false);
        }
    }
}

fn render_summary(
    result: &EffectSummaryResult,
    catalog: Option<&Catalog>,
    show_commands: bool,
) -> String {
    let mut out = format!(
        "### Possible effects\n\n{}\n",
        escape(&subject_text(&result.subject))
    );
    let status = status_text(&result.effects_status);
    if !status.is_empty() {
        out.push_str(&format!("\n{status}\n"));
    }
    if result.effects.is_empty() && matches!(result.effects_status, EffectsStatus::Ready { .. }) {
        out.push_str("\nNo effects detected by this analysis.\n");
    }
    if !result.effects.is_empty() {
        out.push('\n');
    }
    render_effect_groups(&mut out, &result.effects, catalog);
    if let EffectsStatus::Ready { omitted, .. } = &result.effects_status {
        if *omitted > 0 {
            let total = result.effects.len() as u64 + omitted;
            out.push_str(&format!(
                "\nShowing {} of {total} detected effects; {omitted} more are not shown.\n",
                result.effects.len()
            ));
            if show_commands {
                out.push_str(&format!(
                    "\nList all {total}: {}\n",
                    code_span(&format!(
                        "{} --summary-only --format markdown",
                        effects_command(result)
                    ))
                ));
            }
        }
    }
    for body in &result.defined_bodies {
        out.push_str(&format!(
            "\nDefines {}:\n\n",
            escape(&subject_text(&body.subject))
        ));
        for effect in &body.effects {
            out.push_str(&format!(
                "- {} when invoked\n",
                escape(&effect_label(effect, catalog))
            ));
        }
    }
    if show_commands && matches!(result.effects_status, EffectsStatus::Ready { .. }) {
        out.push_str(&format!(
            "\nTraces and conditions (bounded): {}\n",
            code_span(&format!("{} --format markdown", effects_command(result)))
        ));
    }
    out
}

/// Structural containment edges are useful to the analysis, but aren't calls.
fn containment_hop(hop: &WitnessHop) -> bool {
    hop.relation == Relationship::Invoke
        && hop.boundary.is_none()
        && hop.from.spec == hop.to.spec
        && hop.from.anchor == hop.to.anchor
        && hop.to.step_id.is_some()
        && hop.to.step_id == hop.site.subject.step_id
}

fn same_display_site(left: &SourceSite, right: &SourceSite) -> bool {
    left.subject.spec == right.subject.spec
        && left.subject.anchor == right.subject.anchor
        && left.subject.snapshot_sha == right.subject.snapshot_sha
        && match (&left.subject.step_id, &right.subject.step_id) {
            (Some(left), Some(right)) => left == right,
            _ => left.id == right.id,
        }
}

fn site_link(site: &SourceSite) -> String {
    // Body IDs are analysis details; source links identify the enclosing step.
    let mut subject = site.subject.clone();
    subject.body_id = None;
    format!(
        "[{}](<{}>)",
        escape(&subject_text(&subject)),
        site.url.replace('>', "%3E")
    )
}

fn quoted_excerpt(out: &mut String, text: &str, indent: &str) {
    // Source text already contains Markdown for code, variables and links.
    // A blockquote keeps any embedded lists inside this trace entry.
    for line in text.lines() {
        out.push_str(&format!("{indent}> {line}\n"));
    }
}

fn context_label(context: &ContextItem) -> &'static str {
    match context.kind {
        ContextKind::Enclosing => "Enclosing context",
        ContextKind::Branch => "Branch context",
        ContextKind::Binding => "Argument binding",
        ContextKind::PrecedingExit => "Preceding exit (not a condition)",
        ContextKind::Unsupported => "Unresolved context",
    }
}

fn hop_description(hop: &WitnessHop) -> String {
    if let Some(boundary) = &hop.boundary {
        let operation = boundary.effect.as_ref().map(|effect| {
            let summary = EffectSummary {
                id: String::new(),
                kind: effect.kind.clone(),
                params: effect.params.clone(),
                execution: Vec::new(),
                location: None,
                other_locations: Vec::new(),
                additional_locations: 0,
            };
            let label = effect_label(&summary, None);
            label.strip_prefix("may ").unwrap_or(&label).to_owned()
        });
        return match boundary.execution {
            Execution::Inline => "Continue the remaining steps in the current flow.".into(),
            Execution::Separate => operation.map_or_else(
                || "Continue in separately scheduled steps.".into(),
                |operation| format!("Continue in separately scheduled steps ({operation})."),
            ),
            Execution::Unknown => "Continue into supplied steps; scheduling is unresolved.".into(),
        };
    }
    if hop.to.body_id.is_some() && hop.to.step_path.is_none() {
        let algorithm = code_span(&format!("{}#{}", hop.to.spec, hop.to.anchor));
        return match hop.relation {
            Relationship::CandidateInvoke => {
                format!("Local steps in {algorithm}; invocation or scheduling is unresolved.")
            }
            Relationship::Resume => "Continue the remaining steps.".into(),
            Relationship::ScheduleBody => format!("Enter scheduled local steps in {algorithm}."),
            _ => format!("Run local steps in {algorithm}."),
        };
    }
    let target = code_span(&subject_text(&hop.to));
    match hop.relation {
        Relationship::CandidateInvoke => {
            format!("Linked algorithm {target}; call syntax was not recognized.")
        }
        Relationship::Implements => format!("Use the environment implementation {target}."),
        Relationship::ScheduleBody => "Enter the scheduled body.".into(),
        Relationship::Resume => "Continue the remaining steps.".into(),
        _ => format!("Call {target}."),
    }
}

struct TraceRow<'a> {
    site: SourceSite,
    actions: Vec<String>,
    source_excerpts: Vec<String>,
    context: Vec<ContextItem>,
    evidence: Vec<&'a Evidence>,
}

impl TraceRow<'_> {
    fn new(site: SourceSite) -> Self {
        Self {
            site,
            actions: Vec::new(),
            source_excerpts: Vec::new(),
            context: Vec::new(),
            evidence: Vec::new(),
        }
    }
}

fn render_witness(out: &mut String, witness: &Witness) {
    // Merge only adjacent entries for one step. Distinct calls or boundaries
    // remain separate descriptions, and later revisits remain separate entries.
    let mut rows: Vec<TraceRow<'_>> = Vec::new();
    let mut pending_context = Vec::new();
    for hop in &witness.hops {
        for context in &hop.context {
            if !pending_context.contains(context) {
                pending_context.push(context.clone());
            }
        }
        if containment_hop(hop) {
            continue;
        }
        if !rows
            .last()
            .is_some_and(|row| same_display_site(&row.site, &hop.site))
        {
            rows.push(TraceRow::new(hop.site.clone()));
        }
        let row = rows.last_mut().unwrap();
        row.site.url = hop.site.url.clone();
        row.actions.push(hop_description(hop));
        if let Some(text) = &hop.site.step_text {
            let lower = text.trim_start().to_ascii_lowercase();
            let conditional = ["if ", "unless ", "otherwise", "when "]
                .iter()
                .any(|prefix| lower.starts_with(prefix));
            if (hop.boundary.is_some()
                || hop.relation == Relationship::CandidateInvoke
                || conditional)
                && !row.source_excerpts.contains(text)
            {
                row.source_excerpts.push(text.clone());
            }
        }
        row.context.append(&mut pending_context);
    }
    for evidence in &witness.terminal_evidence {
        if !rows
            .last()
            .is_some_and(|row| same_display_site(&row.site, &evidence.site))
        {
            rows.push(TraceRow::new(evidence.site.clone()));
        }
        let row = rows.last_mut().unwrap();
        row.site.url = evidence.site.url.clone();
        row.context.append(&mut pending_context);
        row.evidence.push(evidence);
    }
    let mut shown_context = Vec::new();
    for (
        index,
        TraceRow {
            site,
            actions: descriptions,
            source_excerpts,
            context,
            evidence,
        },
    ) in rows.into_iter().enumerate()
    {
        let indent = " ".repeat((index + 1).to_string().len() + 2);
        out.push_str(&format!("{}. {}\n\n", index + 1, site_link(&site)));
        for description in descriptions {
            out.push_str(&format!("{indent}{description}\n\n"));
        }
        for text in source_excerpts {
            // The endpoint or captured context below already shows this text.
            if evidence
                .iter()
                .any(|item| item.site.step_text.as_deref() == Some(&text))
                || context.iter().any(|item| item.text == text)
            {
                continue;
            }
            out.push_str(&format!("{indent}Source step:\n\n"));
            quoted_excerpt(out, &text, &indent);
            out.push('\n');
        }
        for item in context {
            if shown_context.contains(&item) {
                continue;
            }
            out.push_str(&format!("{indent}{}:\n\n", context_label(&item)));
            quoted_excerpt(out, &item.text, &indent);
            let lower = item.text.to_ascii_lowercase();
            if lower.contains("following are true") {
                out.push_str(&format!("\n{indent}The individual conditions are not included in the captured context.\n"));
            }
            out.push('\n');
            shown_context.push(item);
        }
        let mut shown_evidence = Vec::new();
        for evidence in evidence {
            let text = if evidence.basis == EvidenceBasis::Declared {
                evidence
                    .reason
                    .as_deref()
                    .unwrap_or("The catalog declares this effect for this algorithm.")
            } else {
                evidence
                    .site
                    .step_text
                    .as_deref()
                    .unwrap_or("The catalog matches an effect at this site.")
            };
            let key = (evidence.basis, text);
            if shown_evidence.contains(&key) {
                continue;
            }
            shown_evidence.push(key);
            out.push_str(&indent);
            out.push_str(if evidence.basis == EvidenceBasis::Declared {
                "**Effect endpoint — reviewed declaration:**\n\n"
            } else {
                "**Effect endpoint:**\n\n"
            });
            quoted_excerpt(out, text, &indent);
            out.push('\n');
        }
    }
    if witness
        .issues
        .iter()
        .any(|issue| issue.code == IssueCode::ContextTruncated)
    {
        out.push_str("Some source context was shortened by the context limit.\n\n");
    }
}

pub fn explanation_markdown(result: &ExplainEffectsResult, catalog: Option<&Catalog>) -> String {
    let summary = EffectSummaryResult {
        schema_version: result.schema_version,
        subject: result.subject.clone(),
        effects: result.effects.clone(),
        effects_status: result.effects_status.clone(),
        defined_bodies: result.defined_bodies.clone(),
        issues: result.issues.clone(),
        input_manifest: result.input_manifest.clone(),
    };
    let mut out = render_summary(&summary, catalog, false);
    if !result.explanations.is_empty() {
        out.push_str("\n### Effect traces\n\nThese are possible paths, not a complete execution log. Conditions and path feasibility are unchecked. Structural nesting steps are omitted; calls, continuation boundaries, and effect endpoints are retained.\n");
    }
    if result
        .explanations
        .iter()
        .any(|item| item.witnesses_truncated)
    {
        out.push_str("\nRepresentative paths are shown. Use `--limit N` for more source sites or continuation alternatives; `--effect-id ID` selects one effect.\n");
    }
    if result
        .explanations
        .iter()
        .flat_map(|item| &item.issues)
        .any(|issue| issue.code == IssueCode::WitnessBudget)
    {
        out.push_str("\nSome requested examples could not be reconstructed. The affected effects below identify the search limit reached.\n");
    }
    for explanation in &result.explanations {
        let effect = result
            .effects
            .iter()
            .find(|effect| effect.id == explanation.effect_id);
        let title = effect
            .map(|effect| effect_label(effect, catalog))
            .unwrap_or_else(|| "effect".into());
        let title = format!("{}{}", title[..1].to_uppercase(), &title[1..]);
        out.push_str(&format!(
            "\n<details>\n<summary>{} (<code>{}</code>)</summary>\n\n",
            escape_html(&title),
            escape_html(&explanation.effect_id)
        ));
        if explanation.witnesses.is_empty() {
            out.push_str("Effect detected, but no complete path was found within the trace-search limits.\n\n");
            if let Some(effect) = effect {
                render_effect_sites(&mut out, "Known effect sites", effect, "", false);
            }
            out.push_str(&format!("\nFocused search with larger limits: {}\n", code_span(&format!(
                "{} --effect-id {} --max-depth 64 --max-witness-states 100000 --limit 3 --format markdown",
                effects_command(&summary), shell_arg(&explanation.effect_id)
            ))));
        }
        for (index, witness) in explanation.witnesses.iter().enumerate() {
            if explanation.witnesses.len() > 1 {
                out.push_str(&format!("**Path {}**\n\n", index + 1));
            }
            render_witness(&mut out, witness);
        }
        for issue in &explanation.issues {
            if issue.code == IssueCode::WitnessBudget {
                out.push_str(&format!("\n{}\n", escape(&issue.message)));
            }
        }
        out.push_str("\n</details>\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn snapshot_changed_names_the_command() {
        let status = EffectsStatus::Unavailable {
            semantics: Semantics::May,
            issues: vec![IssueCode::SnapshotChanged],
            omitted: 0,
        };
        assert!(status_text(&status).contains("webspec-index effects --all"));
    }

    #[test]
    fn scheduling_labels_use_spec_terminology() {
        for (queue, expected) in [
            ("task", "may queue a task"),
            ("microtask", "may queue a microtask"),
            ("parallel queue", "may enqueue steps on the parallel queue"),
            (
                "traversal",
                "may enqueue steps on the session history traversal queue",
            ),
        ] {
            let effect: EffectSummary = serde_json::from_value(serde_json::json!({
                "id": "ef_test", "kind": "scheduling.enqueue", "params": {"queue": queue}, "execution": ["inline"]
            })).unwrap();
            assert_eq!(effect_label(&effect, None), expected);
        }
    }

    #[test]
    fn task_labels_preserve_source_names() {
        let effect: EffectSummary = serde_json::from_value(serde_json::json!({
            "id": "ef_test", "kind": "scheduling.enqueue",
            "params": {"queue": "task", "source": "navigation and traversal task source"},
            "execution": ["inline"]
        }))
        .unwrap();
        assert_eq!(
            effect_label(&effect, None),
            "may queue a task on the navigation and traversal task source"
        );
    }

    #[test]
    fn named_event_labels_say_event() {
        for (kind, expected) in [
            ("event.fire", "may fire DOMContentLoaded event"),
            ("event.dispatch", "may dispatch DOMContentLoaded event"),
        ] {
            let effect: EffectSummary = serde_json::from_value(serde_json::json!({
                "id": "ef_test", "kind": kind,
                "params": {"name": "DOMContentLoaded"}, "execution": ["inline"]
            }))
            .unwrap();
            assert_eq!(effect_label(&effect, None), expected);
        }
    }

    #[test]
    fn truncated_summary_offers_full_list_and_scoped_traces() {
        let mut result: EffectSummaryResult = serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/summary-result.json"
        ))
        .unwrap();
        if let EffectsStatus::Ready { omitted, .. } = &mut result.effects_status {
            *omitted = 16;
        }
        result.effects = vec![result.effects[0].clone(); 12];
        let markdown = summary_markdown(&result, None);
        assert!(markdown.contains("Showing 12 of 28 detected effects; 16 more are not shown."));
        assert!(markdown.contains(
            "webspec-index effects \"TEST#r\" --step-id step-r-1 --summary-only --format markdown"
        ));
        assert!(markdown.contains("Traces and conditions (bounded): `webspec-index effects \"TEST#r\" --step-id step-r-1 --format markdown`"));
        let details: ExplainEffectsResult = serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/explain-result.json"
        ))
        .unwrap();
        assert!(!explanation_markdown(&details, None).contains("Traces and conditions (bounded):"));
    }

    #[test]
    fn summary_shows_endpoint_and_other_sites_without_internal_labels() {
        let mut result: EffectSummaryResult = serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/summary-result.json"
        ))
        .unwrap();
        result.effects[0].location = Some(EffectLocation {
            spec: "HTML".into(),
            anchor: "some-section".into(),
            step_path: Some(vec![3, 1, 3]),
            url: "https://html.spec.whatwg.org/#some-section".into(),
        });
        result.effects[0].additional_locations = 2;
        result.effects[0].execution = vec![Execution::Separate, Execution::Unknown];
        let markdown = summary_markdown(&result, None);
        assert!(markdown
            .contains("- May fire events\n  - a in `HTML#some-section:3.1.3` (+2 other sites)"));
        assert!(!markdown.contains("ef_"));
        assert!(!markdown.contains("separate flow"));
        assert!(!markdown.contains("timing unknown"));
        result.effects[0].location.as_mut().unwrap().step_path = None;
        let markdown = summary_markdown(&result, None);
        assert!(markdown.contains("in `HTML#some-section`"));
        assert!(!markdown.contains("some-section:"));
    }

    #[test]
    fn grouped_summary_separates_event_operations_and_expands_sites() {
        let mut result: EffectSummaryResult = serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/summary-result.json"
        ))
        .unwrap();
        let first = result.effects[0].clone();
        let mut second = first.clone();
        second
            .params
            .insert("name".into(), Some(EffectValue::String("b".into())));
        let mut dispatch = first.clone();
        dispatch.kind = "event.dispatch".into();
        dispatch
            .params
            .insert("name".into(), Some(EffectValue::String("navigate".into())));
        let mut queue = first.clone();
        queue.kind = "scheduling.enqueue".into();
        queue.params = BTreeMap::from([(
            "queue".into(),
            Some(EffectValue::String("microtask".into())),
        )]);
        let mut other = queue.location.clone().unwrap();
        other.anchor = "other".into();
        queue.other_locations = vec![other];
        queue.additional_locations = 4;
        result.effects = vec![queue, first, second, dispatch];
        let markdown = summary_markdown(&result, None);
        assert!(markdown.contains(
            "- May queue a microtask in\n  - `TEST#r:1`\n  - `TEST#other:1`\n  - +3 other sites"
        ));
        assert_eq!(markdown.matches("- May fire events").count(), 1);
        assert!(markdown.contains("  - a in `TEST#r:1`"));
        assert!(markdown.contains("  - b in `TEST#r:1`"));
        assert!(markdown.contains("- May dispatch events\n  - navigate in `TEST#r:1`"));
    }

    #[test]
    fn hints_bound_unicode_and_count_omitted_groups() {
        let effects: Vec<_> = (0..5)
            .map(|i| EffectSummary {
                id: format!("ef_{i}"),
                kind: "event.fire".into(),
                params: BTreeMap::from([(
                    "name".into(),
                    Some(EffectValue::String("😀".repeat(200))),
                )]),
                execution: vec![Execution::Inline],
                location: None,
                other_locations: Vec::new(),
                additional_locations: 0,
            })
            .collect();
        let hints = compact_badges(&effects, 3, 120, None);
        assert!(hints.chars().count() <= 120);
        assert!(hints.contains("[+4]"));
        assert!(hints.contains("may fire"));
    }

    #[test]
    fn unknown_event_names_remain_unknown() {
        let effect = EffectSummary {
            id: "ef_test".into(),
            kind: "event.fire".into(),
            params: BTreeMap::from([("name".into(), None)]),
            execution: vec![Execution::Inline],
            location: None,
            other_locations: Vec::new(),
            additional_locations: 0,
        };
        assert_eq!(effect_label(&effect, None), "may fire an event");
    }
    fn explanation_fixture() -> ExplainEffectsResult {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/explain-result.json"
        ))
        .unwrap()
    }

    #[test]
    fn editor_preview_is_category_scoped_without_internal_ids() {
        let mut result = explanation_fixture();
        let mut async_effect = result.effects[0].clone();
        async_effect.id = "ef_async".into();
        async_effect.kind = "scheduling.parallel".into();
        async_effect.params.clear();
        result.effects.push(async_effect);
        let markdown = editor_details_markdown(&result, Some(EditorCategory::Events), None);
        assert!(markdown.starts_with("# May fire events"));
        assert!(markdown.contains("**Effect endpoint:**"));
        assert!(markdown.contains("Fire an event named `a`."));
        assert!(!markdown.contains("parallel"));
        assert!(!markdown.contains("ef_"));
        assert!(!markdown.contains("--limit"));
        assert!(editor_details_markdown(&result, None, None).contains("may run steps in parallel"));
        let details = editor_effect_details(&result, Some(EditorCategory::Events), None);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].headline, "may fire a event");
        assert!(details[0].markdown.contains("**Effect endpoint:**"));
        assert!(!details[0].markdown.contains("<details>"));
        assert!(!details[0].markdown.contains("parallel"));
    }

    #[test]
    fn trace_rows_merge_structural_duplicates_but_keep_boundaries_and_markup() {
        let mut result = explanation_fixture();
        let witness = &mut result.explanations[0].witnesses[0];
        let site = witness.terminal_evidence[0].site.clone();
        let mut algorithm = site.subject.clone();
        algorithm.step_id = None;
        algorithm.step_path = None;
        let context = ContextItem {
            kind: ContextKind::Enclosing,
            text: "If *ready* is **true**:".into(),
            site: site.clone(),
        };
        let containment = WitnessHop {
            from: algorithm.clone(),
            to: site.subject.clone(),
            relation: Relationship::Invoke,
            site: site.clone(),
            context: vec![context.clone()],
            boundary: None,
        };
        let mut call = containment.clone();
        call.from = site.subject.clone();
        call.to = algorithm;
        call.to.anchor = "child".into();
        call.relation = Relationship::CandidateInvoke;
        let mut boundary = call.clone();
        boundary.relation = Relationship::Resume;
        boundary.boundary = Some(Boundary {
            execution: Execution::Inline,
            operation_site_id: site.id.clone(),
            effect: None,
        });
        witness.hops = vec![containment, call, boundary];
        let markdown = explanation_markdown(&result, None);
        assert!(markdown.contains(
            "<details>\n<summary>May fire a event (<code>ef_0123456789abcdef</code>)</summary>"
        ));
        assert_eq!(markdown.matches("[TEST#r step 1]").count(), 1);
        assert!(markdown.contains("Linked algorithm `TEST#child`; call syntax was not recognized."));
        assert!(markdown.contains("Continue the remaining steps in the current flow."));
        assert_eq!(markdown.matches("If *ready* is **true**:").count(), 1);
        assert!(markdown.contains("> Fire an event named `a`."));
        assert!(!markdown.contains("[inline]"));
        assert!(!markdown.contains(" → "));
        assert!(!markdown.contains("\\*ready\\*"));
    }

    #[test]
    fn later_visits_to_the_same_step_are_not_deduplicated() {
        let mut result = explanation_fixture();
        let witness = &mut result.explanations[0].witnesses[0];
        let site = witness.terminal_evidence[0].site.clone();
        let mut target = site.subject.clone();
        target.anchor = "child".into();
        target.step_id = None;
        target.step_path = None;
        let hop = WitnessHop {
            from: site.subject.clone(),
            to: target.clone(),
            relation: Relationship::Invoke,
            site: site.clone(),
            context: vec![],
            boundary: None,
        };
        let mut other = hop.clone();
        other.site.subject.anchor = "other".into();
        witness.hops = vec![hop.clone(), other, hop];
        let markdown = explanation_markdown(&result, None);
        assert_eq!(markdown.matches("[TEST#r step 1]").count(), 2);
    }

    #[test]
    fn declaration_uses_review_reason_instead_of_dumping_algorithm() {
        let mut result = explanation_fixture();
        let evidence = &mut result.explanations[0].witnesses[0].terminal_evidence[0];
        evidence.basis = EvidenceBasis::Declared;
        evidence.reason = Some("Uses promise jobs for the supplied continuations.".into());
        evidence.site.step_text = Some(
            "To **wait for all**:\n\n1. This full algorithm must not appear.\n2. More steps."
                .into(),
        );
        let markdown = explanation_markdown(&result, None);
        assert!(markdown.contains("Effect endpoint — reviewed declaration:"));
        assert!(markdown.contains("> Uses promise jobs for the supplied continuations."));
        assert!(!markdown.contains("This full algorithm"));
    }

    #[test]
    fn missing_paths_have_focused_command_and_specific_budget_reason() {
        let mut result = explanation_fixture();
        let explanation = &mut result.explanations[0];
        explanation.witnesses.clear();
        explanation.witnesses_truncated = true;
        explanation.issues.push(Issue {
            code: IssueCode::WitnessBudget,
            message: "Trace depth limit (7 graph edges) omitted candidate paths.".into(),
            site: None,
        });
        let markdown = explanation_markdown(&result, None);
        assert!(markdown.contains("Effect detected, but no complete path was found"));
        assert!(markdown.contains("Trace depth limit (7 graph edges)"));
        assert!(markdown.contains("webspec-index effects \"TEST#r\" --step-id step-r-1 --effect-id ef_0123456789abcdef --max-depth 64 --max-witness-states 100000 --limit 3 --format markdown"));
        assert!(!markdown.contains("Further evidence was omitted"));
    }

    #[test]
    fn summary_combines_distinct_sites_with_the_same_step_label() {
        let mut result: EffectSummaryResult = serde_json::from_str(include_str!(
            "../../tests/fixtures/effects/contracts/summary-result.json"
        ))
        .unwrap();
        let location = result.effects[0].location.clone().unwrap();
        let mut other = location.clone();
        other.url.push_str("-another-call");
        result.effects[0].other_locations = vec![other];
        result.effects[0].additional_locations = 1;
        let markdown = summary_markdown(&result, None);
        assert_eq!(markdown.matches("`TEST#r:1`").count(), 1);
        assert!(markdown.contains("`TEST#r:1` (2 sites)"));
    }
    #[test]
    fn requested_example_count_is_not_a_search_warning() {
        let mut result = explanation_fixture();
        result.explanations[0].witnesses_truncated = true;
        let markdown = explanation_markdown(&result, None);
        assert!(markdown.contains("Representative paths are shown."));
        assert!(markdown.contains("`--limit N`"));
        assert!(!markdown.contains("Path limit"));
        assert!(!markdown.contains("search limit"));
        assert!(!markdown.contains("search exhaustion"));
    }
    #[test]
    fn local_body_target_is_not_presented_as_a_linked_algorithm_or_hash() {
        let result = explanation_fixture();
        let site = result.explanations[0].witnesses[0].terminal_evidence[0]
            .site
            .clone();
        let mut target = site.subject.clone();
        target.step_id = None;
        target.step_path = None;
        target.body_id = Some("src-internal-body-id".into());
        let mut hop = WitnessHop {
            from: site.subject.clone(),
            to: target,
            relation: Relationship::CandidateInvoke,
            site,
            context: vec![],
            boundary: None,
        };
        assert_eq!(
            hop_description(&hop),
            "Local steps in `TEST#r`; invocation or scheduling is unresolved."
        );
        hop.relation = Relationship::Invoke;
        assert_eq!(hop_description(&hop), "Run local steps in `TEST#r`.");
        assert!(!hop_description(&hop).contains("src-internal"));
    }

    #[test]
    fn conditional_uncertain_and_scheduled_hops_show_their_source() {
        for (relation, scheduled, text, expected) in [
            (
                Relationship::Invoke,
                false,
                "If *ready*, run the child algorithm.",
                true,
            ),
            (
                Relationship::CandidateInvoke,
                false,
                "Consult the child algorithm.",
                true,
            ),
            (
                Relationship::Resume,
                true,
                "Queue a task to continue these steps.",
                true,
            ),
            (
                Relationship::Invoke,
                false,
                "Run the child algorithm.",
                false,
            ),
        ] {
            let mut result = explanation_fixture();
            let witness = &mut result.explanations[0].witnesses[0];
            let mut site = witness.terminal_evidence[0].site.clone();
            site.step_text = Some(text.into());
            let mut target = site.subject.clone();
            target.anchor = "child".into();
            target.step_id = None;
            target.step_path = None;
            witness.hops = vec![WitnessHop {
                from: site.subject.clone(),
                to: target,
                relation,
                boundary: scheduled.then(|| Boundary {
                    execution: Execution::Separate,
                    operation_site_id: site.id.clone(),
                    effect: None,
                }),
                site,
                context: vec![],
            }];
            let markdown = explanation_markdown(&result, None);
            assert_eq!(markdown.contains(&format!("> {text}")), expected, "{text}");
        }
    }
}
