//! Cuts a slice out of the stored section markdown (spec §8): omitted runs of step items become
//! one marker line each, everything else is copied byte for byte.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::Serialize;

use super::{OmittedRun, Slice, SliceIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rendering {
    Aligned,
    /// The markdown's step items differ from the index's steps; `content` is the full markdown.
    Unaligned,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedView {
    pub content: String,
    pub rendering: Rendering,
    /// Ordered items outside blockquotes found in the markdown.
    pub markdown_items: usize,
}

struct StepItem {
    path: String,
    start: usize,
    end: usize,
}

/// Ordered-list items outside blockquotes, in document order, with their ordinal paths.
fn step_items(markdown: &str) -> Vec<StepItem> {
    let mut items: Vec<StepItem> = Vec::new();
    let mut quote_depth = 0usize;
    // (ordered, items seen so far)
    let mut lists: Vec<(bool, usize)> = Vec::new();
    // Per open item: its index in `items` when it is a counted step.
    let mut open: Vec<Option<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(markdown, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote_depth -= 1,
            Event::Start(Tag::List(start)) => lists.push((start.is_some(), 0)),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let Some((ordered, seen)) = lists.last_mut() else {
                    open.push(None);
                    continue;
                };
                *seen += 1;
                if !*ordered || quote_depth > 0 {
                    open.push(None);
                    continue;
                }
                let ordinal = *seen;
                let path = match open.iter().rev().find_map(|o| *o) {
                    Some(parent) => format!("{}.{ordinal}", items[parent].path),
                    None => ordinal.to_string(),
                };
                open.push(Some(items.len()));
                items.push(StepItem {
                    path,
                    start: range.start,
                    end: range.end,
                });
            }
            Event::End(TagEnd::Item) => {
                open.pop();
            }
            _ => {}
        }
    }
    items
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The start of the first non-blank line from `at` on (from the next line when `at` is mid-line), or
/// the end of the text.
fn next_content_line(text: &str, at: usize) -> usize {
    let mut pos = at;
    if pos > 0 && text.as_bytes()[pos - 1] != b'\n' {
        pos = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
    }
    while pos < text.len() {
        let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
        if !text[pos..line_end].trim().is_empty() {
            break;
        }
        pos = line_end;
    }
    pos
}

fn last_component(path: &str) -> u32 {
    path.rsplit('.')
        .next()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

/// `[step P omitted: REASON]` or `[steps P–Q omitted (N steps): REASON]` (spec §8.2).
pub fn marker_text(run: &OmittedRun) -> String {
    let siblings = last_component(&run.last).saturating_sub(last_component(&run.first)) + 1;
    let steps = if run.first == run.last {
        format!("step {}", run.first)
    } else {
        format!("steps {}\u{2013}{}", run.first, run.last)
    };
    let count = if run.steps > siblings {
        format!(" ({} steps)", run.steps)
    } else {
        String::new()
    };
    format!("[{steps} omitted{count}: {}]", run.reason)
}

/// Replaces each omitted run of `slice` with a marker line, when the markdown's ordered items match
/// the index's step paths; otherwise returns the markdown unchanged as `Unaligned`.
pub fn render_view(markdown: &str, index: &SliceIndex, slice: &Slice) -> RenderedView {
    let items = step_items(markdown);
    let unaligned = |markdown_items| RenderedView {
        content: markdown.to_string(),
        rendering: Rendering::Unaligned,
        markdown_items,
    };
    let aligned = items.len() == index.steps.len()
        && items
            .iter()
            .zip(&index.steps)
            .all(|(item, step)| item.path == step.path);
    if !aligned {
        return unaligned(items.len());
    }

    // Aligned items and index steps correspond by position.
    let mut replacements: Vec<(usize, usize, String)> = Vec::with_capacity(slice.omitted.len());
    for run in &slice.omitted {
        let (Some(first), Some(last)) = (items.get(run.positions.0), items.get(run.positions.1))
        else {
            return unaligned(items.len());
        };
        let start = line_start(markdown, first.start);
        // An item nested in a container may start inside the indentation before its list marker.
        let item = &markdown[first.start..];
        let item_marker = first.start + item.len() - item.trim_start_matches([' ', '\t']).len();
        let end = next_content_line(markdown, last.end);
        let mut marker = format!("{}- {}", &markdown[start..item_marker], marker_text(run));
        if end < markdown.len() {
            marker.push_str("\n\n");
        } else if markdown[..end].ends_with('\n') {
            marker.push('\n');
        }
        replacements.push((start, end, marker));
    }

    let mut content = String::with_capacity(markdown.len());
    let mut copied = 0;
    for (start, end, marker) in &replacements {
        content.push_str(&markdown[copied..*start]);
        content.push_str(marker);
        copied = *end;
    }
    content.push_str(&markdown[copied..]);
    RenderedView {
        content,
        rendering: Rendering::Aligned,
        markdown_items: items.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::slice::select::{slice, ViewRequest};
    use crate::state::slice::OmittedRun;
    use crate::state::testing::slice_index;

    const MD: &str =
        "To **go**:\n\n1. A *x*.\n2. B.\n3. C:\n\n    1. D *x*.\n    2. E.\n\n4. F.\n5. G *x*.\n";

    fn index() -> crate::state::slice::SliceIndex {
        slice_index(
            "go",
            &[
                ("1", &["x"]),
                ("2", &[]),
                ("3", &[]),
                ("3.1", &["x"]),
                ("3.2", &[]),
                ("4", &[]),
                ("5", &["x"]),
            ],
            &[],
            &[],
        )
    }

    fn involving_x() -> ViewRequest {
        ViewRequest {
            involving: vec!["x".into()],
            ..Default::default()
        }
    }

    #[test]
    fn markers_replace_runs_at_their_indentation() {
        let index = index();
        let s = slice(&index, &involving_x()).unwrap();
        let view = render_view(MD, &index, &s);
        assert_eq!(view.rendering, Rendering::Aligned);
        assert_eq!(view.markdown_items, 7);
        assert_eq!(
            view.content,
            "To **go**:\n\n1. A *x*.\n- [step 2 omitted: no use of *x*]\n\n3. C:\n\n    1. D *x*.\n    - [step 3.2 omitted: no use of *x*]\n\n- [step 4 omitted: no use of *x*]\n\n5. G *x*.\n"
        );
    }

    #[test]
    fn rendered_numbers_stay_the_specs_numbers() {
        let index = index();
        let s = slice(&index, &involving_x()).unwrap();
        let mut html = String::new();
        pulldown_cmark::html::push_html(
            &mut html,
            pulldown_cmark::Parser::new(&render_view(MD, &index, &s).content),
        );
        assert!(html.contains("<ol start=\"3\">"), "{html}");
        assert!(html.contains("<ol start=\"5\">"), "{html}");
    }

    #[test]
    fn kept_items_keep_bullets_and_notes_and_no_notes_applies_afterwards() {
        let md = "To **go**:\n\n1. A *x*:\n\n    * bullet one\n    * bullet two\n\n    > **Note:** kept note.\n\n2. B.\n\n    > **Note:** dropped note.\n\n3. C *x*.\n";
        let index = slice_index("go", &[("1", &["x"]), ("2", &[]), ("3", &["x"])], &[], &[]);
        let s = slice(&index, &involving_x()).unwrap();
        let view = render_view(md, &index, &s);
        assert_eq!(
            view.content,
            "To **go**:\n\n1. A *x*:\n\n    * bullet one\n    * bullet two\n\n    > **Note:** kept note.\n\n- [step 2 omitted: no use of *x*]\n\n3. C *x*.\n"
        );
        let registry = crate::spec_registry::SpecRegistry::new();
        let stripped = crate::content_filter::transform_content(
            &view.content,
            crate::content_filter::LinksMode::Full,
            true,
            &registry,
        );
        assert!(
            !stripped.contains("kept note")
                && stripped.contains("bullet two")
                && stripped.contains("[step 2 omitted")
        );
    }

    #[test]
    fn ordered_lists_inside_blockquotes_are_not_steps() {
        let md = "1. A *x*.\n\n    > **Note:**\n    >\n    > 1. first\n    > 2. second\n\n2. B.\n";
        let index = slice_index("go", &[("1", &["x"]), ("2", &[])], &[], &[]);
        let view = render_view(md, &index, &slice(&index, &involving_x()).unwrap());
        assert_eq!(view.rendering, Rendering::Aligned);
        assert!(
            view.content
                .ends_with("> 2. second\n\n- [step 2 omitted: no use of *x*]\n"),
            "{}",
            view.content
        );
    }

    #[test]
    fn repeated_paths_resolve_in_document_order() {
        let md = "1. A:\n\n    1. B.\n    2. C *x*.\n\n    Otherwise:\n\n    1. D.\n    2. E.\n    3. F *x*.\n";
        let index = slice_index(
            "go",
            &[
                ("1", &[]),
                ("1.1", &[]),
                ("1.2", &["x"]),
                ("1.1", &[]),
                ("1.2", &[]),
                ("1.3", &["x"]),
            ],
            &[],
            &[],
        );
        let view = render_view(md, &index, &slice(&index, &involving_x()).unwrap());
        assert_eq!(view.rendering, Rendering::Aligned);
        assert_eq!(
            view.content,
            "1. A:\n\n    - [step 1.1 omitted: no use of *x*]\n\n    2. C *x*.\n\n    Otherwise:\n\n    - [steps 1.1–1.2 omitted: no use of *x*]\n\n    3. F *x*.\n"
        );
    }

    #[test]
    fn a_run_in_a_later_sibling_list_is_not_confused_with_a_kept_step() {
        let md = "1. A:\n\n    1. B *x*.\n    2. C *x*.\n\n    Otherwise:\n\n    1. D.\n    2. E.\n\n2. F.\n";
        let index = slice_index(
            "go",
            &[
                ("1", &[]),
                ("1.1", &["x"]),
                ("1.2", &["x"]),
                ("1.1", &[]),
                ("1.2", &[]),
                ("2", &[]),
            ],
            &[],
            &[],
        );
        let view = render_view(md, &index, &slice(&index, &involving_x()).unwrap());
        assert_eq!(
            view.content,
            "1. A:\n\n    1. B *x*.\n    2. C *x*.\n\n    Otherwise:\n\n    - [steps 1.1–1.2 omitted: no use of *x*]\n\n- [step 2 omitted: no use of *x*]\n"
        );
    }

    #[test]
    fn misaligned_markdown_falls_back_to_full_content() {
        let md = "1. A *x*.\n2. B.\n3. C.\n4. Extra.\n";
        let index = slice_index("go", &[("1", &["x"]), ("2", &[]), ("3", &[])], &[], &[]);
        let view = render_view(md, &index, &slice(&index, &involving_x()).unwrap());
        assert_eq!(
            (view.rendering, view.markdown_items, view.content.as_str()),
            (Rendering::Unaligned, 4, md)
        );
    }

    #[test]
    fn marker_wording() {
        let run = |first: &str, last: &str, steps: u32| OmittedRun {
            parent: None,
            first: first.into(),
            last: last.into(),
            steps,
            in_slice: 0,
            reason: "no use of *x*".into(),
            positions: (0, 0),
        };
        assert_eq!(
            marker_text(&run("15.2", "15.2", 1)),
            "[step 15.2 omitted: no use of *x*]"
        );
        assert_eq!(
            marker_text(&run("15", "15", 3)),
            "[step 15 omitted (3 steps): no use of *x*]"
        );
        assert_eq!(
            marker_text(&run("22.1", "22.3", 3)),
            "[steps 22.1–22.3 omitted: no use of *x*]"
        );
        assert_eq!(
            marker_text(&run("24.1", "24.8", 19)),
            "[steps 24.1–24.8 omitted (19 steps): no use of *x*]"
        );
    }
}
