//! Control-flow extraction for algorithm sections.
//!
//! Parses the stored markdown (numbered steps, nested via 4-space indentation)
//! together with outgoing `refs` rows that carry a `step_path` to produce a
//! `FlowResult` whose nodes and edges a client can render as a flowchart.
//!
//! Compiles with `--no-default-features` (no tokio/reqwest); only rusqlite,
//! pulldown-cmark, regex, and serde are used.

use std::collections::{HashMap, HashSet};

/// Spec/anchor/step_path triple for a call site entry.
type CallEntry = (String, String, Option<String>);

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use rusqlite::Connection;

use crate::{db, model};

// ─── Parsed step tree ────────────────────────────────────────────────────────

/// One step in the algorithm's ordered-list tree.
#[derive(Debug)]
struct StepItem {
    id: String,
    text: String,
    children: Vec<StepItem>,
}

// ─── Step parsing ─────────────────────────────────────────────────────────────

/// Parse the top-level ordered list from algorithm markdown.
/// Prose before the first ordered list is ignored.
/// Lists inside block-quotes are not treated as steps.
fn parse_steps(markdown: &str) -> Vec<StepItem> {
    let events: Vec<Event<'_>> = Parser::new_ext(markdown, Options::empty()).collect();

    // Find the first top-level ordered list.
    let mut i = 0;
    while i < events.len() {
        if matches!(&events[i], Event::Start(Tag::List(Some(_)))) {
            break;
        }
        i += 1;
    }
    if i >= events.len() {
        return vec![];
    }

    let (items, _) = parse_ordered_list(&events, i, &[]);
    items
}

/// Parse an ordered list starting at `start` (which points to `Start(List(Some(_)))`).
/// `parent_ids` is the list of ancestor ordinals (1-based) for the step-id prefix.
/// Returns `(items, index_after_end)`.
fn parse_ordered_list<'a>(
    events: &[Event<'a>],
    start: usize,
    parent_ids: &[u32],
) -> (Vec<StepItem>, usize) {
    let start_num: u64 = match &events[start] {
        Event::Start(Tag::List(Some(n))) => *n,
        _ => 1,
    };
    let mut items = Vec::new();
    let mut i = start + 1;
    let mut item_num = start_num as u32;

    loop {
        match events.get(i) {
            Some(Event::Start(Tag::Item)) => {
                let (item, after) = parse_item(events, i, parent_ids, item_num);
                items.push(item);
                item_num += 1;
                i = after;
            }
            Some(Event::End(TagEnd::List(true))) => {
                return (items, i + 1);
            }
            None => return (items, i),
            _ => i += 1,
        }
    }
}

/// Parse one list item starting at `start` (which points to `Start(Item)`).
/// Returns `(item, index_after_end_item)`.
fn parse_item<'a>(
    events: &[Event<'a>],
    start: usize,
    parent_ids: &[u32],
    item_num: u32,
) -> (StepItem, usize) {
    let mut my_ids = parent_ids.to_vec();
    my_ids.push(item_num);
    let id = my_ids
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".");

    let mut text = String::new();
    let mut children: Vec<StepItem> = Vec::new();
    let mut i = start + 1; // move past Start(Item)

    // skip_depth > 0 while we are inside a region we are skipping
    // (unordered lists, block-quotes, ordered lists inside block-quotes).
    let mut skip_depth: u32 = 0;
    let mut in_blockquote: u32 = 0;

    loop {
        match events.get(i) {
            // ── done ──────────────────────────────────────────────────────────
            Some(Event::End(TagEnd::Item)) if skip_depth == 0 => {
                return (
                    StepItem {
                        id,
                        text: normalise_text(&text),
                        children,
                    },
                    i + 1,
                );
            }

            // ── ordered sub-list (not inside a blockquote) ────────────────────
            Some(Event::Start(Tag::List(Some(_)))) if skip_depth == 0 && in_blockquote == 0 => {
                let (sub, after) = parse_ordered_list(events, i, &my_ids);
                // Only keep the first (outermost) nested ordered list as children.
                if children.is_empty() {
                    children = sub;
                }
                i = after;
            }

            // ── block-quote: track but skip ───────────────────────────────────
            Some(Event::Start(Tag::BlockQuote(_))) if skip_depth == 0 => {
                in_blockquote += 1;
                skip_depth += 1;
                i += 1;
            }
            Some(Event::End(TagEnd::BlockQuote(_))) if skip_depth > 0 => {
                in_blockquote = in_blockquote.saturating_sub(1);
                skip_depth -= 1;
                i += 1;
            }

            // ── unordered list: skip ──────────────────────────────────────────
            Some(Event::Start(Tag::List(None))) if skip_depth == 0 => {
                skip_depth += 1;
                i += 1;
            }
            Some(Event::End(TagEnd::List(false))) if skip_depth > 0 && in_blockquote == 0 => {
                skip_depth -= 1;
                i += 1;
            }

            // ── generic depth tracking while skipping ─────────────────────────
            Some(Event::Start(_)) if skip_depth > 0 => {
                skip_depth += 1;
                i += 1;
            }
            Some(Event::End(_)) if skip_depth > 0 => {
                skip_depth -= 1;
                i += 1;
            }

            // ── text collection ───────────────────────────────────────────────
            Some(Event::Text(t)) if skip_depth == 0 => {
                text.push_str(t);
                i += 1;
            }
            Some(Event::Code(c)) if skip_depth == 0 => {
                text.push_str(c);
                i += 1;
            }
            Some(Event::SoftBreak) if skip_depth == 0 => {
                text.push(' ');
                i += 1;
            }

            None => {
                return (
                    StepItem {
                        id,
                        text: normalise_text(&text),
                        children,
                    },
                    i,
                );
            }

            _ => {
                i += 1;
            }
        }
    }
}

/// Collapse whitespace and truncate to 120 chars.
fn normalise_text(raw: &str) -> String {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 120 {
        let cut: String = collapsed.chars().take(120).collect();
        format!("{cut}…")
    } else {
        collapsed
    }
}

// ─── Shape detection ──────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum StepShape {
    IfBlock,             // "If …:" with children
    IfInline,            // "If …, then …" without block children
    Otherwise,           // "Otherwise[, if …][:…]"
    Return,              // "Return …"
    Abort,               // "Abort these steps" / "Abort this algorithm"
    JumpTo(String),      // "Jump to step N" / "return to step N"
    Parallel,            // "In parallel …" / "… in parallel"
    Loop,                // "For each …:" / "While …:"
    UnrecognisedControl, // Continue, Break, Switch on
    Plain,               // everything else
}

struct Patterns {
    re_jump: Regex,
    re_return_to: Regex,
    re_abort: Regex,
    re_return: Regex,
    re_otherwise: Regex,
    re_foreach: Regex,
    re_while: Regex,
    re_parallel_start: Regex,
    re_parallel_end: Regex,
    re_if_block: Regex,
    re_if_inline: Regex,
    re_unrecognised: Regex,
}

impl Patterns {
    fn new() -> Self {
        Patterns {
            re_jump: Regex::new(r"(?i)^jump\s+to\s+step\s+(\d+)").unwrap(),
            re_return_to: Regex::new(r"(?i)^return\s+to\s+step\s+(\d+)").unwrap(),
            re_abort: Regex::new(r"(?i)^abort\s+(these\s+steps|this\s+algorithm)\b").unwrap(),
            re_return: Regex::new(r"(?i)^return\b").unwrap(),
            re_otherwise: Regex::new(r"(?i)^otherwise\b").unwrap(),
            re_foreach: Regex::new(r"(?i)^for\s+each\b").unwrap(),
            re_while: Regex::new(r"(?i)^while\b").unwrap(),
            re_parallel_start: Regex::new(r"(?i)^in\s+parallel[,\s]").unwrap(),
            re_parallel_end: Regex::new(r"(?i)\bin\s+parallel\b").unwrap(),
            re_if_block: Regex::new(r"(?i)^if\b").unwrap(),
            re_if_inline: Regex::new(r"(?i)^if\b.*,\s*then\b").unwrap(),
            re_unrecognised: Regex::new(r"(?i)^(continue|break|switch\s+on)\b").unwrap(),
        }
    }

    fn detect(&self, text: &str, has_children: bool) -> StepShape {
        // Jump / return-to-step must come before Return.
        if let Some(caps) = self.re_jump.captures(text) {
            return StepShape::JumpTo(caps[1].to_string());
        }
        if let Some(caps) = self.re_return_to.captures(text) {
            return StepShape::JumpTo(caps[1].to_string());
        }

        if self.re_abort.is_match(text) {
            return StepShape::Abort;
        }
        if self.re_return.is_match(text) {
            return StepShape::Return;
        }
        if self.re_otherwise.is_match(text) {
            return StepShape::Otherwise;
        }
        if self.re_foreach.is_match(text) || self.re_while.is_match(text) {
            return StepShape::Loop;
        }
        if self.re_parallel_start.is_match(text) || self.re_parallel_end.is_match(text) {
            return StepShape::Parallel;
        }
        if self.re_if_block.is_match(text) {
            if has_children {
                return StepShape::IfBlock;
            }
            if self.re_if_inline.is_match(text) {
                return StepShape::IfInline;
            }
            // "If …" with no children and no ", then" → plain
        }
        if self.re_unrecognised.is_match(text) {
            return StepShape::UnrecognisedControl;
        }
        StepShape::Plain
    }
}

fn shape_to_kind(shape: &StepShape) -> model::FlowNodeKind {
    match shape {
        StepShape::IfBlock | StepShape::IfInline | StepShape::Otherwise => {
            model::FlowNodeKind::Branch
        }
        StepShape::Return | StepShape::Abort => model::FlowNodeKind::Terminal,
        StepShape::JumpTo(_) => model::FlowNodeKind::Step,
        StepShape::Parallel => model::FlowNodeKind::Parallel,
        StepShape::Loop => model::FlowNodeKind::Loop,
        StepShape::UnrecognisedControl | StepShape::Plain => model::FlowNodeKind::Step,
    }
}

// ─── Edge building ────────────────────────────────────────────────────────────

/// How the last step of a block continues after that block ends.
#[derive(Debug, Clone)]
enum Continuation {
    Next(String),
    Loop(String), // last substep loops back to this node
}

struct BuildCtx<'a> {
    nodes: &'a mut Vec<model::FlowNode>,
    edges: &'a mut Vec<model::FlowEdge>,
    issues: &'a mut Vec<model::FlowIssue>,
    external_ids: &'a mut HashSet<String>,
    calls_by_path: &'a HashMap<String, Vec<CallEntry>>,
    top_level_ids: &'a [String],
    patterns: &'a Patterns,
}

fn push_edge(
    edges: &mut Vec<model::FlowEdge>,
    from: &str,
    to: &str,
    kind: model::FlowEdgeKind,
    label: Option<String>,
) {
    edges.push(model::FlowEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind,
        label,
    });
}

fn apply_continuation(edges: &mut Vec<model::FlowEdge>, from: &str, cont: &Option<Continuation>) {
    if let Some(c) = cont {
        match c {
            Continuation::Next(id) => {
                push_edge(edges, from, id, model::FlowEdgeKind::Next, None);
            }
            Continuation::Loop(id) => {
                push_edge(edges, from, id, model::FlowEdgeKind::Loop, None);
            }
        }
    }
}

/// Build nodes and edges for a flat list of step items.
/// `last_branch_id` tracks the most-recent Branch sibling for `Otherwise` resolution.
fn build_list(
    items: &[StepItem],
    parent_continuation: Option<Continuation>,
    last_branch_id: &mut Option<String>,
    ctx: &mut BuildCtx<'_>,
) {
    for (i, item) in items.iter().enumerate() {
        let item_continuation: Option<Continuation> = if i + 1 < items.len() {
            Some(Continuation::Next(items[i + 1].id.clone()))
        } else {
            parent_continuation.clone()
        };
        build_item(item, item_continuation, last_branch_id, ctx);
    }
}

fn build_item(
    item: &StepItem,
    continuation: Option<Continuation>,
    last_branch_id: &mut Option<String>,
    ctx: &mut BuildCtx<'_>,
) {
    let shape = ctx.patterns.detect(&item.text, !item.children.is_empty());
    let kind = shape_to_kind(&shape);

    // Collect calls for this step.
    let calls: Vec<model::FlowCall> = ctx
        .calls_by_path
        .get(&item.id)
        .map(|v| {
            v.iter()
                .map(|(spec, anchor, sp)| model::FlowCall {
                    spec: spec.clone(),
                    anchor: anchor.clone(),
                    step_path: sp.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    ctx.nodes.push(model::FlowNode {
        id: item.id.clone(),
        kind,
        text: item.text.clone(),
        calls: calls.clone(),
    });

    // Add external call nodes and call edges.
    for call in &calls {
        let ext_id = format!("{}#{}", call.spec, call.anchor);
        if ctx.external_ids.insert(ext_id.clone()) {
            ctx.nodes.push(model::FlowNode {
                id: ext_id.clone(),
                kind: model::FlowNodeKind::External,
                text: ext_id.clone(),
                calls: vec![],
            });
        }
        push_edge(
            ctx.edges,
            &item.id,
            &ext_id,
            model::FlowEdgeKind::Call,
            None,
        );
    }

    match shape {
        // ── If …: (with substeps) ─────────────────────────────────────────────
        StepShape::IfBlock => {
            if let Some(first) = item.children.first() {
                push_edge(
                    ctx.edges,
                    &item.id,
                    &first.id,
                    model::FlowEdgeKind::Then,
                    None,
                );
            }
            let mut sub_last_branch: Option<String> = None;
            build_list(&item.children, continuation, &mut sub_last_branch, ctx);
            *last_branch_id = Some(item.id.clone());
        }

        // ── If …, then … (inline, no substeps) ───────────────────────────────
        StepShape::IfInline => {
            if let Some(cont) = &continuation {
                match cont {
                    Continuation::Next(id) => {
                        push_edge(ctx.edges, &item.id, id, model::FlowEdgeKind::Then, None);
                    }
                    Continuation::Loop(id) => {
                        push_edge(ctx.edges, &item.id, id, model::FlowEdgeKind::Then, None);
                    }
                }
            }
            *last_branch_id = Some(item.id.clone());
        }

        // ── Otherwise[, if …]: ───────────────────────────────────────────────
        StepShape::Otherwise => {
            match last_branch_id.take() {
                Some(branch_id) => {
                    push_edge(
                        ctx.edges,
                        &branch_id,
                        &item.id,
                        model::FlowEdgeKind::Else,
                        None,
                    );
                }
                None => {
                    ctx.issues.push(model::FlowIssue {
                        step: item.id.clone(),
                        code: "otherwise_without_if".to_string(),
                        message: format!(
                            "Otherwise at step {} has no preceding If branch",
                            item.id
                        ),
                    });
                }
            }
            if !item.children.is_empty() {
                push_edge(
                    ctx.edges,
                    &item.id,
                    &item.children[0].id,
                    model::FlowEdgeKind::Then,
                    None,
                );
                let mut sub_last_branch: Option<String> = None;
                build_list(&item.children, continuation, &mut sub_last_branch, ctx);
            } else {
                apply_continuation(ctx.edges, &item.id, &continuation);
            }
            *last_branch_id = Some(item.id.clone());
        }

        // ── Return / Abort ────────────────────────────────────────────────────
        StepShape::Return | StepShape::Abort => {
            // Terminal: no outgoing step edges.
            *last_branch_id = None;
        }

        // ── Jump to step N ────────────────────────────────────────────────────
        StepShape::JumpTo(ref label) => {
            let target = resolve_jump(label, ctx.top_level_ids);
            match target {
                Some(id) => push_edge(ctx.edges, &item.id, &id, model::FlowEdgeKind::Jump, None),
                None => {
                    ctx.issues.push(model::FlowIssue {
                        step: item.id.clone(),
                        code: "unresolved_jump".to_string(),
                        message: format!("Jump target '{}' not found", label),
                    });
                }
            }
            *last_branch_id = None;
        }

        // ── For each … / While … ─────────────────────────────────────────────
        StepShape::Loop => {
            if let Some(first) = item.children.first() {
                push_edge(
                    ctx.edges,
                    &item.id,
                    &first.id,
                    model::FlowEdgeKind::Then,
                    None,
                );
            }
            // Children loop back to the Loop node; pass Loop continuation so
            // the last child emits a Loop edge rather than Next.
            let loop_cont = Some(Continuation::Loop(item.id.clone()));
            let mut sub_last_branch: Option<String> = None;
            build_list(&item.children, loop_cont, &mut sub_last_branch, ctx);
            // The loop node itself continues to the following sibling.
            apply_continuation(ctx.edges, &item.id, &continuation);
            *last_branch_id = None;
        }

        // ── In parallel ───────────────────────────────────────────────────────
        StepShape::Parallel => {
            // Add a `then` edge to every top-level substep (all run simultaneously).
            for child in &item.children {
                push_edge(
                    ctx.edges,
                    &item.id,
                    &child.id,
                    model::FlowEdgeKind::Then,
                    None,
                );
            }
            // Each substep's subtree has no continuation (they run in parallel
            // and don't chain into each other).
            let mut sub_last_branch: Option<String> = None;
            build_list(&item.children, None, &mut sub_last_branch, ctx);
            // The Parallel node itself continues to the following sibling.
            apply_continuation(ctx.edges, &item.id, &continuation);
            *last_branch_id = None;
        }

        // ── Unrecognised control flow ─────────────────────────────────────────
        StepShape::UnrecognisedControl => {
            ctx.issues.push(model::FlowIssue {
                step: item.id.clone(),
                code: "unrecognised_control".to_string(),
                message: format!("Unrecognised control flow: {}", item.text),
            });
            if let Some(cont) = &continuation {
                match cont {
                    Continuation::Next(id) | Continuation::Loop(id) => {
                        push_edge(ctx.edges, &item.id, id, model::FlowEdgeKind::Unknown, None);
                    }
                }
            }
            *last_branch_id = None;
        }

        // ── Plain step ────────────────────────────────────────────────────────
        StepShape::Plain => {
            apply_continuation(ctx.edges, &item.id, &continuation);
            *last_branch_id = None;
        }
    }
}

/// Resolve a jump-target label (a step number string like "5" or "5.2") against
/// the top-level step ids.
fn resolve_jump(label: &str, top_level_ids: &[String]) -> Option<String> {
    // Try exact match in top-level ids first.
    if let Some(id) = top_level_ids.iter().find(|id| id.as_str() == label) {
        return Some(id.clone());
    }
    // Try numeric prefix match (e.g. label "5" matches "5" in top-level).
    None
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Extract control-flow nodes, edges, and issues from algorithm markdown.
///
/// `calls` is a slice of outgoing `RefEntry` rows with `kind == "step"` and a
/// `step_path`; each entry is attached to the node whose id equals `step_path`.
pub fn extract_flow(
    markdown: &str,
    calls: &[model::RefEntry],
) -> (
    Vec<model::FlowNode>,
    Vec<model::FlowEdge>,
    Vec<model::FlowIssue>,
) {
    let items = parse_steps(markdown);
    if items.is_empty() {
        return (vec![], vec![], vec![]);
    }

    // Build calls_by_path: step_path → [(spec, anchor, step_path)]
    let mut calls_by_path: HashMap<String, Vec<CallEntry>> = HashMap::new();
    for entry in calls {
        if let Some(ref sp) = entry.step_path {
            if entry.kind.as_deref() == Some("step") {
                calls_by_path.entry(sp.clone()).or_default().push((
                    entry.spec.clone(),
                    entry.anchor.clone(),
                    entry.step_path.clone(),
                ));
            }
        }
    }

    let top_level_ids: Vec<String> = items.iter().map(|it| it.id.clone()).collect();

    let mut nodes: Vec<model::FlowNode> = Vec::new();
    let mut edges: Vec<model::FlowEdge> = Vec::new();
    let mut issues: Vec<model::FlowIssue> = Vec::new();
    let mut external_ids: HashSet<String> = HashSet::new();
    let patterns = Patterns::new();

    let mut ctx = BuildCtx {
        nodes: &mut nodes,
        edges: &mut edges,
        issues: &mut issues,
        external_ids: &mut external_ids,
        calls_by_path: &calls_by_path,
        top_level_ids: &top_level_ids,
        patterns: &patterns,
    };

    let mut last_branch_id: Option<String> = None;
    build_list(&items, None, &mut last_branch_id, &mut ctx);

    (nodes, edges, issues)
}

/// Query the database for a section's markdown and outgoing step-refs, then
/// run `extract_flow`.
///
/// Returns `Ok(None)` when the section is not of type `algorithm` or has no
/// top-level ordered list.
pub fn flow_from_conn(
    conn: &Connection,
    spec: &str,
    anchor: &str,
) -> anyhow::Result<Option<model::FlowResult>> {
    let Some(snapshot_id) = db::queries::get_snapshot(conn, spec)? else {
        anyhow::bail!("spec {} is not indexed", spec);
    };

    let Some(section) = db::queries::get_section(conn, snapshot_id, anchor)? else {
        return Ok(None);
    };

    if section.section_type != model::SectionType::Algorithm {
        return Ok(None);
    }

    let content = match section.content_text {
        Some(c) => c,
        None => return Ok(None),
    };

    // Fetch outgoing step-refs (those with a step_path are algorithm invocations).
    let ref_edges =
        db::queries::get_outgoing_edges(conn, snapshot_id, anchor, Some(model::RefKind::Step))?;
    let calls: Vec<model::RefEntry> = ref_edges
        .into_iter()
        .filter(|e| e.step_path.is_some())
        .map(|e| model::RefEntry {
            spec: e.spec,
            anchor: e.anchor,
            step_path: e.step_path,
            step_text: e.step_text,
            guard_path: e.guard_path,
            call_site_id: e.call_site_id,
            kind: Some("step".to_string()),
        })
        .collect();

    let (nodes, edges, issues) = extract_flow(&content, &calls);

    if nodes.is_empty() {
        return Ok(None);
    }

    Ok(Some(model::FlowResult {
        spec: spec.to_string(),
        anchor: anchor.to_string(),
        nodes,
        edges,
        issues,
    }))
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        md: &str,
    ) -> (
        Vec<model::FlowNode>,
        Vec<model::FlowEdge>,
        Vec<model::FlowIssue>,
    ) {
        extract_flow(md, &[])
    }

    fn node_kinds(nodes: &[model::FlowNode]) -> Vec<(&str, model::FlowNodeKind)> {
        nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect()
    }

    fn edge_triples(edges: &[model::FlowEdge]) -> Vec<(&str, &str, model::FlowEdgeKind)> {
        edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.kind))
            .collect()
    }

    // ── Table row: plain step ─────────────────────────────────────────────────

    #[test]
    fn plain_step_gets_next_edge() {
        let (nodes, edges, issues) = run("1. Do A.\n2. Do B.\n");
        assert_eq!(issues, vec![]);
        let kinds = node_kinds(&nodes);
        assert_eq!(kinds[0], ("1", model::FlowNodeKind::Step));
        assert_eq!(kinds[1], ("2", model::FlowNodeKind::Step));
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Next)));
    }

    // ── Table row: If …: with substeps ────────────────────────────────────────

    #[test]
    fn if_block_gets_then_edge_into_first_substep() {
        let md = "1. If condition:\n   1. Do A.\n   2. Do B.\n2. Next.\n";
        let (nodes, edges, issues) = run(md);
        assert_eq!(issues, vec![]);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Branch);
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "1.1", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("1.1", "1.2", model::FlowEdgeKind::Next)));
        assert!(e.contains(&("1.2", "2", model::FlowEdgeKind::Next)));
    }

    // ── Table row: If …, then … (inline) ─────────────────────────────────────

    #[test]
    fn if_inline_gets_then_edge_to_following_sibling() {
        let md = "1. If A, then return null.\n2. Continue.\n";
        let (nodes, edges, _) = run(md);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Branch);
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Then)));
    }

    // ── Table row: Otherwise ─────────────────────────────────────────────────

    #[test]
    fn otherwise_gets_else_edge_from_preceding_branch() {
        let md = "1. If A:\n   1. Do X.\n2. Otherwise:\n   1. Do Y.\n3. Done.\n";
        let (nodes, edges, issues) = run(md);
        assert_eq!(issues, vec![]);
        assert_eq!(nodes[2].kind, model::FlowNodeKind::Branch); // "2"
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Else)));
        assert!(e.contains(&("2", "2.1", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("2.1", "3", model::FlowEdgeKind::Next)));
    }

    // ── Table row: Return ─────────────────────────────────────────────────────

    #[test]
    fn return_is_terminal_with_no_edges() {
        let md = "1. Return null.\n2. Should not be reached.\n";
        let (nodes, edges, _) = run(md);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Terminal);
        let from_1: Vec<_> = edges.iter().filter(|e| e.from == "1").collect();
        assert!(from_1.is_empty(), "Return node must have no outgoing edges");
    }

    // ── Table row: Abort these steps ─────────────────────────────────────────

    #[test]
    fn abort_is_terminal() {
        let md = "1. Abort these steps.\n";
        let (nodes, _, _) = run(md);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Terminal);
    }

    // ── Table row: For each / While ───────────────────────────────────────────

    #[test]
    fn for_each_loop_edges() {
        let md = "1. For each item:\n   1. Process item.\n2. Done.\n";
        let (nodes, edges, issues) = run(md);
        assert_eq!(issues, vec![]);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Loop);
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "1.1", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("1.1", "1", model::FlowEdgeKind::Loop)));
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Next)));
    }

    // ── Table row: In parallel ────────────────────────────────────────────────

    #[test]
    fn parallel_then_to_each_substep() {
        let md = "1. In parallel, run these steps:\n   1. Do A.\n   2. Do B.\n2. Join.\n";
        let (nodes, edges, issues) = run(md);
        assert_eq!(issues, vec![]);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Parallel);
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "1.1", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("1", "1.2", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Next)));
    }

    // ── Unrecognised control flow ─────────────────────────────────────────────

    #[test]
    fn unrecognised_control_yields_issue_and_unknown_edge() {
        let md = "1. Continue.\n2. Next step.\n";
        let (nodes, edges, issues) = run(md);
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Step);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "unrecognised_control");
        let e = edge_triples(&edges);
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Unknown)));
    }

    // ── Otherwise after branch whose last substep is Return ───────────────────

    #[test]
    fn otherwise_after_branch_with_return_last_substep() {
        let md = concat!(
            "1. If condition:\n",
            "   1. Return null.\n",
            "2. Otherwise:\n",
            "   1. Do something.\n",
            "3. Done.\n",
        );
        let (nodes, edges, issues) = run(md);
        assert_eq!(issues, vec![]);

        // Node kinds
        assert_eq!(nodes[0].kind, model::FlowNodeKind::Branch); // "1" If
        assert_eq!(nodes[1].kind, model::FlowNodeKind::Terminal); // "1.1" Return
        assert_eq!(nodes[2].kind, model::FlowNodeKind::Branch); // "2" Otherwise
        assert_eq!(nodes[3].kind, model::FlowNodeKind::Step); // "2.1"
        assert_eq!(nodes[4].kind, model::FlowNodeKind::Step); // "3"

        let e = edge_triples(&edges);

        // The branch's fall-through (else) still reaches Otherwise even though
        // the last substep of the branch is a Return.
        assert!(e.contains(&("1", "2", model::FlowEdgeKind::Else)));
        // 1.1 is Terminal — no outgoing edges.
        let from_1_1: Vec<_> = edges.iter().filter(|e| e.from == "1.1").collect();
        assert!(
            from_1_1.is_empty(),
            "Return must have no outgoing edges: {from_1_1:?}"
        );

        // Otherwise → 2.1, then 2.1 → 3
        assert!(e.contains(&("2", "2.1", model::FlowEdgeKind::Then)));
        assert!(e.contains(&("2.1", "3", model::FlowEdgeKind::Next)));
    }

    // ── Prose before the list is ignored ─────────────────────────────────────

    #[test]
    fn prose_before_list_is_ignored() {
        let md = "Some introductory text.\n\n1. Step A.\n2. Step B.\n";
        let (nodes, _, _) = run(md);
        assert_eq!(nodes[0].id, "1");
        assert_eq!(nodes[1].id, "2");
    }

    // ── Text truncation ───────────────────────────────────────────────────────

    #[test]
    fn long_step_text_is_truncated() {
        let long = "word ".repeat(30);
        let md = format!("1. {long}\n");
        let (nodes, _, _) = run(&md);
        assert!(nodes[0].text.ends_with('…'));
        assert!(nodes[0].text.chars().count() <= 121); // 120 chars + "…"
    }

    // ── otherwise_without_if issue ────────────────────────────────────────────

    #[test]
    fn otherwise_without_preceding_if_yields_issue() {
        let md = "1. Otherwise:\n   1. Do something.\n";
        let (_, _, issues) = run(md);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "otherwise_without_if");
    }
}
