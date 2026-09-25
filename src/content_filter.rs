//! Post-processing transformations for stored content markdown.
//!
//! Content is stored in the database as pre-rendered markdown produced by the
//! htmd converter. All link targets are embedded as full absolute URLs; this
//! module rewrites them to the compact `SPEC#anchor` form (or strips link
//! markup entirely) and optionally removes Note / Example / Warning / Issue
//! advisement blockquotes.
//!
//! # Why post-processing
//!
//! Link targets are not stored separately from the markdown. The full URL is
//! embedded in the standard `[text](url)` syntax, so `SpecRegistry::resolve_url`
//! can recover the target spec from the stored string. Rewriting uses
//! pulldown-cmark's offset iterator, so links inside code blocks and code spans
//! are never touched.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::spec_registry::SpecRegistry;

/// How to render hyperlinks that appear in content markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinksMode {
    /// Rewrite known-spec URLs to `SPEC#anchor` format; keep others as-is.
    #[default]
    Short,
    /// Keep full absolute URLs.
    Full,
    /// Emit link text only; strip all bracket and URL markup.
    None,
}

/// Advisement block label strings that identify note/example/warning/issue blockquotes.
///
/// These are the first bold-text labels inside the blockquote (`**Note:**` etc.).
const NOTE_LABELS: &[&str] = &["Note:", "Example:", "Warning:", "Issue:"];

/// Apply link-rendering and note-stripping transformations to a content markdown string.
///
/// When both `links == Full` and `no_notes == false` the string is returned unchanged.
pub fn transform_content(
    markdown: &str,
    links: LinksMode,
    no_notes: bool,
    registry: &SpecRegistry,
) -> String {
    if links == LinksMode::Full && !no_notes {
        return markdown.to_string();
    }

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let all_events: Vec<(Event<'_>, std::ops::Range<usize>)> = Parser::new_ext(markdown, options)
        .into_offset_iter()
        .collect();

    // (start, end, replacement) — applied back-to-front.
    let mut changes: Vec<(usize, usize, String)> = Vec::new();

    // Collect note blockquote ranges first so the link pass can skip them.
    let note_ranges: Vec<std::ops::Range<usize>> = if no_notes {
        collect_note_ranges(&all_events, markdown)
    } else {
        Vec::new()
    };

    for r in &note_ranges {
        changes.push((r.start, r.end, String::new()));
    }

    // Collect link replacements.
    if links != LinksMode::Full {
        collect_link_changes(
            &all_events,
            markdown,
            links,
            registry,
            &note_ranges,
            &mut changes,
        );
    }

    // Sort back-to-front so string replacement doesn't shift earlier positions.
    changes.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));

    let mut result = markdown.to_string();
    for (start, end, replacement) in changes {
        result.replace_range(start..end, &replacement);
    }

    // Collapse any triple-newlines left by note removal into double-newlines.
    if no_notes {
        while result.contains("\n\n\n") {
            result = result.replace("\n\n\n", "\n\n");
        }
        // Trim leading/trailing whitespace that note removal may expose.
        let trimmed = result.trim_matches('\n');
        result = trimmed.to_string();
        if !result.is_empty() {
            result.push('\n');
        }
    }

    result
}

/// Return the byte ranges of advisement blockquotes whose first bold-text label
/// is one of `NOTE_LABELS` (`Note:`, `Example:`, `Warning:`, `Issue:`).
fn collect_note_ranges(
    events: &[(Event<'_>, std::ops::Range<usize>)],
    markdown: &str,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut i = 0;

    while i < events.len() {
        if matches!(&events[i].0, Event::Start(Tag::BlockQuote(_))) {
            let bq_start = events[i].1.start;
            let mut depth = 0usize;
            let mut label_found = false;
            let mut first_text_checked = false;
            let mut bq_end = events[i].1.end;

            let mut j = i;
            loop {
                if j >= events.len() {
                    break;
                }
                let (ev, ev_range) = &events[j];
                match ev {
                    Event::Start(Tag::BlockQuote(_)) => depth += 1,
                    Event::End(TagEnd::BlockQuote(_)) => {
                        depth -= 1;
                        if depth == 0 {
                            bq_end = ev_range.end;
                            break;
                        }
                    }
                    // The first Text event at depth 1 (directly inside our blockquote)
                    // carries the label string (e.g. "Note:" from `**Note:**`).
                    Event::Text(text) if depth == 1 && !first_text_checked => {
                        first_text_checked = true;
                        let t = text.as_ref().trim();
                        if NOTE_LABELS.contains(&t) {
                            label_found = true;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }

            if label_found {
                // Extend past a trailing newline if present so we don't leave a
                // stray blank line.
                let mut end = bq_end;
                if markdown.as_bytes().get(end) == Some(&b'\n') {
                    end += 1;
                }
                ranges.push(bq_start..end);
            }
        }
        i += 1;
    }

    ranges
}

/// Collect link replacements into `changes`, skipping links inside code blocks,
/// code spans, and note blockquote ranges that will be deleted.
fn collect_link_changes(
    events: &[(Event<'_>, std::ops::Range<usize>)],
    markdown: &str,
    links: LinksMode,
    registry: &SpecRegistry,
    note_ranges: &[std::ops::Range<usize>],
    changes: &mut Vec<(usize, usize, String)>,
) {
    let mut in_code_block = false;
    let mut link_depth = 0usize;
    let mut link_start: Option<usize> = None;
    let mut link_url: Option<String> = None;

    for (event, range) in events {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,

            Event::Start(Tag::Link { dest_url, .. }) if !in_code_block => {
                if link_depth == 0 {
                    link_start = Some(range.start);
                    link_url = Some(dest_url.to_string());
                }
                link_depth += 1;
            }

            Event::End(TagEnd::Link) if !in_code_block => {
                link_depth = link_depth.saturating_sub(1);
                if link_depth == 0 {
                    if let (Some(start), Some(url)) = (link_start.take(), link_url.take()) {
                        let end = range.end;

                        // Skip links whose source range falls inside a note
                        // blockquote that is being deleted.
                        if note_ranges
                            .iter()
                            .any(|r| r.start <= start && start < r.end)
                        {
                            continue;
                        }

                        let link_source = &markdown[start..end];
                        if let Some(repl) =
                            compute_link_replacement(link_source, &url, links, registry)
                        {
                            changes.push((start, end, repl));
                        }
                    }
                }
            }

            _ => {}
        }
    }
}

/// Compute the replacement string for a single markdown link.
///
/// Returns `None` when no change is needed (unknown spec in `Short` mode, or
/// the `](url)` pattern is not found in the source — safe no-op default).
fn compute_link_replacement(
    link_source: &str,
    url: &str,
    links: LinksMode,
    registry: &SpecRegistry,
) -> Option<String> {
    // Find the position of `](url)` which marks where the link text ends.
    let close_with_url = format!("]({})", url);
    let bracket_end = link_source.rfind(&close_with_url)?;
    // `bracket_end` is the byte index of `]` in `link_source`.

    match links {
        LinksMode::Full => None,
        LinksMode::Short => {
            let (spec, anchor) = registry.resolve_url(url)?;
            let new_url = format!("{spec}#{anchor}");
            // Preserve "[text]", replace "(old_url)" with "(new_url)".
            Some(format!("{}({new_url})", &link_source[..bracket_end + 1]))
        }
        LinksMode::None => {
            // Strip link markup; keep only the text between `[` and `]`.
            if link_source.starts_with('[') && bracket_end > 0 {
                Some(link_source[1..bracket_end].to_string())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SpecRegistry {
        SpecRegistry::new()
    }

    // --- LinksMode::Short ---

    #[test]
    fn short_rewrites_in_spec_link() {
        let md = "[navigate](https://html.spec.whatwg.org/#navigate)";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(out, "[navigate](HTML#navigate)", "in-spec link");
    }

    #[test]
    fn short_rewrites_cross_spec_link() {
        let md = "[tree](https://dom.spec.whatwg.org/#concept-tree)";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(out, "[tree](DOM#concept-tree)", "cross-spec link");
    }

    #[test]
    fn short_leaves_unknown_spec_link_unchanged() {
        let md = "[foo](https://example.com/#bar)";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(
            out, "[foo](https://example.com/#bar)",
            "unknown-spec link must be unchanged"
        );
    }

    #[test]
    fn short_skips_link_inside_inline_code_span() {
        // A backtick code span is never parsed as a link by pulldown-cmark.
        let md = "`[foo](https://html.spec.whatwg.org/#foo)`";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(out, md, "link inside inline code must not be rewritten");
    }

    #[test]
    fn short_skips_links_inside_fenced_code_block() {
        let md = "```\n[foo](https://html.spec.whatwg.org/#foo)\n```";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(out, md, "link inside code block must not be rewritten");
    }

    #[test]
    fn short_preserves_complex_link_text() {
        let md = "[`navigate`](https://html.spec.whatwg.org/#navigate)";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert_eq!(out, "[`navigate`](HTML#navigate)");
    }

    #[test]
    fn short_rewrites_multiple_links_in_paragraph() {
        let md = "See [navigate](https://html.spec.whatwg.org/#navigate) and [tree](https://dom.spec.whatwg.org/#concept-tree).";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert!(
            out.contains("[navigate](HTML#navigate)"),
            "first link: {out}"
        );
        assert!(
            out.contains("[tree](DOM#concept-tree)"),
            "second link: {out}"
        );
    }

    // --- LinksMode::None ---

    #[test]
    fn none_strips_link_markup_plain_text() {
        let md = "[navigate](https://html.spec.whatwg.org/#navigate)";
        let out = transform_content(md, LinksMode::None, false, &registry());
        assert_eq!(out, "navigate");
    }

    #[test]
    fn none_strips_link_markup_code_text() {
        let md = "[`navigate`](https://html.spec.whatwg.org/#navigate)";
        let out = transform_content(md, LinksMode::None, false, &registry());
        assert_eq!(out, "`navigate`");
    }

    #[test]
    fn none_leaves_code_span_links_unchanged() {
        let md = "`[foo](https://html.spec.whatwg.org/#foo)`";
        let out = transform_content(md, LinksMode::None, false, &registry());
        assert_eq!(out, md, "literal code span must not be touched");
    }

    // --- no_notes ---

    #[test]
    fn no_notes_removes_note_blockquote() {
        let md = "> **Note:** This is a note.\n\nFollowing paragraph.";
        let out = transform_content(md, LinksMode::Full, true, &registry());
        assert!(!out.contains("Note:"), "note block must be removed: {out}");
        assert!(
            out.contains("Following paragraph."),
            "surrounding content kept: {out}"
        );
    }

    #[test]
    fn no_notes_removes_example_blockquote() {
        let md = "> **Example:** Here is an example.\n\nText.";
        let out = transform_content(md, LinksMode::Full, true, &registry());
        assert!(!out.contains("Example:"), "example block removed: {out}");
        assert!(out.contains("Text."), "surrounding content kept: {out}");
    }

    #[test]
    fn no_notes_removes_warning_blockquote() {
        let md = "> **Warning:** Be careful.\n\nText after.";
        let out = transform_content(md, LinksMode::Full, true, &registry());
        assert!(!out.contains("Warning:"), "warning block removed: {out}");
        assert!(
            out.contains("Text after."),
            "surrounding content kept: {out}"
        );
    }

    #[test]
    fn no_notes_keeps_non_note_blockquote() {
        let md = "> Regular blockquote text.\n\nParagraph.";
        let out = transform_content(md, LinksMode::Full, true, &registry());
        assert!(
            out.contains("Regular blockquote text."),
            "plain quote must stay: {out}"
        );
    }

    #[test]
    fn no_notes_combined_with_short_links() {
        let md = "> **Note:** See [navigate](https://html.spec.whatwg.org/#navigate).\n\nSee [tree](https://dom.spec.whatwg.org/#concept-tree).";
        let out = transform_content(md, LinksMode::Short, true, &registry());
        assert!(!out.contains("Note:"), "note removed");
        assert!(
            !out.contains("https://html.spec.whatwg.org"),
            "note's links gone too"
        );
        assert!(
            out.contains("[tree](DOM#concept-tree)"),
            "link outside note rewritten: {out}"
        );
    }

    // --- Nested list steps (sanity) ---

    #[test]
    fn nested_list_steps_preserved() {
        let md = "1. Let *x* be [something](https://html.spec.whatwg.org/#something).\n   1. Sub-step.\n2. Return.";
        let out = transform_content(md, LinksMode::Short, false, &registry());
        assert!(out.contains("1. Let *x* be"), "list structure preserved");
        assert!(
            out.contains("[something](HTML#something)"),
            "link rewritten"
        );
        assert!(out.contains("Sub-step."), "nested list preserved");
    }

    // --- Full mode is a no-op ---

    #[test]
    fn full_mode_returns_content_unchanged() {
        let md = "[navigate](https://html.spec.whatwg.org/#navigate)\n> **Note:** text.";
        let out = transform_content(md, LinksMode::Full, false, &registry());
        assert_eq!(out, md);
    }
}
