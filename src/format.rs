//! Markdown output formatters for CLI commands

use crate::effects::{Catalog, QueryWithEffects};
use crate::model::{
    AnchorsResult, ExistsResult, GraphResult, IdlResult, ListEntry, PrDiffResult, QueryResult,
    RefEntry, RefsResult, SearchResult, TraceDetail, TraceHop, TraceResult,
};

#[cfg(test)]
use crate::model::{AnchorEntry, SearchEntry};

use std::collections::HashMap;

/// Format a QueryResult as markdown
pub fn query(result: &QueryResult) -> String {
    let mut md = String::new();

    md.push_str(&format!("# {}#{}\n\n", result.spec, result.anchor));

    if let Some(title) = &result.title {
        md.push_str(&format!("**{}** ({})\n\n", title, result.section_type));
    } else {
        md.push_str(&format!("**Type**: {}\n\n", result.section_type));
    }

    md.push_str(&format!("**SHA**: {}\n\n", result.sha));

    if let Some(content) = &result.content {
        md.push_str("## Content\n\n");
        md.push_str(content);
        md.push_str("\n\n");
    }

    // Navigation
    md.push_str("## Navigation\n\n");
    if let Some(parent) = &result.navigation.parent {
        md.push_str(&format!(
            "- Parent: `{}`{}\n",
            parent.anchor,
            parent
                .title
                .as_deref()
                .map_or(String::new(), |t| format!(" — {}", t))
        ));
    }
    if let Some(prev) = &result.navigation.prev {
        md.push_str(&format!(
            "- Prev: `{}`{}\n",
            prev.anchor,
            prev.title
                .as_deref()
                .map_or(String::new(), |t| format!(" — {}", t))
        ));
    }
    if let Some(next) = &result.navigation.next {
        md.push_str(&format!(
            "- Next: `{}`{}\n",
            next.anchor,
            next.title
                .as_deref()
                .map_or(String::new(), |t| format!(" — {}", t))
        ));
    }
    if !result.navigation.children.is_empty() {
        md.push_str(&format!(
            "- Children: {}\n",
            result.navigation.children.len()
        ));
        for child in &result.navigation.children {
            md.push_str(&format!(
                "  - `{}`{}\n",
                child.anchor,
                child
                    .title
                    .as_deref()
                    .map_or(String::new(), |t| format!(" — {}", t))
            ));
        }
    }

    if !result.outgoing_refs.is_empty() {
        md.push_str(&format!(
            "\n## Outgoing refs ({})\n\n",
            result.outgoing_refs.len()
        ));
        for ref_entry in &result.outgoing_refs {
            md.push_str(&format!("- {}#{}\n", ref_entry.spec, ref_entry.anchor));
        }
    }

    if !result.incoming_refs.is_empty() {
        md.push_str(&format!(
            "\n## Incoming refs ({})\n\n",
            result.incoming_refs.len()
        ));
        for ref_entry in &result.incoming_refs {
            md.push_str(&format!("- {}#{}\n", ref_entry.spec, ref_entry.anchor));
        }
    }

    md
}

/// Format the additive query result using the shared effects renderer.
pub fn query_with_effects(result: &QueryWithEffects, catalog: Option<&Catalog>) -> String {
    let mut markdown = query(&result.query);
    if let Some(effects) = result.effects_result() {
        markdown.push('\n');
        markdown.push_str(&crate::effects::render::summary_markdown(&effects, catalog));
    }
    markdown
}

/// Format an ExistsResult as markdown
pub fn exists(result: &ExistsResult) -> String {
    if result.exists {
        format!(
            "{}#{} exists ({})\n",
            result.spec,
            result.anchor,
            result.section_type.as_deref().unwrap_or("unknown")
        )
    } else {
        format!("{}#{} not found\n", result.spec, result.anchor)
    }
}

/// Format an AnchorsResult as markdown
pub fn anchors(result: &AnchorsResult) -> String {
    let mut md = String::new();

    md.push_str(&format!("# Anchors matching `{}`\n\n", result.pattern));

    if result.results.is_empty() {
        md.push_str("No results.\n");
    } else {
        for entry in &result.results {
            md.push_str(&format!(
                "- **{}#{}**{} ({})\n",
                entry.spec,
                entry.anchor,
                entry
                    .title
                    .as_deref()
                    .map_or(String::new(), |t| format!(" — {}", t)),
                entry.section_type,
            ));
        }
    }

    md
}

/// Format a SearchResult as markdown
pub fn search(result: &SearchResult) -> String {
    let mut md = String::new();

    md.push_str(&format!("# Search: \"{}\"\n\n", result.query));

    if result.results.is_empty() {
        md.push_str("No results.\n");
    } else {
        for entry in &result.results {
            md.push_str(&format!(
                "### {}#{}{}\n\n",
                entry.spec,
                entry.anchor,
                entry
                    .title
                    .as_deref()
                    .map_or(String::new(), |t| format!(" — {}", t)),
            ));
            if !entry.snippet.is_empty() {
                md.push_str(&format!("{}\n\n", entry.snippet));
            }
        }
    }

    md
}

/// Format a list of headings as markdown (tree structure)
pub fn list(entries: &[ListEntry]) -> String {
    let mut md = String::new();

    for entry in entries {
        let indent = if entry.depth > 2 {
            "  ".repeat((entry.depth - 2) as usize)
        } else {
            String::new()
        };

        let number_prefix = entry
            .number
            .as_deref()
            .map_or(String::new(), |n| format!("{} ", n));

        md.push_str(&format!(
            "{}- `{}`{}\n",
            indent,
            entry.anchor,
            entry
                .title
                .as_deref()
                .map_or(String::new(), |t| format!(" — {}{}", number_prefix, t)),
        ));
    }

    md
}

/// One reference as a markdown bullet, with its call site when the index has one.
fn ref_entry_line(entry: &RefEntry) -> String {
    let mut line = format!("- {}#{}", entry.spec, entry.anchor);
    if let Some(step) = &entry.step_path {
        line.push_str(&format!(" — step {step}"));
    }
    if let Some(kind) = &entry.kind {
        if kind != "step" {
            line.push_str(&format!(" ({kind})"));
        }
    }
    line.push('\n');
    if let Some(text) = &entry.step_text {
        line.push_str(&format!("  > {text}\n"));
    }
    for guard in &entry.guard_path {
        line.push_str(&format!("  - under: {guard}\n"));
    }
    line
}

/// Format a RefsResult as markdown
pub fn refs(result: &RefsResult) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "# refs: `{}` ({})\n\n",
        result.query, result.direction
    ));

    if result.matches.is_empty() {
        md.push_str("No matches found in indexed specs.\n");
        return md;
    }

    for m in &result.matches {
        md.push_str(&format!(
            "## {}#{} ({}, {})\n\n",
            m.spec, m.anchor, m.section_type, m.resolution
        ));
        if let Some(title) = &m.title {
            md.push_str(&format!("Title: **{}**\n\n", title));
        }

        if let Some(incoming) = &m.incoming {
            md.push_str(&format!("Incoming: {}\n", incoming.len()));
            for r in incoming {
                md.push_str(&ref_entry_line(r));
            }
            md.push('\n');
        }

        if let Some(outgoing) = &m.outgoing {
            md.push_str(&format!("Outgoing: {}\n", outgoing.len()));
            for r in outgoing {
                md.push_str(&ref_entry_line(r));
            }
            md.push('\n');
        }
    }

    md
}

/// Format a TraceResult as markdown: one numbered chain per route, each hop
/// quoting the step that makes the call.
pub fn trace(result: &TraceResult, detail: TraceDetail) -> String {
    if detail == TraceDetail::Compact {
        return trace_compact(result);
    }
    trace_verbose(result)
}

/// Step numbers sort by segment, so 27.2 precedes 27.10 and both precede 28.
fn step_order(step: Option<&str>) -> Vec<u32> {
    step.unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Position of a call site within its section, from the spec generator's own
/// per-reference numbering: `section:target` is the first, `-2` the second and
/// so on. This is document order by construction, which step numbers only
/// approximate — a link in a late substep can carry its parent's step number.
fn call_site_order(id: Option<&str>) -> Option<u32> {
    let id = id?;
    match id.rsplit_once('-') {
        Some((_, suffix)) => suffix.parse().ok().or(Some(1)),
        None => Some(1),
    }
}

fn link(text: &str, url: Option<&str>) -> String {
    match url {
        Some(url) => format!("[{text}]({url})"),
        None => format!("`{text}`"),
    }
}

/// One line per hop, with every call site of an edge collapsed onto that line.
///
/// The search enumerates a separate route per call site, so an algorithm calling
/// another twelve times yields twelve routes identical but for one link. Grouping
/// by the anchor sequence puts them back together. That is lossless: which edge
/// reached a section does not constrain how the route continues from it, so the
/// call sites listed per hop recombine to exactly the routes found.
fn trace_compact(result: &TraceResult) -> String {
    let mut md = String::new();
    md.push_str(&format!("Spec trace: {} → {}\n\n", result.from, result.to));

    if result.traces.is_empty() {
        md.push_str(if result.truncated {
            "No route found before the search budget ran out.\n"
        } else {
            "No route exists within these bounds.\n"
        });
        return md;
    }

    // Group routes by the sections they pass through, keeping first-seen order.
    let mut order: Vec<Vec<(String, String)>> = Vec::new();
    let mut grouped: HashMap<Vec<(String, String)>, Vec<&Vec<TraceHop>>> = HashMap::new();
    for route in &result.traces {
        let key: Vec<(String, String)> = route
            .hops
            .iter()
            .map(|h| (h.anchor.clone(), h.to_anchor.clone()))
            .collect();
        if !grouped.contains_key(&key) {
            order.push(key.clone());
        }
        grouped.entry(key).or_default().push(&route.hops);
    }

    for (index, key) in order.iter().enumerate() {
        let routes = &grouped[key];
        if order.len() > 1 {
            md.push_str(&format!("## Route {}\n\n", index + 1));
        }

        for position in 0..key.len() {
            // Every call site seen at this position, in document order.
            let mut sites: Vec<&TraceHop> = routes.iter().map(|hops| &hops[position]).collect();
            sites.sort_by_key(|hop| {
                (
                    call_site_order(hop.call_site_id.as_deref()),
                    step_order(hop.step_path.as_deref()),
                )
            });
            sites.dedup_by(|a, b| a.call_site_id == b.call_site_id && a.step_path == b.step_path);

            let first = sites[0];
            let mut line = format!(
                "{}. {} calls {}",
                position + 1,
                link(&format!("#{}", first.anchor), first.url.as_deref()),
                link(
                    &format!("#{}", first.to_anchor),
                    first.call_site_url.as_deref()
                )
            );
            for (n, hop) in sites.iter().enumerate().skip(1) {
                line.push_str(&format!(
                    ", {}",
                    link(&format!("[{}]", n + 1), hop.call_site_url.as_deref())
                ));
            }
            md.push_str(&line);
            md.push_str("\n\n");
        }
    }

    md
}

fn trace_verbose(result: &TraceResult) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "# trace: `{}` -> `{}`\n\n",
        result.from, result.to
    ));
    md.push_str(&format!(
        "Max depth {}{}. Found {} trace(s).{}\n\n",
        result.max_depth,
        result
            .kind
            .as_ref()
            .map(|k| format!(", kind `{k}`"))
            .unwrap_or_default(),
        result.traces.len(),
        if result.truncated {
            " Search was truncated, so this list may be incomplete."
        } else {
            ""
        }
    ));

    if result.traces.is_empty() {
        md.push_str(if result.truncated {
            "No route found before the search budget ran out.\n"
        } else {
            "No route exists within these bounds.\n"
        });
        return md;
    }

    for (index, path) in result.traces.iter().enumerate() {
        md.push_str(&format!(
            "## Trace {} ({} hop(s))\n\n",
            index + 1,
            path.hops.len()
        ));
        for (position, hop) in path.hops.iter().enumerate() {
            // Link the calling step, not the callee's definition: the reader
            // needs to land where the call is made.
            let site = match (&hop.step_path, &hop.call_site_url) {
                (Some(step), Some(url)) => {
                    format!("[`{}#{}` step {step}]({url})", hop.spec, hop.anchor)
                }
                (Some(step), None) => format!("`{}#{}` step {step}", hop.spec, hop.anchor),
                (None, Some(url)) => format!("[`{}#{}`]({url})", hop.spec, hop.anchor),
                (None, None) => format!("`{}#{}`", hop.spec, hop.anchor),
            };
            md.push_str(&format!(
                "{}) {site} calls `{}#{}`\n",
                position + 1,
                hop.to_spec,
                hop.to_anchor
            ));
            for guard in &hop.guard_path {
                md.push_str(&format!("   - under: {guard}\n"));
            }
            if let Some(text) = &hop.step_text {
                md.push_str(&format!("   > {text}\n"));
            }
            md.push('\n');
        }
    }

    md
}

/// Format a GraphResult as markdown
pub fn graph(result: &GraphResult) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "# graph {}#{} ({})\n\n",
        result.root.spec, result.root.anchor, result.direction
    ));
    md.push_str(&format!(
        "Nodes: {} | Edges: {} | Max depth: {} | Truncated: {}\n\n",
        result.nodes.len(),
        result.edges.len(),
        result.max_depth,
        result.truncated
    ));

    md.push_str("## Nodes\n\n");
    for node in &result.nodes {
        md.push_str(&format!(
            "- `{}`{}\n",
            node.id,
            node.title
                .as_deref()
                .map_or(String::new(), |t| format!(" — {}", t))
        ));
    }

    md.push_str("\n## Edges\n\n");
    for edge in &result.edges {
        md.push_str(&format!(
            "- `{}` -> `{}` ({})\n",
            edge.from, edge.to, edge.kind
        ));
    }

    md
}

fn escape_mermaid_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_dot_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render a GraphResult as Mermaid flowchart.
pub fn graph_mermaid(result: &GraphResult) -> String {
    let mut out = String::from("graph TD\n");
    let mut ids = std::collections::HashMap::new();
    let mut bridge_nodes = Vec::new();
    let mut root_nodes = Vec::new();

    for (idx, node) in result.nodes.iter().enumerate() {
        let local_id = format!("n{}", idx);
        ids.insert(node.id.clone(), local_id.clone());
        let label = if let Some(title) = &node.title {
            format!("{}<br>{}", node.id, title.replace('\n', "<br>"))
        } else {
            node.id.clone()
        };
        out.push_str(&format!(
            "  {}[\"{}\"]\n",
            local_id,
            escape_mermaid_label(&label)
        ));

        if let Some(role) = &node.filter_role {
            match role.as_str() {
                "bridge" => bridge_nodes.push(local_id.clone()),
                "root" => root_nodes.push(local_id.clone()),
                _ => {}
            }
        }
    }

    for edge in &result.edges {
        if let (Some(from), Some(to)) = (ids.get(&edge.from), ids.get(&edge.to)) {
            out.push_str(&format!("  {} --> {}\n", from, to));
        }
    }

    if !bridge_nodes.is_empty() {
        out.push_str("  classDef bridge stroke-dasharray: 5 5\n");
        out.push_str(&format!("  class {} bridge\n", bridge_nodes.join(",")));
    }
    if !root_nodes.is_empty() {
        out.push_str("  classDef root stroke-width: 3px\n");
        out.push_str(&format!("  class {} root\n", root_nodes.join(",")));
    }

    out
}

/// Render a GraphResult as Graphviz DOT.
pub fn graph_dot(result: &GraphResult) -> String {
    let mut out = String::from("digraph webspec {\n  rankdir=LR;\n");

    for node in &result.nodes {
        let escaped_id = escape_dot_label(&node.id);
        let label = if let Some(title) = &node.title {
            format!("{}\\n{}", escaped_id, escape_dot_label(title))
        } else {
            escaped_id.clone()
        };
        out.push_str(&format!("  \"{}\" [label=\"{}\"];\n", escaped_id, label));
    }

    for edge in &result.edges {
        out.push_str(&format!(
            "  \"{}\" -> \"{}\";\n",
            escape_dot_label(&edge.from),
            escape_dot_label(&edge.to)
        ));
    }

    out.push_str("}\n");
    out
}

/// Format an IdlResult as markdown
pub fn idl(result: &IdlResult) -> String {
    let mut md = String::new();
    md.push_str(&format!("# IDL: `{}`\n\n", result.query));

    if result.matches.is_empty() {
        md.push_str("No IDL matches found.\n");
        return md;
    }

    for entry in &result.matches {
        md.push_str(&format!("## {} ({})\n\n", entry.canonical_name, entry.kind));
        md.push_str(&format!("- Anchor: `{}#{}`\n", entry.spec, entry.anchor));
        if let Some(owner) = &entry.owner {
            md.push_str(&format!("- Owner: `{}`\n", owner));
        }
        md.push_str(&format!("- Name: `{}`\n", entry.name));
        if let Some(title) = &entry.title {
            md.push_str(&format!("- Title: {}\n", title));
        }
        if let Some(idl_text) = &entry.idl_text {
            md.push_str("\n```webidl\n");
            md.push_str(idl_text);
            md.push_str("\n```\n");
        }
        md.push('\n');
    }

    md
}

/// Format a PrDiffResult as markdown with unified diffs per section
pub fn pr_diff(result: &PrDiffResult) -> String {
    use similar::TextDiff;

    let mut out = String::new();
    out.push_str(&format!(
        "# {} PR #{} diff\n\nHead: `{}` | Base: `{}`\n\n",
        result.spec, result.pr_number, result.head_sha, result.merge_base_sha
    ));
    out.push_str(&format!(
        "**Summary:** {} added, {} removed, {} modified\n\n",
        result.summary.added, result.summary.removed, result.summary.modified
    ));

    for change in &result.changes {
        let title = change.title.as_deref().unwrap_or("(untitled)");
        let icon = match change.change_type.as_str() {
            "added" => "+",
            "removed" => "-",
            "modified" => "~",
            _ => "?",
        };
        out.push_str(&format!(
            "## {}{} `#{}` — {}\n\n",
            icon, change.change_type, change.anchor, title
        ));

        match change.change_type.as_str() {
            "added" => {
                if let Some(content) = &change.new_content {
                    out.push_str("```\n");
                    out.push_str(content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n\n");
                }
            }
            "removed" => {
                if let Some(content) = &change.old_content {
                    out.push_str("```\n");
                    out.push_str(content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n\n");
                }
            }
            "modified" => {
                let old = change.old_content.as_deref().unwrap_or("");
                let new = change.new_content.as_deref().unwrap_or("");
                let diff = TextDiff::from_lines(old, new);
                out.push_str("```diff\n");
                for hunk in diff.unified_diff().context_radius(2).iter_hunks() {
                    out.push_str(&format!("{hunk}"));
                }
                out.push_str("```\n\n");
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        NavEntry, Navigation, PrDiffEntry, PrDiffResult, PrDiffSummary, RefEntry, RefsMatch,
    };

    #[test]
    fn test_query_format_minimal() {
        let result = QueryResult {
            spec: "TEST".to_string(),
            sha: "abc123".to_string(),
            anchor: "test-section".to_string(),
            url: String::new(),
            title: None,
            number: None,
            content: None,
            section_type: "Heading".to_string(),
            navigation: Navigation {
                parent: None,
                prev: None,
                next: None,
                children: vec![],
            },
            outgoing_refs: vec![],
            incoming_refs: vec![],
        };

        let md = query(&result);
        assert!(md.contains("# TEST#test-section"));
        assert!(md.contains("**Type**: Heading"));
        assert!(md.contains("**SHA**: abc123"));
        assert!(md.contains("## Navigation"));
    }

    #[test]
    fn test_query_format_with_content() {
        let result = QueryResult {
            spec: "TEST".to_string(),
            sha: "abc123".to_string(),
            anchor: "navigate".to_string(),
            url: String::new(),
            title: Some("navigate".to_string()),
            number: None,
            content: Some("To **navigate** a [navigable](#foo)".to_string()),
            section_type: "Algorithm".to_string(),
            navigation: Navigation {
                parent: Some(NavEntry {
                    anchor: "section-7".to_string(),
                    title: None,
                    number: None,
                }),
                prev: None,
                next: None,
                children: vec![],
            },
            outgoing_refs: vec![],
            incoming_refs: vec![],
        };

        let md = query(&result);
        assert!(md.contains("**navigate** (Algorithm)"));
        assert!(md.contains("## Content"));
        assert!(md.contains("To **navigate** a [navigable](#foo)"));
        assert!(md.contains("- Parent: `section-7`"));
    }

    #[test]
    fn test_query_format_with_refs() {
        let result = QueryResult {
            spec: "TEST".to_string(),
            sha: "abc123".to_string(),
            anchor: "foo".to_string(),
            url: String::new(),
            title: None,
            number: None,
            content: None,
            section_type: "Definition".to_string(),
            navigation: Navigation {
                parent: None,
                prev: None,
                next: None,
                children: vec![
                    NavEntry {
                        anchor: "child1".to_string(),
                        title: Some("First Child".to_string()),
                        number: None,
                    },
                    NavEntry {
                        anchor: "child2".to_string(),
                        title: None,
                        number: None,
                    },
                ],
            },
            outgoing_refs: vec![RefEntry::plain("OTHER".to_string(), "bar".to_string())],
            incoming_refs: vec![RefEntry::plain("ANOTHER".to_string(), "baz".to_string())],
        };

        let md = query(&result);
        assert!(md.contains("- Children: 2"));
        assert!(md.contains("  - `child1` — First Child"));
        assert!(md.contains("  - `child2`"));
        assert!(md.contains("## Outgoing refs (1)"));
        assert!(md.contains("- OTHER#bar"));
        assert!(md.contains("## Incoming refs (1)"));
        assert!(md.contains("- ANOTHER#baz"));
    }

    #[test]
    fn test_exists_true() {
        let result = ExistsResult {
            exists: true,
            spec: "HTML".to_string(),
            anchor: "navigate".to_string(),
            section_type: Some("Algorithm".to_string()),
        };
        let md = exists(&result);
        assert_eq!(md, "HTML#navigate exists (Algorithm)\n");
    }

    #[test]
    fn test_exists_false() {
        let result = ExistsResult {
            exists: false,
            spec: "DOM".to_string(),
            anchor: "missing".to_string(),
            section_type: None,
        };
        let md = exists(&result);
        assert_eq!(md, "DOM#missing not found\n");
    }

    #[test]
    fn test_anchors_format() {
        let result = AnchorsResult {
            pattern: "*-tree".to_string(),
            results: vec![
                AnchorEntry {
                    spec: "DOM".to_string(),
                    anchor: "concept-tree".to_string(),
                    title: Some("tree".to_string()),
                    section_type: "Definition".to_string(),
                },
                AnchorEntry {
                    spec: "HTML".to_string(),
                    anchor: "document-tree".to_string(),
                    title: None,
                    section_type: "Definition".to_string(),
                },
            ],
        };

        let md = anchors(&result);
        assert!(md.contains("# Anchors matching `*-tree`"));
        assert!(md.contains("- **DOM#concept-tree** — tree (Definition)"));
        assert!(md.contains("- **HTML#document-tree** (Definition)"));
    }

    #[test]
    fn test_search_format() {
        let result = SearchResult {
            query: "tree order".to_string(),
            results: vec![SearchEntry {
                spec: "DOM".to_string(),
                anchor: "concept-tree-order".to_string(),
                title: Some("tree order".to_string()),
                section_type: "Definition".to_string(),
                snippet: "An object A is before an object B in <mark>tree order</mark>..."
                    .to_string(),
            }],
        };

        let md = search(&result);
        assert!(md.contains("# Search: \"tree order\""));
        assert!(md.contains("### DOM#concept-tree-order — tree order"));
        assert!(md.contains("An object A is before"));
    }

    #[test]
    fn test_list_format() {
        let entries = vec![
            ListEntry {
                anchor: "intro".to_string(),
                title: Some("Introduction".to_string()),
                depth: 2,
                parent: None,
                number: None,
            },
            ListEntry {
                anchor: "algorithms".to_string(),
                title: Some("Algorithms".to_string()),
                depth: 3,
                parent: Some("intro".to_string()),
                number: None,
            },
        ];

        let md = list(&entries);
        assert!(md.contains("- `intro` — Introduction"));
        assert!(md.contains("  - `algorithms` — Algorithms")); // depth 3 gets 1 level indent
    }

    #[test]
    fn test_list_format_empty() {
        let md = list(&[]);
        assert_eq!(md, "");
    }

    #[test]
    fn test_refs_format_both_directions() {
        let result = RefsResult {
            query: "HTML#navigate".to_string(),
            direction: "both".to_string(),
            matches: vec![RefsMatch {
                spec: "HTML".to_string(),
                anchor: "navigate".to_string(),
                title: None,
                section_type: "algorithm".to_string(),
                resolution: "exact".to_string(),
                outgoing: Some(vec![
                    RefEntry::plain("URL".to_string(), "concept-url".to_string()),
                    RefEntry::plain("INFRA".to_string(), "assert".to_string()),
                ]),
                incoming: Some(vec![RefEntry::plain(
                    "HTML".to_string(),
                    "navigate-fragid".to_string(),
                )]),
            }],
        };

        let md = refs(&result);
        assert!(md.contains("# refs: `HTML#navigate`"));
        assert!(md.contains("## HTML#navigate"));
        assert!(md.contains("Outgoing: 2"));
        assert!(md.contains("- URL#concept-url"));
        assert!(md.contains("- INFRA#assert"));
        assert!(md.contains("Incoming: 1"));
        assert!(md.contains("- HTML#navigate-fragid"));
    }

    #[test]
    fn test_refs_format_no_matches() {
        let result = RefsResult {
            query: "HTML#orphan".to_string(),
            direction: "both".to_string(),
            matches: vec![],
        };

        let md = refs(&result);
        assert!(md.contains("# refs: `HTML#orphan`"));
        assert!(md.contains("No matches found"));
    }

    #[test]
    fn test_trace_markdown_renders_a_trace() {
        use crate::model::{Trace, TraceHop};

        let result = TraceResult {
            from: "HTML#assign".to_string(),
            to: "HTML#checking".to_string(),
            max_depth: 6,
            kind: Some("step".to_string()),
            traces: vec![Trace {
                hops: vec![TraceHop {
                    spec: "HTML".to_string(),
                    anchor: "navigate".to_string(),
                    to_spec: "HTML".to_string(),
                    to_anchor: "checking".to_string(),
                    step_path: Some("24.1".to_string()),
                    step_text: Some("Let unloadPromptCanceled be the result.".to_string()),
                    guard_path: vec!["In parallel, run these steps:".to_string()],
                    call_site_id: None,
                    call_site_url: None,
                    url: None,
                }],
            }],
            truncated: false,
        };

        let md = trace(&result, TraceDetail::Verbose);
        assert!(md.contains("# trace: `HTML#assign` -> `HTML#checking`"));
        assert!(md.contains("1) `HTML#navigate` step 24.1 calls `HTML#checking`"));
        assert!(md.contains("   - under: In parallel, run these steps:"));
        assert!(md.contains("   > Let unloadPromptCanceled be the result."));
        assert!(!md.contains("truncated"));
    }

    #[test]
    fn test_compact_collapses_repeated_call_sites_onto_one_line() {
        use crate::model::{Trace, TraceHop};

        // An algorithm calling another three times is three routes differing only
        // in which link was followed. Compact puts them back together.
        let hop = |step: &str, site: u32| TraceHop {
            spec: "HTML".to_string(),
            anchor: "update-the-image-data".to_string(),
            to_spec: "HTML".to_string(),
            to_anchor: "abort-the-image-request".to_string(),
            step_path: Some(step.to_string()),
            step_text: None,
            guard_path: vec![],
            call_site_id: Some(format!("updating:abort-{site}")),
            call_site_url: Some(format!(
                "https://html.spec.whatwg.org/#updating:abort-{site}"
            )),
            url: Some("https://html.spec.whatwg.org/#update-the-image-data".to_string()),
        };

        let result = TraceResult {
            from: "HTML#update-the-image-data".to_string(),
            to: "HTML#abort-the-image-request".to_string(),
            max_depth: 2,
            kind: Some("step".to_string()),
            // Deliberately out of order: 27.10 must not sort before 7.4.2.
            traces: vec![
                Trace {
                    hops: vec![hop("27.10", 3)],
                },
                Trace {
                    hops: vec![hop("2", 1)],
                },
                Trace {
                    hops: vec![hop("7.4.2", 2)],
                },
            ],
            truncated: false,
        };

        let md = trace(&result, TraceDetail::Compact);

        assert!(
            md.starts_with("Spec trace: HTML#update-the-image-data → HTML#abort-the-image-request")
        );
        assert_eq!(
            md.matches("1. ").count(),
            1,
            "three routes collapse to one hop line: {md}"
        );
        assert!(
            md.contains(
                "1. [#update-the-image-data](https://html.spec.whatwg.org/#update-the-image-data) calls [#abort-the-image-request](https://html.spec.whatwg.org/#updating:abort-1), [[2]](https://html.spec.whatwg.org/#updating:abort-2), [[3]](https://html.spec.whatwg.org/#updating:abort-3)"
            ),
            "call sites list in document order, the first on the callee name: {md}"
        );
    }

    #[test]
    fn test_compact_keeps_distinct_routes_apart() {
        use crate::model::{Trace, TraceHop};

        let hop = |anchor: &str, to: &str| TraceHop {
            spec: "HTML".to_string(),
            anchor: anchor.to_string(),
            to_spec: "HTML".to_string(),
            to_anchor: to.to_string(),
            step_path: Some("1".to_string()),
            step_text: None,
            guard_path: vec![],
            call_site_id: None,
            call_site_url: None,
            url: None,
        };

        let result = TraceResult {
            from: "HTML#a".to_string(),
            to: "HTML#z".to_string(),
            max_depth: 3,
            kind: Some("step".to_string()),
            traces: vec![
                Trace {
                    hops: vec![hop("a", "b"), hop("b", "z")],
                },
                Trace {
                    hops: vec![hop("a", "c"), hop("c", "z")],
                },
            ],
            truncated: false,
        };

        let md = trace(&result, TraceDetail::Compact);
        assert!(md.contains("## Route 1") && md.contains("## Route 2"));
        assert!(md.contains("#b") && md.contains("#c"));
    }

    #[test]
    fn test_quiet_keeps_the_step_and_link_but_drops_the_prose() {
        use crate::model::{Trace, TraceHop};

        let mut result = TraceResult {
            from: "HTML#a".to_string(),
            to: "HTML#checking".to_string(),
            max_depth: 6,
            kind: Some("step".to_string()),
            traces: vec![Trace {
                hops: vec![TraceHop {
                    spec: "HTML".to_string(),
                    anchor: "navigate".to_string(),
                    to_spec: "HTML".to_string(),
                    to_anchor: "checking".to_string(),
                    step_path: Some("24.1".to_string()),
                    step_text: Some("Let unloadPromptCanceled be the result.".to_string()),
                    guard_path: vec!["In parallel, run these steps:".to_string()],
                    call_site_id: Some("beginning-navigation:checking-2".to_string()),
                    call_site_url: Some(
                        "https://html.spec.whatwg.org/#beginning-navigation:checking-2".to_string(),
                    ),
                    url: None,
                }],
            }],
            truncated: false,
        };

        result.apply_detail(TraceDetail::Edges);
        let md = trace(&result, TraceDetail::Edges);

        assert!(
            md.contains("1) `HTML#navigate` step 24.1 calls `HTML#checking`"),
            "quiet keeps the slug and step number, which feed back into query/refs: {md}"
        );
        assert!(
            !md.contains("unloadPromptCanceled"),
            "step text must be gone"
        );
        assert!(!md.contains("under:"), "guards must be gone");
        assert!(
            !md.contains("https://"),
            "quiet drops URLs; SPEC#anchor is the identifier"
        );
    }

    #[test]
    fn test_trace_markdown_links_the_call_site() {
        use crate::model::{Trace, TraceHop};

        let result = TraceResult {
            from: "HTML#a".to_string(),
            to: "HTML#checking".to_string(),
            max_depth: 6,
            kind: Some("step".to_string()),
            traces: vec![Trace {
                hops: vec![TraceHop {
                    spec: "HTML".to_string(),
                    anchor: "navigate".to_string(),
                    to_spec: "HTML".to_string(),
                    to_anchor: "checking".to_string(),
                    step_path: Some("24.1".to_string()),
                    step_text: None,
                    guard_path: vec![],
                    call_site_id: Some("beginning-navigation:checking-2".to_string()),
                    call_site_url: Some(
                        "https://html.spec.whatwg.org/#beginning-navigation:checking-2".to_string(),
                    ),
                    url: None,
                }],
            }],
            truncated: false,
        };

        let md = trace(&result, TraceDetail::Verbose);
        assert!(
            md.contains(
                "1) [`HTML#navigate` step 24.1](https://html.spec.whatwg.org/#beginning-navigation:checking-2) calls `HTML#checking`"
            ),
            "the hop must link to the call site, not the callee: {md}"
        );
    }

    #[test]
    fn test_trace_markdown_distinguishes_empty_from_truncated() {
        let empty = TraceResult {
            from: "HTML#a".to_string(),
            to: "HTML#b".to_string(),
            max_depth: 6,
            kind: None,
            traces: vec![],
            truncated: false,
        };
        assert!(
            trace(&empty, TraceDetail::Verbose).contains("No route exists within these bounds.")
        );

        let cut_short = TraceResult {
            truncated: true,
            ..empty
        };
        assert!(trace(&cut_short, TraceDetail::Verbose).contains("budget ran out"));
    }

    #[test]
    fn test_pr_diff_markdown_shows_unified_diff() {
        let result = PrDiffResult {
            spec: "HTML".to_string(),
            pr_number: 123,
            head_sha: "pr:123:abc".to_string(),
            merge_base_sha: "def456".to_string(),
            summary: PrDiffSummary {
                added: 1,
                removed: 0,
                modified: 1,
            },
            changes: vec![
                PrDiffEntry {
                    anchor: "sec-a".to_string(),
                    title: Some("Section A".to_string()),
                    change_type: "modified".to_string(),
                    old_content: Some("Line one\nLine two\nLine three".to_string()),
                    new_content: Some("Line one\nLine TWO modified\nLine three".to_string()),
                },
                PrDiffEntry {
                    anchor: "sec-b".to_string(),
                    title: Some("Section B".to_string()),
                    change_type: "added".to_string(),
                    old_content: None,
                    new_content: Some("Brand new content".to_string()),
                },
            ],
        };

        let md = pr_diff(&result);
        assert!(md.contains("## ~modified `#sec-a` — Section A"));
        assert!(md.contains("-Line two"));
        assert!(md.contains("+Line TWO modified"));
        assert!(md.contains("## +added `#sec-b` — Section B"));
        assert!(md.contains("Brand new content"));
    }
}
