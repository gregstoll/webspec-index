//! Declaration blocks as token sequences, for the owner and type grammars.
#![allow(dead_code)]
use crate::parse::steps::{resolve_href, AnchorTarget};
use scraper::{ElementRef, Node};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockToken {
    Text(String),
    Link {
        text: String,
        target: Option<AnchorTarget>,
        code: bool,
    },
    Dfn {
        id: String,
        text: String,
        lt: Vec<String>,
        dfn_for: Option<String>,
        dfn_type: Option<String>,
        var_param: bool,
    },
    Var(String),
    Code(String),
}

const BLOCKS: [&str; 5] = ["p", "li", "dd", "dt", "td"];

pub(crate) fn innermost_block<'a>(element: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|a| BLOCKS.contains(&a.value().name()))
}

pub(crate) fn list_item<'a>(element: &ElementRef<'a>) -> Option<(ElementRef<'a>, ElementRef<'a>)> {
    let item = element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|a| matches!(a.value().name(), "li" | "dt" | "dd"))?;
    let list = item.parent().and_then(ElementRef::wrap)?;
    matches!(list.value().name(), "ul" | "dl").then_some((item, list))
}

fn is_callout(e: &ElementRef<'_>) -> bool {
    e.value()
        .classes()
        .any(|c| matches!(c, "note" | "example" | "warning" | "advisement"))
}

pub(crate) fn norm(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn flatten(block: &ElementRef<'_>, spec: &str, base_url: &str) -> Vec<BlockToken> {
    let mut out = Vec::new();
    walk(**block, spec, base_url, &mut out);
    out
}

fn walk(node: ego_tree::NodeRef<'_, Node>, spec: &str, base_url: &str, out: &mut Vec<BlockToken>) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => out.push(BlockToken::Text(t.text.to_string())),
            Node::Element(e) => {
                let Some(el) = ElementRef::wrap(child) else {
                    continue;
                };
                match e.name() {
                    "ol" | "ul" | "dl" | "pre" => {}
                    _ if is_callout(&el) => {}
                    "dfn" => out.push(dfn_token(&el)),
                    "var" => out.push(BlockToken::Var(norm(&el.text().collect::<String>()))),
                    "a" => out.push(BlockToken::Link {
                        text: norm(&el.text().collect::<String>()),
                        target: e.attr("href").and_then(|h| resolve_href(h, spec, base_url)),
                        code: false,
                    }),
                    "code" => {
                        let links: Vec<_> = el
                            .children()
                            .filter_map(ElementRef::wrap)
                            .filter(|c| c.value().name() == "a")
                            .collect();
                        let only_link = links.len() == 1
                            && norm(&el.text().collect::<String>())
                                == norm(&links[0].text().collect::<String>());
                        if only_link {
                            out.push(BlockToken::Link {
                                text: norm(&links[0].text().collect::<String>()),
                                target: links[0]
                                    .value()
                                    .attr("href")
                                    .and_then(|h| resolve_href(h, spec, base_url)),
                                code: true,
                            });
                        } else {
                            out.push(BlockToken::Code(norm(&el.text().collect::<String>())));
                        }
                    }
                    _ => walk(child, spec, base_url, out),
                }
            }
            _ => {}
        }
    }
}

fn dfn_token(el: &ElementRef<'_>) -> BlockToken {
    let v = el.value();
    BlockToken::Dfn {
        id: v.attr("id").unwrap_or_default().to_string(),
        text: norm(&el.text().collect::<String>()),
        lt: v
            .attr("data-lt")
            .map(|lt| lt.split('|').map(norm).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
        dfn_for: v.attr("data-dfn-for").map(str::to_string),
        dfn_type: v.attr("data-dfn-type").map(str::to_string),
        var_param: el
            .children()
            .filter_map(ElementRef::wrap)
            .any(|c| c.value().name() == "var"),
    }
}

pub(crate) fn token_text(t: &BlockToken) -> &str {
    match t {
        BlockToken::Text(s) | BlockToken::Var(s) | BlockToken::Code(s) => s,
        BlockToken::Link { text, .. } | BlockToken::Dfn { text, .. } => text,
    }
}

pub(crate) fn plain_text(tokens: &[BlockToken]) -> String {
    norm(&tokens.iter().map(token_text).collect::<String>())
}

pub(crate) struct Pattern {
    pub text: String,
    pub slots: Vec<usize>,
}

pub(crate) fn pattern(tokens: &[BlockToken]) -> Pattern {
    let mut raw = String::new();
    let mut slots = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let tag = match token {
            BlockToken::Text(s) => {
                raw.push_str(s);
                continue;
            }
            BlockToken::Link { .. } => 'L',
            BlockToken::Dfn { .. } => 'D',
            BlockToken::Var(_) => 'V',
            BlockToken::Code(_) => 'C',
        };
        raw.push_str(&format!("⟦{tag}{}⟧", slots.len()));
        slots.push(index);
    }
    Pattern {
        text: norm(&raw),
        slots,
    }
}

pub(crate) fn sentences(p: &Pattern) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = p.text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < p.text.len() {
        let rest = &p.text[i..];
        if rest.starts_with('⟦') {
            depth += 1;
            i += '⟦'.len_utf8();
            continue;
        }
        if rest.starts_with('⟧') {
            depth -= 1;
            i += '⟧'.len_utf8();
            continue;
        }
        if depth == 0
            && matches!(bytes[i], b'.' | b':')
            && (i + 1 == p.text.len() || bytes[i + 1] == b' ')
        {
            out.push(start..i + 1);
            start = (i + 2).min(p.text.len());
            i += 1;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    if start < p.text.len() {
        out.push(start..p.text.len())
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::{Html, Selector};

    fn block(html: &str, sel: &str) -> Vec<BlockToken> {
        let doc = Html::parse_document(html);
        let el = doc.select(&Selector::parse(sel).unwrap()).next().unwrap();
        flatten(&el, "HTML", "https://html.spec.whatwg.org/")
    }

    #[test]
    fn flattens_code_links_dfns_and_vars() {
        let t = block(
            r##"<p>Each <code><a href="#document">Document</a></code> has an <dfn id="f" data-lt="x|y">is initial <code>about:blank</code></dfn>, which is a boolean.</p>"##,
            "p",
        );
        let p = pattern(&t);
        assert_eq!(p.text, "Each ⟦L0⟧ has an ⟦D1⟧, which is a boolean.");
        assert!(
            matches!(&t[p.slots[0]], BlockToken::Link { code: true, target: Some(a), .. } if a.anchor == "document")
        );
        assert!(
            matches!(&t[p.slots[1]], BlockToken::Dfn { id, text, lt, .. } if id == "f" && text == "is initial about:blank" && lt == &["x", "y"])
        );
        assert_eq!(
            plain_text(&t),
            "Each Document has an is initial about:blank, which is a boolean."
        );
    }

    #[test]
    fn innermost_block_prefers_li_paragraph_and_list_item_finds_the_list() {
        let doc = Html::parse_document(
            r#"<ul><li><p>An <dfn id="nav-id">id</dfn>, a new unique internal value.</p></li></ul>"#,
        );
        let dfn = doc.select(&Selector::parse("dfn").unwrap()).next().unwrap();
        assert_eq!(innermost_block(&dfn).unwrap().value().name(), "p");
        let (item, list) = list_item(&dfn).unwrap();
        assert_eq!((item.value().name(), list.value().name()), ("li", "ul"));
    }

    #[test]
    fn sentences_split_on_period_and_colon_outside_placeholders() {
        let t = block(
            r#"<p>A <dfn id="n">navigable</dfn> presents a document. Each navigable has:</p>"#,
            "p",
        );
        let p = pattern(&t);
        let s = sentences(&p);
        assert_eq!(&p.text[s[1].clone()], "Each navigable has:");
    }

    #[test]
    fn parameter_dfn_is_marked() {
        let t = block(
            r#"<p><dfn data-dfn-for="navigate" id="p"><var>documentResource</var></dfn></p>"#,
            "p",
        );
        assert!(matches!(
            &t[0],
            BlockToken::Dfn {
                var_param: true,
                ..
            }
        ));
    }
}
