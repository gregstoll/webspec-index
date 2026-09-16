//! Markdown to HTML for section content, with spec links turned into app routes.

use pulldown_cmark::{html, BlockQuoteKind, CowStr, Event, Options, Parser, Tag, TagEnd};

pub struct LinkTarget {
    pub spec: String,
    pub anchor: String,
}

/// Rewrites links whose URL resolves to an indexed spec into `#/SPEC/anchor`; other URLs stay unchanged.
pub fn markdown_to_html(markdown: &str, resolve: &dyn Fn(&str) -> Option<LinkTarget>) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut events: Vec<Event> = Parser::new_ext(markdown, options)
        .map(|event| match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest_url = match resolve(&dest_url) {
                    Some(target) => CowStr::from(format!("#/{}/{}", target.spec, target.anchor)),
                    None => dest_url,
                };
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                })
            }
            other => other,
        })
        .collect();
    classify_admonitions(&mut events);
    let mut out = String::with_capacity(markdown.len() * 2);
    html::push_html(&mut out, events.into_iter());
    out
}

/// The spec parser renders notes, examples, warnings and issues as blockquotes whose
/// first words are a bold label (`> **Note:** …`). Give those blockquotes the matching
/// alert kind so the HTML carries a class the stylesheet can turn into a callout box.
fn classify_admonitions(events: &mut [Event]) {
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::BlockQuote(None)) = events[i] {
            let label = match (events.get(i + 1), events.get(i + 2), events.get(i + 3)) {
                (
                    Some(Event::Start(Tag::Paragraph)),
                    Some(Event::Start(Tag::Strong)),
                    Some(Event::Text(text)),
                ) => Some(text.as_ref()),
                _ => None,
            };
            let kind = match label {
                Some("Note:") => Some(BlockQuoteKind::Note),
                Some("Example:") => Some(BlockQuoteKind::Tip),
                Some("Warning:") => Some(BlockQuoteKind::Warning),
                Some("Issue:") => Some(BlockQuoteKind::Important),
                _ => None,
            };
            if let Some(kind) = kind {
                events[i] = Event::Start(Tag::BlockQuote(Some(kind)));
                let mut depth = 0usize;
                for event in events[i..].iter_mut() {
                    match event {
                        Event::Start(Tag::BlockQuote(_)) => depth += 1,
                        Event::End(TagEnd::BlockQuote(_)) => {
                            depth -= 1;
                            if depth == 0 {
                                *event = Event::End(TagEnd::BlockQuote(Some(kind)));
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        i += 1;
    }
}

/// Renders the stored text of an IDL section as a code block. The text carries
/// markdown links for the CLI; the block form needs the plain identifiers.
pub fn idl_to_html(idl: &str) -> String {
    let mut plain = String::with_capacity(idl.len());
    let mut rest = idl;
    while let Some(open) = rest.find('[') {
        let candidate = &rest[open..];
        match candidate
            .find("](")
            .and_then(|close| candidate[close..].find(')').map(|end| (close, close + end)))
        {
            Some((close, end)) if !candidate[1..close].contains('[') => {
                plain.push_str(&rest[..open]);
                plain.push_str(&candidate[1..close]);
                rest = &candidate[end + 1..];
            }
            _ => {
                plain.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    plain.push_str(rest);
    let plain = plain
        .replace("**", "")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<pre class=\"idl\"><code class=\"language-webidl\">{plain}</code></pre>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labelled_blockquotes_become_alert_boxes() {
        let html = markdown_to_html(
            "> **Note:** The name can be cleared.\n\nText.\n\n> **Warning:** Careful.\n\n> Plain quote.",
            &|_| None,
        );
        assert!(
            html.contains(r#"<blockquote class="markdown-alert-note">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<blockquote class="markdown-alert-warning">"#),
            "{html}"
        );
        assert!(html.contains("<blockquote>\n<p>Plain quote.</p>"), "{html}");
        assert!(html.contains("<strong>Note:</strong> The name"), "{html}");
    }

    #[test]
    fn idl_sections_render_as_plain_code() {
        let html = idl_to_html(
            "[[Exposed](https://webidl.spec.whatwg.org/#Exposed)=Window]\ninterface **`Node`** : [EventTarget](https://dom.spec.whatwg.org#eventtarget) {\n  const [unsigned short](https://webidl.spec.whatwg.org/#idl-unsigned-short) ELEMENT_NODE = 1;\n  attribute DOMString? x; // legacy <T>\n};",
        );
        assert_eq!(
            html,
            "<pre class=\"idl\"><code class=\"language-webidl\">[Exposed=Window]\ninterface `Node` : EventTarget {\n  const unsigned short ELEMENT_NODE = 1;\n  attribute DOMString? x; // legacy &lt;T&gt;\n};</code></pre>"
        );
    }

    fn resolver(url: &str) -> Option<LinkTarget> {
        url.strip_prefix("https://dom.spec.whatwg.org/#")
            .map(|anchor| LinkTarget {
                spec: "DOM".into(),
                anchor: anchor.into(),
            })
    }

    #[test]
    fn indexed_spec_links_become_app_routes() {
        let html = markdown_to_html(
            "See [tree](https://dom.spec.whatwg.org/#concept-tree).",
            &resolver,
        );
        assert!(
            html.contains(r##"<a href="#/DOM/concept-tree">tree</a>"##),
            "{html}"
        );
    }

    #[test]
    fn other_links_stay_absolute() {
        let html = markdown_to_html(
            "See [grid](https://www.w3.org/TR/css-grid-1/#grid).",
            &resolver,
        );
        assert!(
            html.contains(r#"<a href="https://www.w3.org/TR/css-grid-1/#grid">grid</a>"#),
            "{html}"
        );
    }

    #[test]
    fn ordered_lists_and_emphasis_render() {
        let html = markdown_to_html("1. Let *x* be **y**.\n2. Return.", &|_| None);
        assert!(
            html.contains("<ol>")
                && html.contains("<em>x</em>")
                && html.contains("<strong>y</strong>"),
            "{html}"
        );
    }
}
