//! `extract_state` (§9.1): the object model, statement sources, statements and
//! occurrences of one snapshot, and the rows derived from them (§9.2).
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{ParsedIdlDefinition, ParsedSection};
use crate::parse::steps::{AnchorTarget, InlineTokenKind, LinkSpan, StructuralSpec, TextSpan};
use crate::state::declare::{self, NameBindings};
use crate::state::ir::{
    self, Expr, Hop, MutationOp, Path, Root, SetForm, SourceContext, Statement, StatementKind,
    StatementSource,
};
use crate::state::model::{
    CoverageCounters, FieldDef, Literal, ObjectModel, Occurrence, OccurrenceClass, Owner,
    ReviewItem, Site, SiteClass, StateCatalog, StateSpec, TypeKey, TypeRef,
};
use crate::state::{classify, types};

/// Everything `extract_state` reads. The caller parses the document once and
/// shares it with the section and structural extraction.
pub struct StateInputs<'a> {
    pub document: &'a scraper::Html,
    pub spec: &'a str,
    pub base_url: &'a str,
    pub snapshot_sha: &'a str,
    pub structure: &'a StructuralSpec,
    pub sections: &'a [ParsedSection],
    pub idl_definitions: &'a [ParsedIdlDefinition],
    pub catalog: &'a StateCatalog,
}

/// The reviewed catalog applied at index time.
pub fn bundled_catalog() -> &'static StateCatalog {
    static CATALOG: OnceLock<StateCatalog> = OnceLock::new();
    CATALOG.get_or_init(StateCatalog::default)
}

/// The `representation_version` of a snapshot indexed with the bundled catalog.
pub fn bundled_representation_version() -> String {
    bundled_catalog().representation_version()
}

/// Pure: no DB access, no network.
pub fn extract_state(inputs: &StateInputs) -> StateSpec {
    let StateInputs {
        document,
        spec,
        base_url,
        structure,
        ..
    } = *inputs;
    let concepts = declare::concept_dfns(document);
    let mut table =
        types::collect_types(document, spec, base_url, inputs.idl_definitions, &concepts);
    let declared = declare::declare_fields(
        document,
        spec,
        base_url,
        structure,
        &mut table,
        &NameBindings::default(),
    );
    let model = ObjectModel {
        types: table.types.into_values().collect(),
        fields: declared.fields,
        members: declared.members,
        reflections: vec![],
    };

    let sources = algorithm_sources(structure);
    let mut statements = Vec::new();
    let mut occurrences = Vec::new();
    for source in &sources {
        let parsed = ir::parse_source(source);
        occurrences.extend(classify::classify(source, &parsed));
        statements.extend(parsed.statements);
    }

    let counters = declared.counters;
    let mut coverage = CoverageCounters {
        concept_dfns: counters.concept_dfns,
        owner_by_rule: counters.owner_by_rule,
        owner_candidates: counters.owner_candidates,
        owner_resolved: counters.owner_resolved,
        set_members: counters.set_members,
        ..CoverageCounters::default()
    };
    count_statements(&statements, &mut coverage);
    let concept_ids: HashSet<&str> = concepts.iter().map(|c| c.id.as_str()).collect();
    count_occurrences(
        spec,
        &model.fields,
        &concept_ids,
        &sources,
        &occurrences,
        &mut coverage,
    );

    StateSpec {
        representation_version: inputs.catalog.representation_version(),
        spec: spec.to_string(),
        snapshot_sha: inputs.snapshot_sha.to_string(),
        model,
        sources,
        statements,
        occurrences,
        prose_mentions: BTreeMap::new(),
        coverage,
        issues: declared.issues,
    }
}

/// The snake_case serde tag of a unit enum variant.
fn tag<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(tag)) => tag,
        _ => "other".to_string(),
    }
}

fn anchor_key(target: &AnchorTarget) -> String {
    format!("{}#{}", target.spec, target.anchor)
}

fn root_name(root: &Root) -> &'static str {
    match root {
        Root::Var(_) => "var",
        Root::This => "this",
        Root::Link { .. } => "link",
        Root::Implicit => "implicit",
        Root::Opaque { .. } => "opaque",
    }
}

/// `var`, `this`, `link` (each with `_subscript`), `field:<root>`,
/// `field_subscript:<root>`, `code_member`, `slot` or `other`.
fn path_shape(path: &Path) -> String {
    let root = root_name(&path.root);
    let subscript = if path.subscript.is_some() {
        "_subscript"
    } else {
        ""
    };
    match path.hops.last() {
        None if matches!(path.root, Root::Var(_) | Root::This | Root::Link { .. }) => {
            format!("{root}{subscript}")
        }
        None => "other".to_string(),
        Some(Hop::Field { .. }) => format!("field{subscript}:{root}"),
        Some(Hop::CodeMember { .. }) => "code_member".to_string(),
        Some(Hop::Slot { .. }) => "slot".to_string(),
    }
}

/// Statement-form key: `let`, `set:<form>:<shape>`, `mutate:<op>:<shape>`,
/// `init:<form>`, `opaque:<reason>:<verb or none>`.
fn statement_key(kind: &StatementKind) -> String {
    match kind {
        StatementKind::Let { .. } => "let".to_string(),
        StatementKind::Set { targets, form, .. } => {
            let shape = targets.first().map_or("other".to_string(), path_shape);
            format!("set:{}:{shape}", tag(form))
        }
        StatementKind::Mutate { op, target, .. } => {
            format!("mutate:{}:{}", tag(op), path_shape(target))
        }
        StatementKind::Init { form, .. } => format!("init:{}", tag(form)),
        StatementKind::Opaque { reason, verb, .. } => {
            format!(
                "opaque:{}:{}",
                tag(reason),
                verb.as_deref().unwrap_or("none")
            )
        }
    }
}

fn count_statements(statements: &[Statement], coverage: &mut CoverageCounters) {
    for statement in statements {
        *coverage
            .statements
            .entry(statement_key(&statement.kind))
            .or_default() += 1;
        match &statement.kind {
            StatementKind::Set { .. } => {
                coverage.set_total += 1;
                coverage.set_structured += 1;
            }
            StatementKind::Opaque {
                verb: Some(verb), ..
            } if verb == "set" || verb == "unset" => coverage.set_total += 1,
            _ => {}
        }
    }
}

fn step_path(context: &SourceContext) -> Option<&str> {
    match context {
        SourceContext::Algorithm { step_path, .. } | SourceContext::Prose { step_path, .. } => {
            step_path.as_deref()
        }
        SourceContext::BranchLabel { step_path, .. } => Some(step_path),
    }
}

fn step_id(context: &SourceContext) -> Option<&str> {
    match context {
        SourceContext::Algorithm { step_id, .. } => step_id.as_deref(),
        SourceContext::BranchLabel { step_id, .. } => Some(step_id),
        SourceContext::Prose { .. } => None,
    }
}

/// Occurrence classes over local owned fields, written fields by owner basis,
/// unresolved links and the unclassified review list (§5.1, §5.3). A written
/// field is a local `FieldDef` or concept dfn that is the target of a `Write`.
fn count_occurrences(
    spec: &str,
    fields: &[FieldDef],
    concept_ids: &HashSet<&str>,
    sources: &[StatementSource],
    occurrences: &[Occurrence],
    coverage: &mut CoverageCounters,
) {
    let fields: HashMap<&str, &FieldDef> = fields
        .iter()
        .filter(|field| field.anchor.spec == spec)
        .map(|field| (field.anchor.anchor.as_str(), field))
        .collect();
    let sources_by_id: HashMap<&str, &StatementSource> =
        sources.iter().map(|s| (s.id.as_str(), s)).collect();
    coverage.unresolved_links = sources
        .iter()
        .flat_map(|source| &source.links)
        .filter(|link| link.target.is_none())
        .count() as u32;

    let mut written = BTreeSet::new();
    for occurrence in occurrences {
        let local = occurrence
            .target
            .as_ref()
            .filter(|target| target.spec == spec)
            .map(|target| target.anchor.as_str());
        if let Some(anchor) = local {
            let owned = fields
                .get(anchor)
                .is_some_and(|field| matches!(field.owner, Owner::Known { .. }));
            if owned {
                *coverage
                    .occurrences
                    .entry(tag(&occurrence.class))
                    .or_default() += 1;
            }
            let is_field = fields.contains_key(anchor) || concept_ids.contains(anchor);
            if occurrence.class == OccurrenceClass::Write && is_field {
                written.insert(anchor);
            }
        }
        if occurrence.class == OccurrenceClass::Unclassified {
            let source = sources_by_id.get(occurrence.source_id.as_str());
            coverage.unclassified_review.push(ReviewItem {
                subject: source.map_or_else(String::new, |s| anchor_key(&s.subject)),
                step_path: source
                    .and_then(|s| step_path(&s.context))
                    .map(str::to_string),
                target: occurrence
                    .target
                    .as_ref()
                    .map_or_else(|| "unresolved".to_string(), anchor_key),
                text: source.map_or_else(String::new, |s| s.text.clone()),
            });
        }
    }

    coverage.written_fields = written.len() as u32;
    for anchor in written {
        match fields.get(anchor).map(|field| &field.owner) {
            Some(Owner::Known { basis, .. }) => {
                *coverage
                    .written_fields_owned
                    .entry(declare::rule_key(basis).to_string())
                    .or_default() += 1;
            }
            _ => coverage.written_fields_unresolved.push(anchor.to_string()),
        }
    }
}

/// `site-` + sha256(source_id \0 link_id-or-statement_id \0 class).
fn site_id(source_id: &str, local_id: &str, class: SiteClass) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    for component in [local_id, class.as_str()] {
        hasher.update([0]);
        hasher.update(component.as_bytes());
    }
    format!("site-{:x}", hasher.finalize())
}

/// The parts of a site that depend on its class and statement.
struct SiteParts {
    op: String,
    receiver: &'static str,
    constructed: Option<String>,
    target_text: String,
    value_text: Option<String>,
}

/// One `state_sites` row per `Write`/`Init`/`Unclassified` occurrence with a
/// target, then one `OpaqueWrite` row per `Opaque` statement with a verb (§9.2).
pub fn derive_sites(state: &StateSpec) -> Vec<Site> {
    let sources: HashMap<&str, &StatementSource> =
        state.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let statements: HashMap<&str, &Statement> = state
        .statements
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut opaque_by_source: HashMap<&str, Vec<&Statement>> = HashMap::new();
    for statement in &state.statements {
        if matches!(statement.kind, StatementKind::Opaque { verb: Some(_), .. }) {
            opaque_by_source
                .entry(statement.source_id.as_str())
                .or_default()
                .push(statement);
        }
    }
    let type_anchors: HashMap<&AnchorTarget, &TypeKey> = state
        .model
        .types
        .iter()
        .flat_map(|ty| ty.anchors.iter().map(move |a| (&a.target, &ty.key)))
        .collect();

    let mut sites = Vec::new();
    for occurrence in &state.occurrences {
        let class = match occurrence.class {
            OccurrenceClass::Write => SiteClass::Write,
            OccurrenceClass::Init => SiteClass::Init,
            OccurrenceClass::Unclassified => SiteClass::Unclassified,
            OccurrenceClass::Read | OccurrenceClass::ReadPath => continue,
        };
        let Some(target) = &occurrence.target else {
            continue;
        };
        let Some(source) = sources.get(occurrence.source_id.as_str()) else {
            continue;
        };
        let Some(link) = source.links.iter().find(|l| l.id == occurrence.link_id) else {
            continue;
        };
        let statement = occurrence
            .statement_id
            .as_deref()
            .and_then(|id| statements.get(id).copied());
        let parts = match class {
            SiteClass::Write => write_parts(source, statement, link),
            SiteClass::Init => init_parts(source, statement, link, &type_anchors),
            _ => SiteParts {
                op: clause_verb(&opaque_by_source, source, link),
                receiver: "opaque",
                constructed: None,
                target_text: link_text(source, link),
                value_text: None,
            },
        };
        sites.push(site(
            source,
            site_id(&source.id, &link.id, class),
            class,
            Some(target.clone()),
            parts,
            &occurrence.basis,
            statement.as_ref().map(|s| s.span),
        ));
    }

    for statement in &state.statements {
        let StatementKind::Opaque {
            verb: Some(verb),
            target_text,
            ..
        } = &statement.kind
        else {
            continue;
        };
        let Some(source) = sources.get(statement.source_id.as_str()) else {
            continue;
        };
        let class = SiteClass::OpaqueWrite;
        let parts = SiteParts {
            op: verb.clone(),
            receiver: "opaque",
            constructed: None,
            target_text: target_text.clone().unwrap_or_default(),
            value_text: None,
        };
        sites.push(site(
            source,
            site_id(&source.id, &statement.id, class),
            class,
            None,
            parts,
            "ir",
            Some(statement.span),
        ));
    }
    sites
}

fn site(
    source: &StatementSource,
    site_id: String,
    class: SiteClass,
    target: Option<AnchorTarget>,
    parts: SiteParts,
    basis: &str,
    statement_span: Option<TextSpan>,
) -> Site {
    let (context, role) = match &source.context {
        SourceContext::Algorithm { .. } => ("algorithm", None),
        SourceContext::BranchLabel { .. } => ("branch_label", None),
        SourceContext::Prose { role, .. } => ("prose", Some(tag(role))),
    };
    let (segment_id, body_id) = match &source.context {
        SourceContext::Algorithm {
            segment_id,
            body_id,
            ..
        } => (Some(segment_id.clone()), Some(body_id.clone())),
        SourceContext::BranchLabel { segment_id, .. } => (Some(segment_id.clone()), None),
        SourceContext::Prose { .. } => (None, None),
    };
    let (span_start, span_end) = match statement_span {
        Some(span) => (
            Some(span.start.min(u32::MAX as usize) as u32),
            Some(span.end.min(u32::MAX as usize) as u32),
        ),
        None => (None, None),
    };
    Site {
        site_id,
        class,
        target,
        op: parts.op,
        subject: source.subject.clone(),
        context: context.to_string(),
        role,
        constructed: parts.constructed,
        step_path: step_path(&source.context).map(str::to_string),
        step_id: step_id(&source.context).map(str::to_string),
        receiver: parts.receiver.to_string(),
        target_text: parts.target_text,
        value_text: parts.value_text,
        text: source.text.clone(),
        basis: basis.to_string(),
        segment_id,
        body_id,
        span_start,
        span_end,
    }
}

fn link_text(source: &StatementSource, link: &LinkSpan) -> String {
    source.text[link.span.start..link.span.end].to_string()
}

/// Trimmed source text of `start..end`, if not empty.
fn text(source: &StatementSource, start: usize, end: usize) -> Option<String> {
    let slice = source.text.get(start..end)?.trim();
    (!slice.is_empty()).then(|| slice.to_string())
}

/// The verb of the unconsumed clause a link sits in: that of the `Opaque`
/// statement covering it.
fn clause_verb(
    opaque_by_source: &HashMap<&str, Vec<&Statement>>,
    source: &StatementSource,
    link: &LinkSpan,
) -> String {
    opaque_by_source
        .get(source.id.as_str())
        .into_iter()
        .flatten()
        .find(|st| st.span.start <= link.span.start && link.span.start < st.span.end)
        .and_then(|st| match &st.kind {
            StatementKind::Opaque { verb, .. } => verb.clone(),
            _ => None,
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn last_hop_is(path: &Path, link_id: &str) -> bool {
    matches!(path.hops.last(), Some(Hop::Field { link_id: id, .. }) if id == link_id)
}

fn link_span_start(source: &StatementSource, link_id: &str) -> Option<usize> {
    source
        .links
        .iter()
        .find(|link| link.id == link_id)
        .map(|link| link.span.start)
}

/// Where the text of `path` starts in the statement starting at `from`: the
/// earliest of its root and field hops, with a leading `the `.
fn path_start(source: &StatementSource, path: &Path, from: usize, write: &LinkSpan) -> usize {
    let first_hop = path
        .hops
        .iter()
        .filter_map(|hop| match hop {
            Hop::Field { link_id, .. } => link_span_start(source, link_id),
            _ => None,
        })
        .filter(|&start| start >= from)
        .min()
        .unwrap_or(write.span.start)
        .max(from);
    let before = &source.text[from..first_hop];
    let root = match &path.root {
        Root::Var(name) => source
            .tokens
            .iter()
            .filter(|t| t.kind == InlineTokenKind::Variable && t.source_text == *name)
            .filter(|t| t.span.start >= from && t.span.end <= first_hop)
            .map(|t| t.span.start)
            .next_back(),
        Root::Link { link_id, .. } => link_span_start(source, link_id),
        Root::This => before.rfind("this").map(|at| from + at),
        Root::Opaque { text } => before.rfind(text.as_str()).map(|at| from + at),
        Root::Implicit => None,
    };
    let start = root.map_or(first_hop, |root| root.min(first_hop)).max(from);
    let lead = &source.text[from..start];
    if lead.ends_with("the ") || lead.ends_with("The ") {
        start - "the ".len()
    } else {
        start
    }
}

/// `Set`/`Mutate` write: op, receiver, target text and value text.
fn write_parts(
    source: &StatementSource,
    statement: Option<&Statement>,
    link: &LinkSpan,
) -> SiteParts {
    let fallback = || SiteParts {
        op: "unknown".to_string(),
        receiver: "opaque",
        constructed: None,
        target_text: link_text(source, link),
        value_text: None,
    };
    let Some(statement) = statement else {
        return fallback();
    };
    let span = statement.span;
    let (op, path, separator, operand_before) = match &statement.kind {
        StatementKind::Set {
            targets,
            value,
            form,
        } => {
            let path = targets
                .iter()
                .find(|path| last_hop_is(path, &link.id))
                .or(targets.first());
            let op = match (form, value) {
                (SetForm::Flag, Expr::Literal(Literal::Bool(false))) => "unset",
                _ => "set",
            };
            let separator = (*form != SetForm::Flag).then_some(" to ");
            (op.to_string(), path, separator, false)
        }
        StatementKind::Mutate {
            op,
            target,
            operand,
            ..
        } => {
            let separator = match op {
                MutationOp::MapSet => Some(" to "),
                MutationOp::Increment | MutationOp::Decrement => operand.as_ref().map(|_| " by "),
                _ => None,
            };
            let before = separator.is_none() && operand.is_some();
            (tag(op), Some(target), separator, before)
        }
        _ => return fallback(),
    };
    let Some(path) = path else {
        return fallback();
    };
    let start = path_start(source, path, span.start, link);
    let (target_text, value_text) = match separator {
        Some(separator) => {
            let found = source
                .text
                .get(link.span.end..span.end)
                .and_then(|rest| rest.find(separator))
                .map(|at| link.span.end + at);
            match found {
                Some(at) => (
                    text(source, start, at),
                    text(source, at + separator.len(), span.end),
                ),
                None => (text(source, start, span.end), None),
            }
        }
        None if operand_before => (
            text(source, start, link.span.end),
            operand_text(source, span.start, start),
        ),
        None => (text(source, start, span.end.max(link.span.end)), None),
    };
    SiteParts {
        op,
        receiver: root_name(&path.root),
        constructed: None,
        target_text: target_text.unwrap_or_else(|| link_text(source, link)),
        value_text,
    }
}

/// The operand of `VERB OPERAND PREP TARGET` (Infra mutations): the text
/// between the verb and the target, without the preposition.
fn operand_text(source: &StatementSource, from: usize, target_start: usize) -> Option<String> {
    let head = source.text.get(from..target_start)?;
    let verb_end = source
        .links
        .iter()
        .find(|link| link.span.start == from)
        .map(|link| link.span.end)
        .or_else(|| head.find(' ').map(|at| from + at))?;
    let operand = source.text.get(verb_end..target_start)?.trim_end();
    let operand = [" into", " from", " to", " in", " on"]
        .iter()
        .find_map(|prep| operand.strip_suffix(prep))
        .unwrap_or(operand)
        .trim();
    (!operand.is_empty()).then(|| operand.to_string())
}

/// `Init` entry: the field hop text, the entry value, and the constructed type
/// canonicalized through the local type anchors.
fn init_parts(
    source: &StatementSource,
    statement: Option<&Statement>,
    link: &LinkSpan,
    type_anchors: &HashMap<&AnchorTarget, &TypeKey>,
) -> SiteParts {
    let mut parts = SiteParts {
        op: "init".to_string(),
        receiver: "new",
        constructed: None,
        target_text: link_text(source, link),
        value_text: None,
    };
    let Some(StatementKind::Init {
        constructed,
        entries,
        ..
    }) = statement.map(|s| &s.kind)
    else {
        return parts;
    };
    parts.constructed = constructed.as_ref().map(|ty| match ty {
        TypeRef::Known(key) => key.to_string(),
        TypeRef::Unresolved(target) => type_anchors
            .get(target)
            .map_or_else(|| anchor_key(target), |key| key.to_string()),
    });
    let index = entries.iter().position(
        |entry| matches!(&entry.field, Hop::Field { link_id, .. } if *link_id == link.id),
    );
    if let (Some(index), Some(statement)) = (index, statement) {
        let next_start = entries.get(index + 1).and_then(|entry| match &entry.field {
            Hop::Field { link_id, .. } => link_span_start(source, link_id),
            _ => None,
        });
        parts.value_text = entry_value_text(source, link.span.end, next_start, statement.span.end);
    }
    parts
}

/// `⟦F⟧ is VALUE` / `⟦F⟧ set to VALUE`, up to the next entry or the statement end.
fn entry_value_text(
    source: &StatementSource,
    field_end: usize,
    next_start: Option<usize>,
    statement_end: usize,
) -> Option<String> {
    let rest = source.text.get(field_end..)?;
    let value_start = field_end
        + [" is ", " set to "]
            .iter()
            .find(|word| rest.starts_with(*word))
            .map(|word| word.len())?;
    let value = source
        .text
        .get(value_start..next_start.unwrap_or(statement_end))?
        .trim_end();
    let value = value.strip_suffix(" its").unwrap_or(value);
    let value = [", and", " and", ","]
        .iter()
        .find_map(|separator| value.strip_suffix(separator))
        .unwrap_or(value)
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Per target: totals of `read` and `read_path` occurrences and prose mentions.
pub fn derive_occurrence_counts(state: &StateSpec) -> Vec<(AnchorTarget, &'static str, u32)> {
    let mut counts: BTreeMap<(AnchorTarget, &'static str), u32> = BTreeMap::new();
    for occurrence in &state.occurrences {
        let class = match occurrence.class {
            OccurrenceClass::Read => "read",
            OccurrenceClass::ReadPath => "read_path",
            _ => continue,
        };
        if let Some(target) = &occurrence.target {
            *counts.entry((target.clone(), class)).or_default() += 1;
        }
    }
    for (key, count) in &state.prose_mentions {
        if let Some((spec, anchor)) = key.split_once('#') {
            let target = AnchorTarget {
                spec: spec.to_string(),
                anchor: anchor.to_string(),
            };
            *counts.entry((target, "prose_mention")).or_default() += count;
        }
    }
    counts
        .into_iter()
        .map(|((target, class), count)| (target, class, count))
        .collect()
}

/// One source per structural segment, in algorithm and segment order.
pub(crate) fn algorithm_sources(structure: &StructuralSpec) -> Vec<StatementSource> {
    let mut sources = Vec::new();
    for algorithm in &structure.algorithms {
        let subject = AnchorTarget {
            spec: structure.spec.clone(),
            anchor: algorithm.source.section_anchor.clone(),
        };
        let step_paths: BTreeMap<&str, String> = algorithm
            .steps
            .iter()
            .map(|step| {
                let path = step
                    .path
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(".");
                (step.source.node_id.as_str(), path)
            })
            .collect();
        for segment in &algorithm.segments {
            let step_path = segment
                .owner_step_id
                .as_deref()
                .and_then(|id| step_paths.get(id).cloned());
            sources.push(StatementSource {
                id: segment.source.node_id.clone(),
                subject: subject.clone(),
                context: SourceContext::Algorithm {
                    segment_id: segment.source.node_id.clone(),
                    step_id: segment.owner_step_id.clone(),
                    step_path,
                    body_id: segment.owner_body_id.clone(),
                },
                text: segment.text.clone(),
                tokens: segment.tokens.clone(),
                links: segment.links.clone(),
            });
        }
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{extract_html as extract, MINI};
    use crate::state::STATE_VERSION;

    #[test]
    fn mini_spec_produces_field_sites_and_counts() {
        let state = extract(MINI, "HTML");
        assert_eq!(state.representation_version, STATE_VERSION);
        assert_eq!(state.model.fields.len(), 1);
        let sites = derive_sites(&state);
        let writes: Vec<_> = sites
            .iter()
            .filter(|s| s.class == SiteClass::Write)
            .collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].step_path.as_deref(), Some("1"));
        assert_eq!(writes[0].receiver, "var");
        assert_eq!(writes[0].value_text.as_deref(), Some("false"));
        assert_eq!(writes[0].subject.anchor, "document-open-steps");
        assert!(sites.iter().any(|s| s.class == SiteClass::Unclassified
            && s.step_path.as_deref() == Some("3")
            && s.op == "add"));
        assert!(sites
            .iter()
            .any(|s| s.class == SiteClass::OpaqueWrite && s.op == "add"));
        let counts = derive_occurrence_counts(&state);
        assert!(counts
            .iter()
            .any(|(t, class, n)| t.anchor == "is-initial-about:blank"
                && *class == "read"
                && *n == 1));
        assert_eq!(state.coverage.written_fields, 1);
        assert_eq!(state.coverage.unclassified_review.len(), 1);
    }

    #[test]
    fn mini_spec_coverage_counters() {
        let coverage = extract(MINI, "HTML").coverage;
        assert_eq!(coverage.statements.get("set:to:field:var"), Some(&1));
        assert_eq!(
            coverage.statements.get("opaque:unsupported_form:add"),
            Some(&1)
        );
        assert_eq!((coverage.set_total, coverage.set_structured), (1, 1));
        let occurrences: Vec<_> = coverage
            .occurrences
            .iter()
            .map(|(class, n)| (class.as_str(), *n))
            .collect();
        assert_eq!(
            occurrences,
            [("read", 1), ("unclassified", 1), ("write", 1)]
        );
        assert_eq!(
            coverage.written_fields_owned.get("declaration_sentence"),
            Some(&1)
        );
        assert!(coverage.written_fields_unresolved.is_empty());
        let review = &coverage.unclassified_review[0];
        assert_eq!(
            (
                review.subject.as_str(),
                review.step_path.as_deref(),
                review.target.as_str()
            ),
            (
                "HTML#document-open-steps",
                Some("3"),
                "HTML#is-initial-about:blank"
            )
        );
    }

    #[test]
    fn site_parts_for_flags_mutations_and_initializers() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="run">run</dfn> given <var>d</var>:</p><ol>
<li><p>Unset <var>d</var>'s <a href="#done-flag">done</a> flag.</p></li>
<li><p><a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>x</var> to <var>d</var>'s <a href="#pending">pending list</a>.</p></li>
<li><p>Return a new <a href="#concept-node">node</a>, with its <a href="#concept-node-document">node document</a> set to <var>d</var>.</p></li></ol></div>"##;
        let sites = derive_sites(&extract(html, "HTML"));
        let site = |anchor: &str| {
            sites
                .iter()
                .find(|s| s.target.as_ref().is_some_and(|t| t.anchor == anchor))
                .unwrap_or_else(|| panic!("no site for {anchor}: {sites:#?}"))
        };
        let flag = site("done-flag");
        assert_eq!(
            (
                flag.op.as_str(),
                flag.target_text.as_str(),
                flag.value_text.as_deref()
            ),
            ("unset", "*d*'s done flag", None)
        );
        let append = site("pending");
        assert_eq!(
            (
                append.op.as_str(),
                append.receiver.as_str(),
                append.target_text.as_str(),
                append.value_text.as_deref()
            ),
            ("append", "var", "*d*'s pending list", Some("*x*"))
        );
        let init = site("concept-node-document");
        assert_eq!(
            (
                init.class,
                init.op.as_str(),
                init.receiver.as_str(),
                init.target_text.as_str(),
                init.value_text.as_deref()
            ),
            (SiteClass::Init, "init", "new", "node document", Some("*d*"))
        );
        assert_eq!(init.constructed.as_deref(), Some("HTML#concept-node"));
        assert!(sites.iter().all(|s| s.site_id.starts_with("site-")));
    }

    #[test]
    fn ecmarkup_and_empty_documents_do_not_panic() {
        let empty = extract("", "HTML");
        assert!(empty.model.fields.is_empty() && empty.sources.is_empty());
        let ecma = r#"<emu-clause id="sec-x" type="abstract operation"><h1><span class="secnum">7.1</span> X ( <var>O</var> )</h1><emu-alg><ol><li>Set <var>O</var>.[[Prototype]] to <emu-val>null</emu-val>.</li></ol></emu-alg></emu-clause>"#;
        let state = extract(ecma, "ECMA-262");
        assert!(state.statements.iter().any(|s| matches!(&s.kind, crate::state::ir::StatementKind::Set { targets, .. } if matches!(targets[0].hops[0], crate::state::ir::Hop::Slot { .. }))));
    }
}
