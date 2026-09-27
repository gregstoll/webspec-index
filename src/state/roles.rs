//! Link roles (§7.7): one role per link of every statement source.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::parse::steps::AnchorTarget;
use crate::state::grammar::role_rank;
use crate::state::ir::{
    LinkRole, OpBasis, ParsedSource, SourceContext, SourceLinkRoles, StatementKind, StatementSource,
};
use crate::state::model::{Occurrence, StateSpec};

/// The role of every link of `source`, in link order.
#[allow(dead_code)]
pub(crate) fn link_roles(
    source: &StatementSource,
    parsed: &ParsedSource,
    occurrences: &[Occurrence],
) -> SourceLinkRoles {
    let roles = if matches!(source.context, SourceContext::Intro { .. }) {
        intro_roles(source)
    } else {
        statement_roles(source, parsed, occurrences)
    };
    SourceLinkRoles {
        source_id: source.id.clone(),
        roles,
    }
}

fn raise(slot: &mut LinkRole, role: LinkRole) {
    if role_rank(&role) > role_rank(slot) {
        *slot = role;
    }
}

fn statement_roles(
    source: &StatementSource,
    parsed: &ParsedSource,
    occurrences: &[Occurrence],
) -> Vec<LinkRole> {
    let mut roles: Vec<LinkRole> = (0..source.links.len())
        .map(|i| parsed.roles.get(&i).cloned().unwrap_or(LinkRole::Unknown))
        .collect();
    let with_occurrence: HashSet<&str> = occurrences
        .iter()
        .filter(|o| o.source_id == source.id)
        .map(|o| o.link_id.as_str())
        .collect();
    for (role, link) in roles.iter_mut().zip(&source.links) {
        if with_occurrence.contains(link.id.as_str()) {
            raise(role, LinkRole::Field);
        }
    }
    for statement in &parsed.statements {
        let StatementKind::Mutate {
            basis: OpBasis::InfraLink(op),
            ..
        } = &statement.kind
        else {
            continue;
        };
        let first = source.links.iter().position(|link| {
            link.span.start >= statement.span.start
                && link.span.end <= statement.span.end
                && link.target.as_ref() == Some(op)
        });
        if let Some(i) = first {
            raise(
                &mut roles[i],
                LinkRole::InfraOp {
                    statement: statement.id.clone(),
                },
            );
        }
    }
    roles
}

/// Intro links name types, except the targets of parameter defaults
/// (`(default ⟦"auto"⟧)`), which are values. Defaults are read from the
/// `(default …)` parentheticals the signature grammar parses them from.
fn intro_roles(source: &StatementSource) -> Vec<LinkRole> {
    let defaults = default_ranges(&source.text);
    let value_targets: HashSet<&AnchorTarget> = source
        .links
        .iter()
        .filter(|link| {
            defaults
                .iter()
                .any(|&(start, end)| link.span.start >= start && link.span.end <= end)
        })
        .filter_map(|link| link.target.as_ref())
        .collect();
    source
        .links
        .iter()
        .map(|link| match &link.target {
            Some(target) if value_targets.contains(target) => LinkRole::Value,
            _ => LinkRole::Type,
        })
        .collect()
}

/// Byte ranges of the insides of `(default …)` parentheticals.
fn default_ranges(text: &str) -> Vec<(usize, usize)> {
    const OPEN: &str = "(default ";
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(OPEN) {
        let inside = from + found + OPEN.len();
        let end = text[inside..].find(')').map_or(text.len(), |i| inside + i);
        ranges.push((inside, end));
        from = inside;
    }
    ranges
}

/// `(segment_id, link_id)` → role for every link of a source with an
/// `Algorithm` context.
pub fn link_role_index(state: &StateSpec) -> BTreeMap<(String, String), LinkRole> {
    let sources: HashMap<&str, &StatementSource> =
        state.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut index = BTreeMap::new();
    for entry in &state.link_roles {
        let Some(source) = sources.get(entry.source_id.as_str()) else {
            continue;
        };
        let SourceContext::Algorithm { segment_id, .. } = &source.context else {
            continue;
        };
        for (role, link) in entry.roles.iter().zip(&source.links) {
            index.insert((segment_id.clone(), link.id.clone()), role.clone());
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::grammar::{Callable, Env};
    use crate::state::ir::{parse_source_with, tests_support::sources_in, LinkRole};
    use crate::state::testing::NAV_HTML;

    #[test]
    fn every_link_gets_exactly_one_role_and_g2_roles_hold() {
        let env = Env {
            spec: "HTML".into(),
            callables: [("navigate".to_string(), Callable::Template)].into(),
            ..Env::default()
        };
        let all = sources_in(NAV_HTML, "HTML");
        for src in all
            .iter()
            .filter(|s| s.subject.anchor == "location-object-navigate")
        {
            let parsed = parse_source_with(src, &env);
            let occ = crate::state::classify::classify(src, &parsed);
            let roles = link_roles(src, &parsed, &occ);
            assert_eq!(
                (roles.source_id.as_str(), roles.roles.len()),
                (src.id.as_str(), src.links.len())
            );
            for (role, link) in roles.roles.iter().zip(&src.links) {
                let callee = matches!(role, LinkRole::Callee { .. });
                assert_eq!(
                    callee,
                    link.visible_text == "Navigate",
                    "{} → {:?}",
                    link.visible_text,
                    role
                );
                if link.target.as_ref().is_some_and(|t| {
                    t.anchor == "exceptions-enabled" || t.anchor == "navigation-hh"
                }) {
                    assert!(matches!(role, LinkRole::ParamName { .. }));
                }
            }
        }
    }

    fn one(step: &str) -> (StatementSource, ParsedSource) {
        let source = crate::state::ir::tests_support::sources(&[step]).remove(0);
        let parsed = parse_source_with(&source, &Env::default());
        (source, parsed)
    }

    #[test]
    fn infra_op_links_occurrences_and_the_rest() {
        let (src, parsed) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>x</var> to <var>d</var>'s <a href="#f">f</a>."##,
        );
        let occ = crate::state::classify::classify(&src, &parsed);
        let roles = link_roles(&src, &parsed, &occ).roles;
        assert!(
            matches!(&roles[0], LinkRole::InfraOp { statement } if statement == &parsed.statements[0].id),
            "{roles:?}"
        );
        assert_eq!(roles[1], LinkRole::Field);
        let roles = link_roles(&src, &parsed, &[]).roles;
        assert!(matches!(roles[0], LinkRole::InfraOp { .. }));
        assert_eq!(roles[1], LinkRole::Unknown);
    }

    #[test]
    fn intro_links_are_types_except_default_values() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="go">go</dfn> given a <a href="#doc">Document</a> <var>d</var> and a <a href="#nhb">NavigationHistoryBehavior</a> <var>h</var> (default "<a href="#nhb-auto">auto</a>"):</p><ol><li><p>Return.</p></li></ol></div>"##;
        let state = crate::state::testing::extract_html(html, "HTML");
        let intro = state
            .sources
            .iter()
            .find(|s| matches!(s.context, SourceContext::Intro { .. }))
            .unwrap();
        let roles = link_roles(intro, &ParsedSource::default(), &[]).roles;
        let named: Vec<(&str, &LinkRole)> = intro
            .links
            .iter()
            .map(|l| l.visible_text.as_str())
            .zip(&roles)
            .collect();
        assert_eq!(
            named,
            vec![
                ("Document", &LinkRole::Type),
                ("NavigationHistoryBehavior", &LinkRole::Type),
                ("auto", &LinkRole::Value)
            ]
        );
    }

    #[test]
    fn role_index_keys_algorithm_links_by_segment() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="u">u</dfn> given a string <var>a</var>:</p><ol><li><p>Set <var>b</var>'s <a href="#f">f</a> to <var>a</var>.</p></li></ol></div>"##;
        let mut state = crate::state::testing::extract_html(html, "HTML");
        let source = state
            .sources
            .iter()
            .find(|s| matches!(s.context, SourceContext::Algorithm { .. }))
            .unwrap()
            .clone();
        let SourceContext::Algorithm { segment_id, .. } = &source.context else {
            unreachable!()
        };
        state.link_roles = vec![SourceLinkRoles {
            source_id: source.id.clone(),
            roles: vec![LinkRole::Field],
        }];
        let index = link_role_index(&state);
        assert_eq!(
            index,
            [(
                (segment_id.clone(), source.links[0].id.clone()),
                LinkRole::Field
            )]
            .into()
        );
    }
}
