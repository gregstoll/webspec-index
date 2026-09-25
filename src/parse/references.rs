// Cross-reference extraction from <a> elements
use super::algorithms::step_number;
use crate::model::{ParsedReference, ParsedSection, RefKind, SectionType};
use crate::spec_registry::SpecRegistry;
use scraper::{ElementRef, Html};

/// Extract all cross-references from a parsed HTML document.
///
/// Uses a single document-order pass: walk all nodes, track the "current section"
/// (only headings and algorithms set it), and when we hit a link, attribute it to
/// that section.  Definition sub-sections are intentionally skipped for attribution
/// because they don't establish a new scope for algorithm steps and prose that
/// follow them.
pub fn extract_references(
    document: &Html,
    spec_name: &str,
    sections: &[ParsedSection],
    registry: &SpecRegistry,
) -> Vec<ParsedReference> {
    // Build lookup set of section anchors that establish reference scope.
    // Only headings and algorithms create persistent scope; definitions are
    // sub-sections that shouldn't override the enclosing algorithm/heading.
    let scope_anchors: std::collections::HashSet<&str> = sections
        .iter()
        .filter(|s| {
            matches!(
                s.section_type,
                SectionType::Heading | SectionType::Algorithm
            )
        })
        .map(|s| s.anchor.as_str())
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut references = Vec::new();
    let mut current_section: Option<String> = None;

    // Single document-order pass over all nodes
    for node_ref in document.root_element().descendants() {
        let Some(elem) = scraper::ElementRef::wrap(node_ref) else {
            continue;
        };

        // Check if this element defines a scope section
        if let Some(id) = elem.value().attr("id") {
            if scope_anchors.contains(id) {
                current_section = Some(id.to_string());
            }
        }

        // Check if this is a link worth recording
        if elem.value().name() == "a" {
            if let Some(href) = elem.value().attr("href") {
                if is_self_link(&elem) || is_biblio_ref(&elem) {
                    continue;
                }

                if let Some(ref section) = current_section {
                    if let Some((mut to_spec, to_anchor)) = parse_href(href, registry) {
                        // Resolve intra-spec placeholder to the actual spec name
                        if to_spec == "self" {
                            to_spec = spec_name.to_string();
                        }

                        let ctx = link_context(&elem);

                        // Deduplicate by call site. Step number alone is too coarse:
                        // a switch renders as a <dl> inside one step, so branches
                        // that each call the same algorithm share a step number
                        // while being distinct calls. The generator's per-reference
                        // id separates them where it exists.
                        let call_site_id = elem.value().attr("id").map(str::to_string);
                        let key = (
                            section.clone(),
                            to_spec.clone(),
                            to_anchor.clone(),
                            ctx.step_path.clone(),
                            call_site_id.clone(),
                        );
                        if seen.insert(key) {
                            references.push(ParsedReference {
                                from_anchor: section.clone(),
                                to_spec,
                                to_anchor,
                                step_path: ctx.step_path,
                                step_text: ctx.step_text,
                                guard_path: ctx.guard_path,
                                call_site_id,
                                kind: ctx.kind,
                            });
                        }
                    }
                }
            }
        }
    }

    references
}

/// Check if a link is a self-link (should be skipped)
fn is_self_link(link: &scraper::ElementRef) -> bool {
    let classes: Vec<_> = link.value().classes().collect();
    classes.contains(&"self-link")
}

/// Check if a link is a bibliography reference (should be skipped)
fn is_biblio_ref(link: &scraper::ElementRef) -> bool {
    if let Some(link_type) = link.value().attr("data-link-type") {
        link_type == "biblio"
    } else {
        false
    }
}

/// Parse an href attribute to determine the target spec and anchor
fn parse_href(href: &str, registry: &SpecRegistry) -> Option<(String, String)> {
    // Intra-spec reference (starts with #)
    if href.starts_with('#') {
        return Some(("self".to_string(), href.trim_start_matches('#').to_string()));
    }

    // Cross-spec reference (full URL)
    if href.starts_with("http://") || href.starts_with("https://") {
        // Try to resolve the URL using the registry
        if let Some((spec_name, anchor)) = registry.resolve_url(href) {
            return Some((spec_name, anchor));
        }
    }

    // Unknown or external URL, skip
    None
}

/// Where a link sits within its section: which algorithm step encloses it, what
/// that step says, which steps guard it, and whether it is a call at all.
struct LinkContext {
    step_path: Option<String>,
    step_text: Option<String>,
    guard_path: Vec<String>,
    kind: RefKind,
}

fn link_context(link: &ElementRef) -> LinkContext {
    let mut callout = false;
    let mut idl = false;
    // Enclosing <li> elements, innermost first.
    let mut items: Vec<(usize, ElementRef)> = Vec::new();

    for ancestor in link.ancestors() {
        let Some(elem) = ElementRef::wrap(ancestor) else {
            continue;
        };
        let name = elem.value().name();

        if is_callout(&elem) {
            callout = true;
        }
        if (name == "pre" || name == "code") && has_class(&elem, "idl") {
            idl = true;
        }
        if name == "li" {
            if let Some(number) = step_number(&elem) {
                items.push((number, elem));
            }
        }
    }

    items.reverse(); // outermost first

    let kind = if callout {
        RefKind::Note
    } else if idl {
        RefKind::Idl
    } else if items.is_empty() {
        RefKind::Prose
    } else {
        RefKind::Step
    };

    let step_path = if items.is_empty() {
        None
    } else {
        Some(
            items
                .iter()
                .map(|(n, _)| n.to_string())
                .collect::<Vec<_>>()
                .join("."),
        )
    };

    let guard_path = items
        .iter()
        .take(items.len().saturating_sub(1))
        .map(|(_, elem)| own_text(elem))
        .collect();

    let step_text = items.last().map(|(_, elem)| own_text(elem));

    LinkContext {
        step_path,
        step_text,
        guard_path,
        kind,
    }
}

fn is_callout(elem: &ElementRef) -> bool {
    matches!(
        elem.value().name(),
        "p" | "div" | "aside" | "details" | "blockquote"
    ) && ["note", "example", "warning", "advisement"]
        .iter()
        .any(|c| has_class(elem, c))
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "li"
            | "ul"
            | "ol"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "tr"
            | "td"
            | "th"
            | "section"
            | "aside"
            | "blockquote"
            | "pre"
            | "figure"
            | "figcaption"
    )
}

fn has_class(elem: &ElementRef, class: &str) -> bool {
    elem.value().classes().any(|c| c == class)
}

/// Text of an element excluding any nested lists, whitespace-normalized. Keeps a
/// step's own wording out of its substeps and vice versa.
fn own_text(elem: &ElementRef) -> String {
    fn collect(node: ego_tree::NodeRef<'_, scraper::Node>, out: &mut String) {
        for child in node.children() {
            match child.value() {
                scraper::Node::Text(text) => out.push_str(&text.text),
                scraper::Node::Element(element) => {
                    // Numbered substeps and switch bodies get their own step paths,
                    // and notes are commentary. A <ul> is a condition list, without
                    // which the step it guards reads as a bare "If ... then:".
                    if matches!(element.name(), "ol" | "dl") {
                        continue;
                    }
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        if is_callout(&child_elem) {
                            continue;
                        }
                    }
                    // Block boundaries are word boundaries; adjacent <li> texts
                    // would otherwise run together. Inline elements must not get
                    // spaces, or "inner." becomes "inner .".
                    let block = is_block(element.name());
                    if block {
                        out.push(' ');
                    }
                    collect(child, out);
                    if block {
                        out.push(' ');
                    }
                }
                _ => {}
            }
        }
    }

    let mut raw = String::new();
    collect(**elem, &mut raw);
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(
        html: &str,
        spec: &str,
        sections: &[ParsedSection],
        registry: &SpecRegistry,
    ) -> Vec<ParsedReference> {
        let document = Html::parse_document(html);
        extract_references(&document, spec, sections, registry)
    }

    #[test]
    fn test_intra_spec_reference() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a href="#section2">Section 2</a> for details.</p>

            <h2 id="section2">Section 2</h2>
            <p>Content here.</p>
        "##;

        let sections = vec![
            ParsedSection {
                anchor: "section1".to_string(),
                title: Some("Section 1".to_string()),
                content_text: None,
                section_type: SectionType::Heading,
                parent_anchor: None,
                prev_anchor: None,
                next_anchor: None,
                depth: Some(2),
                number: None,
            },
            ParsedSection {
                anchor: "section2".to_string(),
                title: Some("Section 2".to_string()),
                content_text: None,
                section_type: SectionType::Heading,
                parent_anchor: None,
                prev_anchor: None,
                next_anchor: None,
                depth: Some(2),
                number: None,
            },
        ];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // Should have one reference from section1 to section2
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].from_anchor, "section1");
        assert_eq!(refs[0].to_spec, "TEST");
        assert_eq!(refs[0].to_anchor, "section2");
    }

    #[test]
    fn test_skip_self_links() {
        let html = r##"
            <h2 id="section1">Section 1<a class="self-link" href="#section1"></a></h2>
            <p>Content here.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // Should have no references (self-link skipped)
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn test_skip_biblio_refs() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a data-link-type="biblio" href="#biblio-infra">[INFRA]</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // Should have no references (biblio ref skipped)
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn test_cross_spec_reference() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a href="https://dom.spec.whatwg.org/#concept-tree">tree</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        // SpecRegistry already includes WhatwgProvider
        let registry = SpecRegistry::new();

        let refs = extract(html, "TEST", &sections, &registry);

        // Should have one cross-spec reference
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].from_anchor, "section1");
        assert_eq!(refs[0].to_spec, "DOM");
        assert_eq!(refs[0].to_anchor, "concept-tree");
    }

    #[test]
    fn test_unknown_url_skipped() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a href="https://example.com/foo">external link</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // Should have no references (unknown URL skipped)
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn test_nested_sections() {
        let html = r##"
            <h2 id="parent">Parent</h2>
            <div>
                <h3 id="child">Child</h3>
                <p>See <a href="#parent">parent section</a>.</p>
            </div>
        "##;

        let sections = vec![
            ParsedSection {
                anchor: "parent".to_string(),
                title: Some("Parent".to_string()),
                content_text: None,
                section_type: SectionType::Heading,
                parent_anchor: None,
                prev_anchor: None,
                next_anchor: None,
                depth: Some(2),
                number: None,
            },
            ParsedSection {
                anchor: "child".to_string(),
                title: Some("Child".to_string()),
                content_text: None,
                section_type: SectionType::Heading,
                parent_anchor: Some("parent".to_string()),
                prev_anchor: None,
                next_anchor: None,
                depth: Some(3),
                number: None,
            },
        ];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // Should have one reference from child to parent
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].from_anchor, "child");
        assert_eq!(refs[0].to_anchor, "parent");
    }

    #[test]
    fn test_wattsi_algorithm_references() {
        // Test the Wattsi pattern where <dfn> and <a> are siblings in same <p>,
        // plus links in algorithm steps (sibling <ol>)
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn> a <a href="#navigable">navigable</a> to a
            <a href="https://url.spec.whatwg.org/#concept-url">URL</a>, with optional <a href="#post-resource">POST resource</a>:</p>
            <ol>
                <li><p>Let x be <a href="#snapshotting-params">snapshotting params</a>.</p></li>
                <li><p><a href="https://infra.spec.whatwg.org/#assert">Assert</a>: foo.</p></li>
            </ol>
        "##;

        let sections = vec![ParsedSection {
            anchor: "navigate".to_string(),
            title: Some("navigate".to_string()),
            content_text: None,
            section_type: SectionType::Algorithm,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: None,
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        assert_eq!(refs.len(), 5, "Expected 5 references, got {}", refs.len());

        // All refs should be from "navigate"
        for ref_item in &refs {
            assert_eq!(ref_item.from_anchor, "navigate");
        }

        // Check intra-spec refs
        let intra: Vec<_> = refs
            .iter()
            .filter(|r| r.to_spec == "TEST")
            .map(|r| r.to_anchor.as_str())
            .collect();
        assert!(intra.contains(&"navigable"));
        assert!(intra.contains(&"post-resource"));
        assert!(intra.contains(&"snapshotting-params"));

        // Check cross-spec refs
        assert!(refs
            .iter()
            .any(|r| r.to_spec == "URL" && r.to_anchor == "concept-url"));
        assert!(refs
            .iter()
            .any(|r| r.to_spec == "INFRA" && r.to_anchor == "assert"));
    }

    #[test]
    fn test_algorithm_with_parameter_dfns() {
        // Real-world pattern: algorithm intro has parameter dfns that should NOT
        // steal attribution from the algorithm for links in the steps.
        let html = r##"
            <div data-algorithm="">
            <p>To <dfn id="navigate">navigate</dfn> a <a href="#navigable">navigable</a>
            using <dfn id="navigation-resource">documentResource</dfn> and
            <dfn id="navigation-response">response</dfn>:</p>
            <ol>
                <li><p><a href="#assert">Assert</a>: stuff.</p></li>
                <li><p>Let x be <a href="#snapshot">snapshot</a>.</p></li>
            </ol>
            </div>
        "##;

        let sections = vec![
            ParsedSection {
                anchor: "navigate".to_string(),
                title: Some("navigate".to_string()),
                content_text: None,
                section_type: SectionType::Algorithm,
                parent_anchor: None,
                prev_anchor: None,
                next_anchor: None,
                depth: None,
                number: None,
            },
            ParsedSection {
                anchor: "navigation-resource".to_string(),
                title: Some("documentResource".to_string()),
                content_text: None,
                section_type: SectionType::Definition,
                parent_anchor: Some("navigate".to_string()),
                prev_anchor: None,
                next_anchor: None,
                depth: None,
                number: None,
            },
            ParsedSection {
                anchor: "navigation-response".to_string(),
                title: Some("response".to_string()),
                content_text: None,
                section_type: SectionType::Definition,
                parent_anchor: Some("navigate".to_string()),
                prev_anchor: None,
                next_anchor: None,
                depth: None,
                number: None,
            },
        ];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        // ALL links (including those in <ol> steps) should be attributed to "navigate",
        // not to the parameter definitions
        assert_eq!(refs.len(), 3, "Expected 3 references, got {}", refs.len());
        for ref_item in &refs {
            assert_eq!(
                ref_item.from_anchor, "navigate",
                "Link to {} should be from navigate, not {}",
                ref_item.to_anchor, ref_item.from_anchor
            );
        }
    }

    #[test]
    fn test_cross_spec_reference_to_w3c() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a href="https://drafts.csswg.org/selectors-4/#specificity">specificity</a>.</p>
            <p>Also <a href="https://w3c.github.io/ServiceWorker/#service-worker-concept">SW</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        assert_eq!(refs.len(), 2);
        assert!(refs
            .iter()
            .any(|r| r.to_spec == "SELECTORS-4" && r.to_anchor == "specificity"));
        assert!(refs
            .iter()
            .any(|r| r.to_spec == "SERVICEWORKER" && r.to_anchor == "service-worker-concept"));
    }

    #[test]
    fn test_cross_spec_reference_to_tc39() {
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>Call <a href="https://tc39.es/ecma262/#sec-tostring">ToString</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].to_spec, "ECMA-262");
        assert_eq!(refs[0].to_anchor, "sec-tostring");
    }

    #[test]
    fn test_duplicate_refs_deduplicated() {
        // Same anchor linked multiple times from the same section → single ref
        let html = r##"
            <h2 id="section1">Section 1</h2>
            <p>See <a href="#target">target</a> and also <a href="#target">target again</a>.</p>
        "##;

        let sections = vec![ParsedSection {
            anchor: "section1".to_string(),
            title: Some("Section 1".to_string()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }];

        let registry = SpecRegistry::new();
        let refs = extract(html, "TEST", &sections, &registry);

        assert_eq!(refs.len(), 1, "Duplicate ref should be deduplicated");
        assert_eq!(refs[0].to_anchor, "target");
    }

    fn algo_section(anchor: &str) -> ParsedSection {
        ParsedSection {
            anchor: anchor.to_string(),
            title: Some(anchor.to_string()),
            content_text: None,
            section_type: SectionType::Algorithm,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: None,
            number: None,
        }
    }

    fn find<'a>(refs: &'a [ParsedReference], anchor: &str) -> &'a ParsedReference {
        refs.iter()
            .find(|r| r.to_anchor == anchor)
            .unwrap_or_else(|| panic!("no ref to {anchor} in {refs:#?}"))
    }

    const NESTED_ALGORITHM: &str = r##"
        <p>To <dfn id="navigate">navigate</dfn>:</p>
        <ol>
            <li><p>Let x be <a href="#first">first</a>.</p></li>
            <li><p>If <a href="#cond">cond</a> is true, run these steps:</p>
                <ol>
                    <li><p>Call <a href="#inner">inner</a>.</p></li>
                    <li><p>Return <a href="#result">result</a>.</p></li>
                </ol>
            </li>
            <li><p>Finally <a href="#last">last</a>.</p></li>
        </ol>
    "##;

    #[test]
    fn test_step_path_top_level() {
        let refs = extract(
            NESTED_ALGORITHM,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(find(&refs, "first").step_path.as_deref(), Some("1"));
        assert_eq!(find(&refs, "cond").step_path.as_deref(), Some("2"));
        assert_eq!(find(&refs, "last").step_path.as_deref(), Some("3"));
    }

    #[test]
    fn test_step_path_nested() {
        let refs = extract(
            NESTED_ALGORITHM,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(find(&refs, "inner").step_path.as_deref(), Some("2.1"));
        assert_eq!(find(&refs, "result").step_path.as_deref(), Some("2.2"));
    }

    #[test]
    fn test_step_text_excludes_substeps() {
        let refs = extract(
            NESTED_ALGORITHM,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        let cond = find(&refs, "cond");
        assert_eq!(
            cond.step_text.as_deref(),
            Some("If cond is true, run these steps:"),
            "step text must not absorb its substeps"
        );
        assert_eq!(
            find(&refs, "inner").step_text.as_deref(),
            Some("Call inner.")
        );
    }

    #[test]
    fn test_guard_path_from_enclosing_steps() {
        let refs = extract(
            NESTED_ALGORITHM,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert!(
            find(&refs, "first").guard_path.is_empty(),
            "top-level step has no guard"
        );
        assert_eq!(
            find(&refs, "inner").guard_path,
            vec!["If cond is true, run these steps:".to_string()],
            "nested step carries its enclosing step as guard"
        );
    }

    #[test]
    fn test_prose_reference_has_no_step() {
        let refs = extract(
            NESTED_ALGORITHM,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        // The intro <p> is prose, not a step. It contains no links here, so verify
        // via a section whose only link sits outside any list.
        let prose = extract(
            r##"<h2 id="s">S</h2><p>See <a href="#other">other</a>.</p>"##,
            "TEST",
            &[ParsedSection {
                section_type: SectionType::Heading,
                ..algo_section("s")
            }],
            &SpecRegistry::new(),
        );

        assert_eq!(find(&prose, "other").kind, RefKind::Prose);
        assert!(find(&prose, "other").step_path.is_none());
        assert_eq!(find(&refs, "first").kind, RefKind::Step);
    }

    #[test]
    fn test_step_paths_across_spec_generators() {
        // Every generator nests algorithm steps as <ol><li>, but wraps them
        // differently: wattsi bare, bikeshed inside div.algorithm, ecmarkup inside
        // <emu-alg> with links buried in <emu-xref>. Step numbering must not care.
        let wattsi = include_str!("../../tests/fixtures/algorithms/wattsi_navigate.html");
        let refs = extract(
            wattsi,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );
        assert_eq!(
            find(&refs, "snapshotting-source-snapshot-params")
                .step_path
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            find(&refs, "starting-an-unload").step_path.as_deref(),
            Some("4.1"),
            "wattsi nested step"
        );
        assert_eq!(
            find(&refs, "starting-an-unload").guard_path.len(),
            1,
            "nested step keeps its parent as guard"
        );

        let bikeshed = include_str!("../../tests/fixtures/algorithms/bikeshed_algorithm.html");
        let refs = extract(
            bikeshed,
            "TEST",
            &[algo_section("concept-ordered-set-parser")],
            &SpecRegistry::new(),
        );
        assert!(
            refs.iter()
                .all(|r| r.kind == RefKind::Step || r.step_path.is_none()),
            "bikeshed: {refs:#?}"
        );

        let ecmarkup = include_str!("../../tests/fixtures/ecmarkup/tostring.html");
        let refs = extract(
            ecmarkup,
            "TEST",
            &[algo_section("sec-tostring")],
            &SpecRegistry::new(),
        );
        let assert_ref = find(&refs, "assert");
        assert_eq!(
            assert_ref.step_path.as_deref(),
            Some("9"),
            "ecmarkup step inside <emu-alg><ol>, link nested in <emu-xref>"
        );
        assert_eq!(assert_ref.kind, RefKind::Step);
        assert_eq!(
            find(&refs, "sec-toprimitive").step_path.as_deref(),
            Some("10")
        );
    }

    #[test]
    fn test_switch_body_attributes_to_enclosing_step() {
        // Wattsi renders switches as <dl class=switch>. Branches are not numbered
        // steps, so a link inside one belongs to the step containing the switch,
        // and the branch condition must not leak into that step's text.
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Switch on url's scheme:</p>
                    <dl class="switch">
                        <dt>"about"</dt>
                        <dd><p>Call <a href="#about-handler">about handler</a>.</p></dd>
                        <dt>Otherwise</dt>
                        <dd><p>Call <a href="#fetch-handler">fetch handler</a>.</p></dd>
                    </dl>
                </li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        for anchor in ["about-handler", "fetch-handler"] {
            let r = find(&refs, anchor);
            assert_eq!(r.step_path.as_deref(), Some("1"), "{anchor}");
            assert_eq!(r.kind, RefKind::Step, "{anchor}");
            assert_eq!(
                r.step_text.as_deref(),
                Some("Switch on url's scheme:"),
                "switch branches must not be absorbed into the step text"
            );
        }
    }

    #[test]
    fn test_step_text_excludes_notes() {
        // Wattsi puts notes inside the step they annotate. Absorbing them makes
        // quoted step text unusable — real notes run to hundreds of words.
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>While cond is true:</p>
                    <p class="note">This is a loop, since aborting can run JavaScript.</p>
                    <ol>
                        <li><p>Call <a href="#inner">inner</a>.</p></li>
                    </ol>
                </li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(
            find(&refs, "inner").guard_path,
            vec!["While cond is true:".to_string()]
        );
    }

    #[test]
    fn test_step_text_keeps_condition_bullets() {
        // A step whose condition is spelled out in a <ul> is meaningless without
        // the bullets, and those bullets are not substeps.
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>If all of the following are true:</p>
                    <ul>
                        <li>documentResource is null;</li>
                        <li>response is null,</li>
                    </ul>
                    <p>then:</p>
                    <ol>
                        <li><p>Call <a href="#frag">frag</a>.</p></li>
                    </ol>
                </li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        let guard = &find(&refs, "frag").guard_path[0];
        assert_eq!(
            guard,
            "If all of the following are true: documentResource is null; response is null, then:",
            "condition bullets belong in the guard, separated at element boundaries"
        );
    }

    #[test]
    fn test_note_inside_step_is_not_a_call() {
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Call <a href="#real">real</a>.</p>
                    <p class="note">See <a href="#mentioned">mentioned</a> for context.</p>
                </li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(find(&refs, "real").kind, RefKind::Step);
        assert_eq!(
            find(&refs, "mentioned").kind,
            RefKind::Note,
            "a link inside a note is a mention, not a call, even within a step"
        );
    }

    #[test]
    fn test_idl_block_reference() {
        let html = r##"
            <h2 id="iface">Interface</h2>
            <pre class="idl">interface <a href="#thing">Thing</a> {};</pre>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[ParsedSection {
                section_type: SectionType::Heading,
                ..algo_section("iface")
            }],
            &SpecRegistry::new(),
        );

        assert_eq!(find(&refs, "thing").kind, RefKind::Idl);
    }

    #[test]
    fn test_call_site_id_captured() {
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Call <a id="navigate:target-3" href="#target">target</a>.</p></li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(
            find(&refs, "target").call_site_id.as_deref(),
            Some("navigate:target-3")
        );
    }

    #[test]
    fn test_same_target_at_different_steps_kept_separately() {
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Call <a href="#target">target</a>.</p></li>
                <li><p>Call <a href="#target">target</a> again.</p></li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        let steps: Vec<_> = refs
            .iter()
            .filter(|r| r.to_anchor == "target")
            .filter_map(|r| r.step_path.clone())
            .collect();
        assert_eq!(
            steps,
            vec!["1".to_string(), "2".to_string()],
            "distinct call sites must survive deduplication"
        );
    }

    #[test]
    fn test_switch_branches_calling_the_same_target_are_distinct_call_sites() {
        // A switch lives inside one step, so its branches share a step number.
        // They are still separate calls, and the generator's per-reference ids
        // say so — collapsing them loses a call site from the trace.
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Switch on state:</p>
                    <dl class="switch">
                        <dt>"broken"</dt>
                        <dd><p><a id="navigate:abort-3" href="#abort">Abort</a> the request.</p></dd>
                        <dt>Otherwise</dt>
                        <dd><p><a id="navigate:abort-4" href="#abort">Abort</a> it too.</p></dd>
                    </dl>
                </li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        let mut ids: Vec<&str> = refs
            .iter()
            .filter(|r| r.to_anchor == "abort")
            .filter_map(|r| r.call_site_id.as_deref())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["navigate:abort-3", "navigate:abort-4"]);
    }

    #[test]
    fn test_same_target_in_same_step_deduplicated() {
        let html = r##"
            <p>To <dfn id="navigate">navigate</dfn>:</p>
            <ol>
                <li><p>Call <a href="#target">target</a> then <a href="#target">target</a>.</p></li>
            </ol>
        "##;

        let refs = extract(
            html,
            "TEST",
            &[algo_section("navigate")],
            &SpecRegistry::new(),
        );

        assert_eq!(
            refs.iter().filter(|r| r.to_anchor == "target").count(),
            1,
            "same target twice in one step is one call site"
        );
    }
}
