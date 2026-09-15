//! Markdown to HTML for section content, with spec links turned into app routes.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

pub struct LinkTarget {
    pub spec: String,
    pub anchor: String,
}

/// Rewrites links whose URL resolves to an indexed spec into `#/SPEC/anchor`; other URLs stay unchanged.
pub fn markdown_to_html(markdown: &str, resolve: &dyn Fn(&str) -> Option<LinkTarget>) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
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
    });
    let mut out = String::with_capacity(markdown.len() * 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
