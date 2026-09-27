//! Algorithm introduction blocks (§8): the paragraph that names an algorithm
//! and its parameters, rendered as an intro source with its variables.
use std::collections::HashMap;

use scraper::{ElementRef, Html};
use sha2::{Digest, Sha256};

use crate::parse::steps::{
    canonical_inline_marked, AnchorTarget, InlineContext, InlineToken, InlineTokenKind, LinkSpan,
    StructuralSpec, TextSpan,
};
use crate::state::block::innermost_block;
use crate::state::ir::{ProseRole, SourceContext, StatementSource};

/// Every element with an `id`, keyed by that id (first occurrence wins).
pub(crate) struct IdIndex<'a> {
    by_id: HashMap<&'a str, ElementRef<'a>>,
}

impl<'a> IdIndex<'a> {
    pub(crate) fn new(document: &'a Html) -> Self {
        let mut by_id = HashMap::new();
        for element in document.root_element().descendent_elements() {
            if let Some(id) = element.value().id() {
                by_id.entry(id).or_insert(element);
            }
        }
        Self { by_id }
    }

    pub(crate) fn get(&self, id: &str) -> Option<ElementRef<'a>> {
        self.by_id.get(id).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Intro {
    pub source: StatementSource,
    pub anchor: String,
    /// Span of the defining dfn in `source.text`.
    pub dfn_span: Option<TextSpan>,
    pub dfn_for: Option<String>,
    pub dfn_text: String,
    pub vars: Vec<IntroVar>,
    pub ecmarkup: bool,
    pub prose_role: Option<ProseRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntroVar {
    pub span: TextSpan,
    pub name: String,
    pub anchor: Option<AnchorTarget>,
    /// The variable is wrapped in a parameter `<dfn>` (named parameter).
    pub param_dfn: bool,
}

/// The intro of every structural algorithm whose anchored element and intro
/// block are found.
pub(crate) fn algorithm_intros(
    document: &Html,
    index: &IdIndex,
    spec: &str,
    base_url: &str,
    snapshot_sha: &str,
    structure: &StructuralSpec,
) -> Vec<Intro> {
    structure
        .algorithms
        .iter()
        .filter_map(|algorithm| {
            algorithm_intro(
                document,
                index,
                spec,
                base_url,
                snapshot_sha,
                &algorithm.source.section_anchor,
            )
        })
        .collect()
}

fn algorithm_intro(
    document: &Html,
    index: &IdIndex,
    spec: &str,
    base_url: &str,
    snapshot_sha: &str,
    anchor: &str,
) -> Option<Intro> {
    let anchored = index.get(anchor)?;
    let ecmarkup = anchored.value().name() == "emu-clause";
    let (block, mark) = if ecmarkup {
        let p = anchored
            .children()
            .filter_map(ElementRef::wrap)
            .find(|child| child.value().name() == "p")?;
        let dfn = p
            .descendent_elements()
            .find(|element| element.value().name() == "dfn");
        (p, dfn)
    } else {
        (intro_block(&anchored)?, Some(anchored))
    };
    let ctx = InlineContext {
        spec,
        base_url,
        snapshot_sha,
        anchor,
    };
    let (text, tokens, links, marks) = canonical_inline_marked(&block, &ctx, mark.map(|m| m.id()));
    let vars = tokens
        .iter()
        .filter(|token| token.kind == InlineTokenKind::Variable)
        .zip(&marks.vars)
        .filter(|(token, _)| !inside_link(token.span, &links))
        .map(|(token, (span, node))| {
            let var = document.tree.get(*node).and_then(ElementRef::wrap);
            let param = var.and_then(|v| parameter_dfn(&v));
            let anchor = match (param, var) {
                (Some(dfn), _) => dfn.value().id(),
                (None, Some(v)) => v.value().id(),
                (None, None) => None,
            };
            IntroVar {
                span: *span,
                name: token.source_text.clone(),
                anchor: anchor.map(|anchor| AnchorTarget {
                    spec: spec.to_string(),
                    anchor: anchor.to_string(),
                }),
                param_dfn: param.is_some(),
            }
        })
        .collect();
    Some(Intro {
        source: intro_source(spec, anchor, text, tokens, links),
        anchor: anchor.to_string(),
        dfn_span: marks.mark,
        dfn_for: mark.and_then(|m| m.value().attr("data-dfn-for").map(str::to_string)),
        dfn_text: mark.map(|m| trimmed_text(&m)).unwrap_or_default(),
        vars,
        ecmarkup,
        prose_role: None,
    })
}

/// An intro built from an IDL-role prose source: its text is the intro.
#[allow(dead_code)]
pub(crate) fn prose_intro(
    index: &IdIndex,
    source: &StatementSource,
    role: ProseRole,
) -> Option<Intro> {
    let anchor = source.subject.anchor.clone();
    let element = index.get(&anchor)?;
    let vars = source
        .tokens
        .iter()
        .filter(|token| {
            token.kind == InlineTokenKind::Variable && !inside_link(token.span, &source.links)
        })
        .map(|token| IntroVar {
            span: token.span,
            name: token.source_text.clone(),
            anchor: None,
            param_dfn: false,
        })
        .collect();
    Some(Intro {
        source: source.clone(),
        anchor,
        dfn_span: None,
        dfn_for: element.value().attr("data-dfn-for").map(str::to_string),
        dfn_text: trimmed_text(&element),
        vars,
        ecmarkup: false,
        prose_role: Some(role),
    })
}

fn intro_block<'a>(anchored: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let is_intro_block = |e: &ElementRef<'_>| matches!(e.value().name(), "p" | "li" | "dd" | "dt");
    if is_intro_block(anchored) {
        return Some(*anchored);
    }
    innermost_block(anchored).filter(is_intro_block)
}

/// The `<dfn>` wrapping exactly this variable, which makes it a named parameter.
fn parameter_dfn<'a>(var: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let parent = var.parent().and_then(ElementRef::wrap)?;
    (parent.value().name() == "dfn" && trimmed_text(&parent) == trimmed_text(var)).then_some(parent)
}

fn inside_link(span: TextSpan, links: &[LinkSpan]) -> bool {
    links
        .iter()
        .any(|link| link.span.start <= span.start && span.end <= link.span.end)
}

fn trimmed_text(element: &ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_string()
}

fn intro_source(
    spec: &str,
    anchor: &str,
    text: String,
    tokens: Vec<InlineToken>,
    links: Vec<LinkSpan>,
) -> StatementSource {
    let subject = AnchorTarget {
        spec: spec.to_string(),
        anchor: anchor.to_string(),
    };
    let id = intro_id(spec, anchor);
    StatementSource {
        id: id.clone(),
        subject: subject.clone(),
        context: SourceContext::Intro {
            algorithm: subject,
            node_id: id,
        },
        text,
        tokens,
        links,
    }
}

/// `intro-` + sha256(spec \0 anchor): stable under unrelated document edits.
fn intro_id(spec: &str, anchor: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(spec.as_bytes());
    hasher.update([0]);
    hasher.update(anchor.as_bytes());
    format!("intro-{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::testing::{ECMA_HTML, EVENT_DOM, NAV_HTML};

    fn intros(html: &str, spec: &str) -> Vec<Intro> {
        let document = Html::parse_document(html);
        let base = crate::state::testing::base_url(spec);
        let structure = extract_step_structure_from_document(&document, spec, base, "hash:t");
        let index = IdIndex::new(&document);
        algorithm_intros(&document, &index, spec, base, "hash:t", &structure)
    }

    #[test]
    fn navigate_intro_vars_and_parameter_dfns() {
        let all = intros(NAV_HTML, "HTML");
        let nav = all.iter().find(|i| i.anchor == "navigate").unwrap();
        assert!(nav
            .source
            .text
            .starts_with("To navigate a navigable *navigable* to a URL *url*"));
        let span = nav.dfn_span.unwrap();
        assert_eq!(&nav.source.text[span.start..span.end], "navigate");
        let names: Vec<_> = nav
            .vars
            .iter()
            .map(|v| {
                (
                    v.name.as_str(),
                    v.param_dfn,
                    v.anchor.as_ref().map(|a| a.anchor.as_str()),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("navigable", false, None),
                ("url", false, None),
                ("sourceDocument", false, Some("source-browsing-context")),
                ("exceptionsEnabled", true, Some("exceptions-enabled")),
                ("historyHandling", true, Some("navigation-hh")),
                ("referrerPolicy", true, Some("navigation-referrer-policy")),
            ]
        );
        assert!(
            matches!(&nav.source.context, SourceContext::Intro { node_id, .. } if node_id == &nav.source.id && node_id.starts_with("intro-"))
        );
        assert_eq!(nav.dfn_for, None);
    }

    #[test]
    fn idl_and_ecmarkup_intros() {
        let event = intros(EVENT_DOM, "DOM");
        let init = event
            .iter()
            .find(|i| i.anchor == "dom-event-initevent")
            .unwrap();
        assert_eq!(init.dfn_for.as_deref(), Some("Event"));
        assert!(init.source.text.contains("method steps are"));
        let ecma = intros(ECMA_HTML, "ECMA-262");
        let sio = ecma
            .iter()
            .find(|i| i.anchor == "sec-stringindexof")
            .unwrap();
        assert!(sio.ecmarkup);
        assert!(sio.source.text.starts_with(
            "The abstract operation StringIndexOf takes arguments *string* (a String)"
        ));
        assert_eq!(sio.vars.len(), 3);
    }

    #[test]
    fn variables_inside_links_are_not_intro_vars() {
        let html = r##"<div class="algorithm"><p>To <dfn id="a">run it</dfn> given <a href="#x"><var>v</var></a> and <var id="w-id">w</var>:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = intros(html, "DOM");
        let a = all.iter().find(|i| i.anchor == "a").unwrap();
        assert_eq!(a.dfn_text, "run it");
        let span = a.dfn_span.unwrap();
        assert_eq!(&a.source.text[span.start..span.end], "run it");
        assert_eq!(a.vars.len(), 1);
        assert_eq!(a.vars[0].name, "w");
        assert_eq!(
            &a.source.text[a.vars[0].span.start..a.vars[0].span.end],
            "*w*"
        );
        assert_eq!(a.vars[0].anchor.as_ref().unwrap().anchor, "w-id");
        assert!(!a.vars[0].param_dfn);
    }

    #[test]
    fn prose_intro_uses_the_source_text_and_the_dfn_owner() {
        let html = r##"<p>The <dfn data-dfn-for="Foo" id="dom-foo-bar"><code>bar(x)</code></dfn> method steps are to <a href="#r">run <var>y</var></a> with <var>z</var>.</p>"##;
        let document = Html::parse_document(html);
        let index = IdIndex::new(&document);
        let p = document
            .root_element()
            .descendent_elements()
            .find(|e| e.value().name() == "p")
            .unwrap();
        let ctx = InlineContext {
            spec: "DOM",
            base_url: crate::state::testing::base_url("DOM"),
            snapshot_sha: "hash:t",
            anchor: "dom-foo-bar",
        };
        let (text, tokens, links) = crate::parse::steps::canonical_inline(&p, &ctx);
        let subject = AnchorTarget {
            spec: "DOM".into(),
            anchor: "dom-foo-bar".into(),
        };
        let source = StatementSource {
            id: "prose-x".into(),
            subject,
            context: SourceContext::Prose {
                node_id: "prose-x".into(),
                step_path: None,
                role: ProseRole::Method,
            },
            text,
            tokens,
            links,
        };
        let intro = prose_intro(&index, &source, ProseRole::Method).unwrap();
        assert_eq!(intro.source, source);
        assert_eq!(intro.anchor, "dom-foo-bar");
        assert_eq!(intro.dfn_for.as_deref(), Some("Foo"));
        assert_eq!(intro.dfn_span, None);
        assert_eq!(intro.prose_role, Some(ProseRole::Method));
        assert!(!intro.ecmarkup);
        assert_eq!(
            intro.vars,
            [IntroVar {
                span: source
                    .tokens
                    .iter()
                    .find(|t| t.source_text == "z")
                    .unwrap()
                    .span,
                name: "z".into(),
                anchor: None,
                param_dfn: false,
            }]
        );
    }

    #[test]
    fn intro_ids_are_content_stable() {
        let a = intros(NAV_HTML, "HTML");
        let b = intros(&format!("<p>unrelated</p>{NAV_HTML}"), "HTML");
        assert_eq!(
            a.iter().map(|i| &i.source.id).collect::<Vec<_>>(),
            b.iter().map(|i| &i.source.id).collect::<Vec<_>>()
        );
    }
}
