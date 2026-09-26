//! View names, content headings and summary blocks for slice results (spec §8.3).
use super::{
    select::{StepRole, UnfollowedReason, VarBasis},
    view::{SliceIssue, SliceResult},
    DefKind, FeedingSelector, ViewRequest,
};

fn or_list(names: &[String]) -> String {
    let emph: Vec<String> = names.iter().map(|n| format!("*{n}*")).collect();
    match emph.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

/// Human-readable name for a view request (spec §8.3).
pub fn view_name(view: &ViewRequest) -> String {
    let selector = if !view.involving.is_empty() {
        format!("steps involving {}", or_list(&view.involving))
    } else if let Some(feeding) = &view.feeding {
        feeding_name(feeding)
    } else if !view.steps.is_empty() {
        format!("steps {}", view.steps.join(", "))
    } else {
        String::new()
    };

    match (selector.is_empty(), view.depth) {
        (true, Some(d)) => format!("outline to depth {d}"),
        (false, None) => selector,
        (false, Some(d)) => format!("{selector}, outline to depth {d}"),
        (true, None) => String::new(),
    }
}

fn feeding_name(feeding: &FeedingSelector) -> String {
    if feeding.variables.is_empty() {
        format!("steps feeding step {}", feeding.step)
    } else {
        format!(
            "steps feeding {} in step {}",
            or_list(&feeding.variables),
            feeding.step
        )
    }
}

/// `Content — {view_name}` heading for use in the rendered output.
pub fn content_heading(result: &SliceResult) -> String {
    format!("Content \u{2014} {}", view_name(&result.slice.view))
}

/// Multi-line summary block describing what the view kept and why (spec §8.3).
pub fn summary_block(result: &SliceResult) -> String {
    let view = &result.slice.view;
    let counts = &result.status.counts;
    let slice = &result.slice;

    let context_paths: Vec<String> = slice
        .steps
        .iter()
        .filter(|s| s.role == StepRole::Context)
        .map(|s| s.path.clone())
        .collect();
    let context_count = context_paths.len();
    let encloses_part = if context_count > 0 {
        let paths = context_paths.join(", ");
        if context_count == 1 {
            format!(", 1 encloses them ({paths})")
        } else {
            format!(", {context_count} enclose them ({paths})")
        }
    } else {
        String::new()
    };

    let omitted_total = counts.omitted;
    let omitted_runs = slice.omitted.len();
    let omitted_part = if omitted_runs == 0 {
        ". Nothing omitted.".to_string()
    } else {
        let marker_word = if omitted_runs == 1 {
            "marker"
        } else {
            "markers"
        };
        format!(". {omitted_total} omitted in {omitted_runs} {marker_word}.")
    };

    let depth_only = view.involving.is_empty() && view.feeding.is_none() && view.steps.is_empty();
    let line1 = if depth_only {
        format!(
            "Kept {} of {} steps to depth {}{}",
            counts.kept,
            counts.steps,
            view.depth.unwrap_or(0),
            omitted_part
        )
    } else if !view.involving.is_empty() {
        let match_word = if counts.matched == 1 {
            "involves"
        } else {
            "involve"
        };
        let inherited_part = if counts.inherited > 0 {
            format!(", {} inherited", counts.inherited)
        } else {
            String::new()
        };
        format!(
            "Kept {} of {} steps: {} {} the slice variables{}{}{omitted_part}",
            counts.kept, counts.steps, counts.matched, match_word, inherited_part, encloses_part,
        )
    } else if view.feeding.is_some() {
        let target_path = slice
            .steps
            .iter()
            .find(|s| s.role == StepRole::Target)
            .map(|s| s.path.as_str())
            .unwrap_or("?");
        let def_count = slice
            .steps
            .iter()
            .filter(|s| s.role == StepRole::Definition)
            .count();
        let def_word = if def_count == 1 {
            "definition"
        } else {
            "definitions"
        };
        format!(
            "Kept {} of {} steps: step {target_path} (target), {def_count} {def_word}{}{omitted_part}",
            counts.kept, counts.steps, encloses_part,
        )
    } else {
        let selected_count = slice
            .steps
            .iter()
            .filter(|s| s.role == StepRole::Selected)
            .count();
        format!(
            "Kept {} of {} steps: {selected_count} selected{}{omitted_part}",
            counts.kept, counts.steps, encloses_part,
        )
    };

    let mut lines = vec![line1];

    let has_slice_vars = !view.involving.is_empty() || view.feeding.is_some();
    if has_slice_vars {
        let seeds: Vec<String> = slice
            .variables
            .iter()
            .filter(|v| v.basis == VarBasis::Seed)
            .map(|v| format!("*{}*", v.name))
            .collect();
        let derived: Vec<String> = slice
            .variables
            .iter()
            .filter(|v| v.basis != VarBasis::Seed)
            .map(|v| {
                let step = v.step.as_deref().unwrap_or("?");
                let label = match v.basis {
                    VarBasis::Let => format!("Let at {step}"),
                    VarBasis::Set => format!("Set at {step}"),
                    VarBasis::Mutate => format!("Mutate at {step}"),
                    VarBasis::Feeds => format!("feeds {step}"),
                    VarBasis::Seed => unreachable!(),
                };
                format!("*{}* ({label})", v.name)
            })
            .collect();
        let vars_line = if derived.is_empty() {
            format!("Slice variables: {}.", seeds.join(", "))
        } else {
            format!(
                "Slice variables: {}; {}.",
                seeds.join(", "),
                derived.join(", ")
            )
        };
        lines.push(vars_line);
        lines.push("Semantics: may; this algorithm only, callees are not followed.".to_string());
    }

    if !view.involving.is_empty() {
        let inherited_paths: Vec<String> = slice
            .steps
            .iter()
            .filter(|s| s.role == StepRole::Inherited)
            .map(|s| s.path.clone())
            .collect();
        if !inherited_paths.is_empty() {
            lines.push(format!(
                "Inherited (no variables, under a matched step): {}",
                inherited_paths.join(", ")
            ));
        }
    }

    if !slice.stores.is_empty() {
        let entries: Vec<String> = slice
            .stores
            .iter()
            .map(|s| format!("{} {} ({})", s.step, s.target, or_list(&s.from)))
            .collect();
        lines.push(format!(
            "Stored into object state, not followed: {}",
            entries.join("; ")
        ));
    }

    if !slice.unfollowed.is_empty() {
        let entries: Vec<String> = slice
            .unfollowed
            .iter()
            .map(|u| match u.reason {
                UnfollowedReason::LoopBinding => {
                    format!("{} binds {} in a loop", u.step, or_list(&u.variables))
                }
                UnfollowedReason::OpaqueStatement => format!(
                    "{} has an unparsed statement using {}",
                    u.step,
                    or_list(&u.variables)
                ),
            })
            .collect();
        lines.push(format!("Not followed: {}", entries.join("; ")));
    }

    if !slice.rebound.is_empty() {
        let entries: Vec<String> = slice
            .rebound
            .iter()
            .map(|r| format!("*{}* (Let at {})", r.name, r.steps.join(", ")))
            .collect();
        lines.push(format!(
            "Rebound names treated as one variable: {}",
            entries.join(", ")
        ));
    }

    if let Some(inputs) = &slice.inputs {
        if !inputs.is_empty() {
            let emphs: Vec<String> = inputs.iter().map(|n| format!("*{n}*")).collect();
            lines.push(format!("Inputs: {}", emphs.join(", ")));
        }
    }

    if let Some(later_defs) = &slice.later_definitions {
        if !later_defs.is_empty() {
            let entries: Vec<String> = later_defs
                .iter()
                .map(|d| {
                    let kind_str = match d.kind {
                        DefKind::Let => "Let",
                        DefKind::Set => "Set",
                        DefKind::Mutate => "Mutate",
                        DefKind::Store => "Store",
                        DefKind::Opaque => "Opaque",
                    };
                    format!("{} {kind_str} *{}*", d.step, d.variable)
                })
                .collect();
            lines.push(format!(
                "Later definitions, not followed: {}",
                entries.join("; ")
            ));
        }
    }

    if result.status.issues.contains(&SliceIssue::RenderUnaligned) {
        if let Some(markdown_items) = result.status.markdown_items {
            let view_steps: Vec<String> = slice.steps.iter().map(|s| s.path.clone()).collect();
            lines.push(format!(
                "Rendering fell back to the full algorithm: the stored content's step numbering \
                 differs from the structure ({markdown_items} items vs {} steps). Steps in the view: {}.",
                counts.steps,
                view_steps.join(", ")
            ));
        }
    }

    lines.join("\n")
}

/// `## Content — {heading}\n\n{summary_block}\n\n{content}`.
pub fn content_part(result: &SliceResult, content: &str) -> String {
    format!(
        "## {}\n\n{}\n\n{content}",
        content_heading(result),
        summary_block(result)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::slice::{
        render::render_view,
        select::{slice, FeedingSelector, ViewRequest},
        view::slice_result,
        DefKind, SliceIndex,
    };
    use crate::state::testing::{slice_fixture_insert, slice_fixture_navigate, slice_index};

    /// Builds a synthetic ordered-list markdown aligned with `index.steps`.
    fn synthetic_markdown(index: &SliceIndex) -> String {
        let mut out = String::new();
        for (i, step) in index.steps.iter().enumerate() {
            let parts: Vec<&str> = step.path.split('.').collect();
            let depth = parts.len();
            let ordinal = parts.last().copied().unwrap_or("1");
            let indent = "    ".repeat(depth - 1);
            let prev_depth = if i == 0 {
                0
            } else {
                index.steps[i - 1].path.split('.').count()
            };
            if i > 0 && depth <= prev_depth {
                out.push('\n');
            }
            out.push_str(&format!("{indent}{ordinal}. S{}.\n", step.path));
            if let Some(next) = index.steps.get(i + 1) {
                if next.path.split('.').count() > depth {
                    out.push('\n');
                }
            }
        }
        out
    }

    fn result(index: &SliceIndex, view: ViewRequest) -> SliceResult {
        let s = slice(index, &view).unwrap();
        let md = synthetic_markdown(index);
        let rendered = render_view(&md, index, &s);
        slice_result(format!("T#{}", index.anchor), index, s, &rendered)
    }

    #[test]
    fn forward_summary() {
        let r = result(
            &slice_fixture_insert(),
            ViewRequest {
                involving: vec!["node".into()],
                ..Default::default()
            },
        );
        assert_eq!(
            content_heading(&r),
            "Content \u{2014} steps involving *node*"
        );
        assert_eq!(
            summary_block(&r),
            "Kept 14 of 18 steps: 12 involve the slice variables, 1 inherited, 1 encloses them (5). 4 omitted in 3 markers.\n\
             Slice variables: *node*; *nodes* (Let at 1), *count* (Let at 2).\n\
             Semantics: may; this algorithm only, callees are not followed.\n\
             Inherited (no variables, under a matched step): 4.1\n\
             Stored into object state, not followed: 7.2 *parent*'s field (*node*)\n\
             Not followed: 7.3 binds *inclusiveDescendant* in a loop"
        );
    }

    #[test]
    fn backward_summary() {
        let r = result(
            &slice_fixture_navigate(),
            ViewRequest {
                feeding: Some(FeedingSelector {
                    step: "7".into(),
                    variables: vec![],
                }),
                ..Default::default()
            },
        );
        assert_eq!(content_heading(&r), "Content \u{2014} steps feeding step 7");
        assert_eq!(
            summary_block(&r),
            "Kept 8 of 11 steps: step 7 (target), 6 definitions, 1 encloses them (3). 3 omitted in 2 markers.\n\
             Slice variables: *navigable*, *entry*, *historyHandling*; *state* (feeds 5), *url* (feeds 5), *initiator* (feeds 4), *referrerPolicy* (feeds 4), *sourceDocument* (feeds 2).\n\
             Semantics: may; this algorithm only, callees are not followed.\n\
             Not followed: 4 has an unparsed statement using *initiator*\n\
             Inputs: *historyHandling*, *navigable*, *referrerPolicy*, *sourceDocument*, *url*\n\
             Later definitions, not followed: 8 Set *entry*"
        );
    }

    #[test]
    fn steps_depth_and_combined_names() {
        let r = result(
            &slice_fixture_navigate(),
            ViewRequest {
                steps: vec!["3".into(), "4.1".into()],
                ..Default::default()
            },
        );
        assert_eq!(content_heading(&r), "Content \u{2014} steps 3, 4.1");
        assert_eq!(
            summary_block(&r),
            "Kept 4 of 11 steps: 3 selected, 1 encloses them (4). 7 omitted in 2 markers."
        );
        let r = result(
            &slice_fixture_navigate(),
            ViewRequest {
                depth: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(content_heading(&r), "Content \u{2014} outline to depth 1");
        assert_eq!(
            summary_block(&r),
            "Kept 9 of 11 steps to depth 1. 2 omitted in 2 markers."
        );
        let view = ViewRequest {
            involving: vec!["initiator".into()],
            depth: Some(1),
            ..Default::default()
        };
        assert_eq!(
            view_name(&view),
            "steps involving *initiator*, outline to depth 1"
        );
        let view = ViewRequest {
            feeding: Some(FeedingSelector {
                step: "7".into(),
                variables: vec!["entry".into(), "url".into()],
            }),
            ..Default::default()
        };
        assert_eq!(view_name(&view), "steps feeding *entry* or *url* in step 7");
    }

    #[test]
    fn nothing_omitted() {
        let index = slice_index(
            "go",
            &[("1", &["x"]), ("2", &["x"]), ("3", &["x"])],
            &[],
            &[],
        );
        let r = result(
            &index,
            ViewRequest {
                involving: vec!["x".into()],
                ..Default::default()
            },
        );
        assert_eq!(
            summary_block(&r).lines().next().unwrap(),
            "Kept 3 of 3 steps: 3 involve the slice variables. Nothing omitted."
        );
    }

    #[test]
    fn rebound_line() {
        let index = slice_index(
            "go",
            &[("1", &["x"]), ("2", &["x", "foo"])],
            &[
                ("1", DefKind::Let, Some("x"), &[]),
                ("2", DefKind::Let, Some("x"), &["foo"]),
            ],
            &[],
        );
        let r = result(
            &index,
            ViewRequest {
                involving: vec!["foo".into()],
                ..Default::default()
            },
        );
        assert!(
            summary_block(&r)
                .ends_with("\nRebound names treated as one variable: *x* (Let at 1, 2)"),
            "{}",
            summary_block(&r)
        );
    }

    #[test]
    fn unaligned_summary_lists_the_view() {
        let index = slice_index("go", &[("1", &["x"]), ("2", &[]), ("3", &["x"])], &[], &[]);
        let view = ViewRequest {
            involving: vec!["x".into()],
            ..Default::default()
        };
        let s = slice(&index, &view).unwrap();
        let rendered = render_view("1. A.\n2. B.\n3. C.\n4. D.\n", &index, &s);
        let r = slice_result("T#go".into(), &index, s, &rendered);
        assert!(
            summary_block(&r).ends_with(
                "\nRendering fell back to the full algorithm: the stored content's step numbering \
                 differs from the structure (4 items vs 3 steps). Steps in the view: 1, 3."
            ),
            "{}",
            summary_block(&r)
        );
        assert_eq!(
            content_part(&r, "BODY"),
            format!(
                "## Content \u{2014} steps involving *x*\n\n{}\n\nBODY",
                summary_block(&r)
            )
        );
    }
}
