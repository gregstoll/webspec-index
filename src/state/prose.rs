//! Prose statement sources (§7.5): normative prose blocks outside structural
//! algorithm bodies that hold a mutation clause or state IDL getter, setter,
//! method or constructor steps.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use ego_tree::iter::Edge;
use regex::Regex;
use scraper::{ElementRef, Html};
use sha2::{Digest, Sha256};

use crate::model::{ParsedSection, SectionType};
use crate::parse::algorithms::step_number;
use crate::parse::steps::{
    canonical_inline, resolve_href, structural_body_nodes, AnchorTarget, InlineContext,
};
use crate::state::ir::{has_mutation_clause, ProseRole, SourceContext, StatementSource};

pub(crate) struct ProseOutput {
    pub sources: Vec<StatementSource>,
    pub mentions: BTreeMap<String, u32>,
    pub callouts_excluded: u32,
}

/// One document-order pass. Blocks inside a structural algorithm body or a
/// `pre` are skipped; callout blocks only count toward `callouts_excluded`.
///
/// `precomputed_bodies` — when the caller already ran `structural_body_nodes`
/// on this document (e.g. pipeline.rs, to avoid a second pass), pass a
/// reference here.  Pass `None` to compute it inline.
pub(crate) fn prose_sources(
    document: &Html,
    spec: &str,
    base_url: &str,
    snapshot_sha: &str,
    sections: &[ParsedSection],
    precomputed_bodies: Option<&HashSet<ego_tree::NodeId>>,
) -> ProseOutput {
    let scope_anchors: HashSet<&str> = sections
        .iter()
        .filter(|s| {
            matches!(
                s.section_type,
                SectionType::Heading | SectionType::Algorithm
            )
        })
        .map(|s| s.anchor.as_str())
        .collect();
    let bodies_owned;
    let bodies: &HashSet<ego_tree::NodeId> = if let Some(b) = precomputed_bodies {
        b
    } else {
        bodies_owned = structural_body_nodes(document);
        &bodies_owned
    };
    let mut out = ProseOutput {
        sources: Vec::new(),
        mentions: BTreeMap::new(),
        callouts_excluded: 0,
    };

    let mut scope: Option<&str> = None;
    // Per open element: its path step, its same-name child counts, and
    // whether it excludes or marks as callout everything below it.
    let mut path: Vec<(&str, usize)> = Vec::new();
    let mut child_counts: Vec<HashMap<&str, usize>> = Vec::new();
    let mut flags: Vec<(bool, bool)> = Vec::new();
    let mut excluded_depth = 0usize;
    let mut callout_depth = 0usize;
    let mut steps: Vec<Option<usize>> = Vec::new();

    for edge in document.root_element().traverse() {
        let node = match edge {
            Edge::Open(node) => node,
            Edge::Close(node) => {
                if node.value().is_element() {
                    path.pop();
                    child_counts.pop();
                    steps.pop();
                    let (excluded, callout) = flags.pop().expect("balanced traversal");
                    excluded_depth -= usize::from(excluded);
                    callout_depth -= usize::from(callout);
                }
                continue;
            }
        };
        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        let name = element.value().name();
        let index = child_counts.last_mut().map_or(1, |counts| {
            let count = counts.entry(name).or_default();
            *count += 1;
            *count
        });
        path.push((name, index));
        child_counts.push(HashMap::new());
        steps.push(if name == "li" {
            step_number(&element)
        } else {
            None
        });
        let excluded = name == "pre" || bodies.contains(&element.id());
        let callout = is_callout(&element);
        flags.push((excluded, callout));
        excluded_depth += usize::from(excluded);
        callout_depth += usize::from(callout);

        if let Some(id) = element.value().id() {
            if scope_anchors.contains(id) {
                scope = Some(id);
            }
        }
        if excluded_depth > 0 || !is_block(&element) {
            continue;
        }

        let provisional = scope.unwrap_or("");
        let render = |anchor: &str| {
            canonical_inline(
                &element,
                &InlineContext {
                    spec,
                    base_url,
                    snapshot_sha,
                    anchor,
                },
            )
        };
        let (text, tokens, links) = render(provisional);
        let step_path = steps
            .iter()
            .flatten()
            .map(usize::to_string)
            .collect::<Vec<_>>();
        let mut source = StatementSource {
            id: String::new(),
            subject: AnchorTarget {
                spec: spec.to_string(),
                anchor: provisional.to_string(),
            },
            context: SourceContext::Prose {
                node_id: String::new(),
                step_path: (!step_path.is_empty()).then(|| step_path.join(".")),
                role: ProseRole::Normative,
            },
            text,
            tokens,
            links,
        };
        let mutation = has_mutation_clause(&source);
        if callout_depth > 0 {
            out.callouts_excluded += u32::from(mutation);
            continue;
        }
        let found = if mutation {
            subject(&element, &source.text, spec, base_url, scope)
        } else {
            steps_subject(&element, &source.text, spec, base_url).filter(|(_, role)| {
                matches!(
                    role,
                    ProseRole::Getter
                        | ProseRole::Setter
                        | ProseRole::Method
                        | ProseRole::Constructor
                )
            })
        };
        let Some((subject, role)) = found else {
            if !mutation {
                for target in source.links.iter().filter_map(|link| link.target.as_ref()) {
                    *out.mentions
                        .entry(format!("{}#{}", target.spec, target.anchor))
                        .or_default() += 1;
                }
            }
            continue;
        };
        if subject.anchor != provisional {
            (source.text, source.tokens, source.links) = render(&subject.anchor);
        }
        let node_id = std::iter::once(".".to_string())
            .chain(
                path[1..]
                    .iter()
                    .map(|(name, index)| format!("{name}[{index}]")),
            )
            .collect::<Vec<_>>()
            .join("/");
        source.id = prose_id(snapshot_sha, spec, &node_id);
        source.subject = subject;
        if let SourceContext::Prose {
            node_id: id,
            role: r,
            ..
        } = &mut source.context
        {
            *id = node_id;
            *r = role;
        }
        out.sources.push(source);
    }
    out
}

/// `prose-` + sha256(snapshot_sha \0 spec \0 node path).
fn prose_id(snapshot_sha: &str, spec: &str, node_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(snapshot_sha.as_bytes());
    for component in [spec, node_id] {
        hasher.update([0]);
        hasher.update(component.as_bytes());
    }
    format!("prose-{:x}", hasher.finalize())
}

fn is_nested_list(name: &str) -> bool {
    matches!(name, "ol" | "ul" | "dl")
}

fn is_block_name(name: &str) -> bool {
    matches!(name, "p" | "li" | "dd" | "dt")
}

/// A `p`/`li`/`dd`/`dt` holding no other block, except inside its own nested
/// lists.
fn is_block(element: &ElementRef<'_>) -> bool {
    fn holds_block(element: &ElementRef<'_>) -> bool {
        element
            .children()
            .filter_map(ElementRef::wrap)
            .any(|child| {
                let name = child.value().name();
                !is_nested_list(name) && (is_block_name(name) || holds_block(&child))
            })
    }
    is_block_name(element.value().name()) && !holds_block(element)
}

fn is_callout(element: &ElementRef<'_>) -> bool {
    let value = element.value();
    match value.name() {
        "p" | "div" | "aside" | "details" | "blockquote" => value
            .classes()
            .any(|class| matches!(class, "note" | "example" | "warning" | "advisement")),
        "dl" => value.classes().any(|class| class == "domintro"),
        _ => false,
    }
}

/// The block's elements in document order, without its nested lists.
fn own_elements<'a>(block: &ElementRef<'a>) -> Vec<ElementRef<'a>> {
    fn walk<'a>(element: &ElementRef<'a>, out: &mut Vec<ElementRef<'a>>) {
        for child in element.children().filter_map(ElementRef::wrap) {
            if !is_nested_list(child.value().name()) {
                out.push(child);
                walk(&child, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(block, &mut out);
    out
}

/// Text compared by the `steps are to` subject rule: no backticks, no `()`
/// arguments, collapsed whitespace.
fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '`' => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dfn_id<'a>(element: &ElementRef<'a>) -> Option<&'a str> {
    let value = element.value();
    (value.name() == "dfn").then(|| value.id()).flatten()
}

fn local(spec: &str, anchor: &str) -> AnchorTarget {
    AnchorTarget {
        spec: spec.to_string(),
        anchor: anchor.to_string(),
    }
}

/// The block's subject and role, first match wins: [`steps_subject`]; the
/// block's only `dfn[id]`; the reference scope.
fn subject(
    block: &ElementRef<'_>,
    text: &str,
    spec: &str,
    base_url: &str,
    scope: Option<&str>,
) -> Option<(AnchorTarget, ProseRole)> {
    if let Some(found) = steps_subject(block, text, spec, base_url) {
        return Some(found);
    }
    let elements = own_elements(block);
    let mut dfns = elements.iter().filter_map(dfn_id);
    if let (Some(id), None) = (dfns.next(), dfns.next()) {
        return Some((local(spec, id), ProseRole::Normative));
    }
    scope.map(|anchor| (local(spec, anchor), ProseRole::Normative))
}

/// Subject rule 1: a `The X [role] steps are to` block whose first
/// `dfn[id]`/`a[href]` is X.
fn steps_subject(
    block: &ElementRef<'_>,
    text: &str,
    spec: &str,
    base_url: &str,
) -> Option<(AnchorTarget, ProseRole)> {
    static STEPS: OnceLock<Regex> = OnceLock::new();
    let steps = STEPS.get_or_init(|| {
        Regex::new(r"^The (?P<x>.+?) (?:(?P<role>getter|setter|method|constructor) )?steps are to ")
            .expect("valid regex")
    });
    let captures = steps.captures(text)?;
    let (element, target) = own_elements(block).into_iter().find_map(|element| {
        if let Some(id) = dfn_id(&element) {
            return Some((element, Some(local(spec, id))));
        }
        let href = (element.value().name() == "a")
            .then(|| element.value().attr("href"))
            .flatten()?;
        Some((element, resolve_href(href, spec, base_url)))
    })?;
    let target = target?;
    if normalized(&element.text().collect::<String>()) != normalized(&captures["x"]) {
        return None;
    }
    let role = match captures.name("role").map(|m| m.as_str()) {
        Some("getter") => ProseRole::Getter,
        Some("setter") => ProseRole::Setter,
        Some("method") => ProseRole::Method,
        Some("constructor") => ProseRole::Constructor,
        _ => ProseRole::Steps,
    };
    Some((target, role))
}

#[cfg(test)]
mod tests {
    use crate::state::ir::{ProseRole, SourceContext};
    use crate::state::model::{OccurrenceClass, SiteClass, StateSpec};

    fn state(html: &str) -> StateSpec {
        crate::state::testing::extract_html(html, "DOM")
    }

    const EVENT: &str = crate::state::testing::PROSE_DOM;

    #[test]
    fn method_and_setter_steps_are_prose_writes_getter_is_a_read_source() {
        let s = state(EVENT);
        let sites = crate::state::extract::derive_sites(&s);
        let writes: Vec<_> = sites
            .iter()
            .filter(|x| x.class == SiteClass::Write)
            .map(|x| (x.subject.anchor.as_str(), x.role.as_deref()))
            .collect();
        assert_eq!(
            writes,
            [
                ("dom-event-stoppropagation", Some("method")),
                ("dom-event-cancelbubble", Some("setter"))
            ]
        );
        let getter = s
            .sources
            .iter()
            .find(|x| {
                matches!(
                    x.context,
                    SourceContext::Prose {
                        role: ProseRole::Getter,
                        ..
                    }
                )
            })
            .expect("getter block is a source");
        assert_eq!(getter.subject.anchor, "dom-event-cancelbubble");
        let getter_classes: Vec<_> = s
            .occurrences
            .iter()
            .filter(|o| o.source_id == getter.id)
            .map(|o| o.class)
            .collect();
        assert!(!getter_classes.is_empty());
        assert!(getter_classes.iter().all(|c| *c == OccurrenceClass::Read));
        assert_eq!(s.prose_mentions.get("DOM#stop-propagation-flag"), None);
        assert_eq!(s.coverage.prose_callouts_excluded, 2);
        let method = sites
            .iter()
            .find(|x| x.role.as_deref() == Some("method"))
            .unwrap();
        assert_eq!(method.context, "prose");
        assert_eq!(method.text, "… set this’s stop propagation flag.");
    }

    #[test]
    fn steps_blocks_without_idl_role_or_mutation_stay_mentions() {
        let html = r##"<h3 id="h">H</h3><p>The <dfn id="foo-steps">foo</dfn> steps are to return <a href="#f">f</a>.</p>"##;
        let s = state(html);
        assert!(s.sources.iter().all(|x| !x.id.starts_with("prose-")));
        assert_eq!(s.prose_mentions.get("DOM#f"), Some(&1));
    }

    #[test]
    fn prose_step_lists_outside_structure_keep_step_numbers() {
        let html = r##"<h3 id="h">H</h3><p>Some intro without a dfn.</p><ol><li><p>Do a thing.</p></li><li><p>Set <var>x</var>’s <a href="#f">f</a> to 1.</p></li></ol>"##;
        let s = state(html);
        let site = crate::state::extract::derive_sites(&s)
            .into_iter()
            .find(|x| x.class == SiteClass::Write)
            .unwrap();
        assert_eq!(site.step_path.as_deref(), Some("2"));
        assert_eq!(site.subject.anchor, "h");
    }
}
