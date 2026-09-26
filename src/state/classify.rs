//! Occurrence classification over the statement IR (§7.4).

use crate::state::ir::{Clause, ParsedSource, StatementSource};
use crate::state::model::{Occurrence, OccurrenceClass};

/// Classify every link in `source` against `parsed`, producing exactly one
/// [`Occurrence`] per link in `source.links`.  All occurrences carry
/// `basis: "ir"`.
///
/// Links whose grammar role is captured in `parsed.link_roles` use the stored
/// class and statement id.  All other links fall through to the sentence-local
/// heuristic: find the governing clause (the last clause whose start precedes
/// the link and that lies in the same sentence), then classify as
/// `Unclassified` if the clause has a verb and was not consumed, or `Read`
/// otherwise.
pub(crate) fn classify(source: &StatementSource, parsed: &ParsedSource) -> Vec<Occurrence> {
    source
        .links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            let (class, statement_id) =
                if let Some((class, stmt_id)) = parsed.link_roles.get(&index) {
                    (*class, Some(stmt_id.clone()))
                } else {
                    (fallback_class(source, parsed, link.span.start), None)
                };
            Occurrence {
                source_id: source.id.clone(),
                link_id: link.id.clone(),
                target: link.target.clone(),
                class,
                statement_id,
                basis: "ir".to_string(),
                rule_ids: Vec::new(),
            }
        })
        .collect()
}

/// Fallback classification for a link that has no grammar role.
///
/// Searches backward through the clause list for the last clause that
/// starts at or before `link_start` within the same sentence (no `. ` or `;`
/// separates its start from `link_start` in the source text).  If that clause
/// has a verb and was not consumed by the grammar → `Unclassified`; otherwise
/// → `Read`.
pub(crate) fn fallback_class(
    source: &StatementSource,
    parsed: &ParsedSource,
    link_start: usize,
) -> OccurrenceClass {
    let governing = parsed.clauses.iter().rev().find(|clause| {
        clause.start <= link_start && same_sentence(&source.text, clause.start, link_start)
    });
    match governing {
        Some(Clause {
            verb: Some(_),
            consumed: false,
            ..
        }) => OccurrenceClass::Unclassified,
        _ => OccurrenceClass::Read,
    }
}

/// Returns `true` if no `. ` or `;` appears in `text[clause_start..link_start]`.
fn same_sentence(text: &str, clause_start: usize, link_start: usize) -> bool {
    if clause_start >= link_start {
        return true;
    }
    let segment = &text[clause_start..link_start];
    !segment.contains(". ") && !segment.contains(';')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ir::parse_source;

    fn classes(step: &str) -> Vec<(String, OccurrenceClass)> {
        let source = crate::state::ir::tests_support::sources(&[step]).remove(0);
        let parsed = parse_source(&source);
        classify(&source, &parsed)
            .into_iter()
            .map(|o| {
                let link = source.links.iter().find(|l| l.id == o.link_id).unwrap();
                (link.visible_text.clone(), o.class)
            })
            .collect()
    }

    #[test]
    fn unlinked_add_leaves_its_links_unclassified() {
        let c = classes(
            r##"Add <var>subject</var> to <var>subject</var>'s <a href="#concept-node-document">node document</a>'s <a href="#open-dialogs-list">open dialogs list</a>."##,
        );
        assert_eq!(
            c,
            vec![
                ("node document".into(), OccurrenceClass::Unclassified),
                ("open dialogs list".into(), OccurrenceClass::Unclassified)
            ]
        );
    }

    #[test]
    fn condition_links_are_reads_and_bindings_are_not_verbs() {
        let c = classes(
            r##"If <var>d</var>'s <a href="#concept-document-salvageable">salvageable</a> is false, then set <var>e</var>'s <a href="#concept-document-salvageable">salvageable</a> state to false."##,
        );
        assert_eq!(c[0].1, OccurrenceClass::Read);
        assert_eq!(c[1].1, OccurrenceClass::Write);
        let c = classes(
            r##"<a href="#fetch">Fetch</a> <var>request</var> with <a href="#processResponse">processResponse</a> set to <var>steps</var>."##,
        );
        assert!(c.iter().all(|(_, class)| *class == OccurrenceClass::Read));
    }

    #[test]
    fn flag_test_is_a_read() {
        let c = classes(
            r##"If <var>event</var>'s <a href="#stop-propagation-flag">stop propagation flag</a> is set, then return."##,
        );
        assert_eq!(c[0].1, OccurrenceClass::Read);
    }
}
