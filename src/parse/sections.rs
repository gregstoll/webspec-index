use super::markdown::Converter;
use crate::model::{ParsedSection, SectionType};
use anyhow::Result;
use regex::Regex;
#[cfg(test)]
use scraper::{Html, Selector};
use std::sync::OnceLock;

/// Extract content between a heading and the next section (heading or dfn)
/// Returns the markdown-converted prose content
fn extract_heading_content(
    heading: &scraper::ElementRef,
    current_depth: u8,
    converter: &Converter,
) -> Option<String> {
    use super::markdown;

    let mut content_html = String::new();
    let mut current = heading.next_sibling();

    while let Some(node) = current {
        if let Some(sibling_elem) = scraper::ElementRef::wrap(node) {
            let tag_name = sibling_elem.value().name();

            // Stop at next heading of same or higher level
            if let Some(sibling_depth) = heading_depth(tag_name) {
                if sibling_depth <= current_depth {
                    break;
                }
            }

            // Stop at definitions (they're separate sections)
            if tag_name == "dfn" && sibling_elem.value().attr("id").is_some() {
                break;
            }

            // Collect this element's HTML
            content_html.push_str(&sibling_elem.html());
        }

        current = node.next_sibling();
    }

    if content_html.trim().is_empty() {
        return None;
    }

    let markdown = markdown::element_to_markdown_from_html(&content_html, converter);
    let trimmed = markdown.trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Extract title text from a heading element, stripping secno and self-link
fn extract_heading_title(element: &scraper::ElementRef) -> Option<String> {
    // Clone the element to manipulate it
    let mut text_parts = Vec::new();

    for node in element.children() {
        if let Some(elem) = scraper::ElementRef::wrap(node) {
            // Skip section number spans and self-links:
            // - "secno" (Bikeshed/Wattsi), "secnum" (ecmarkup/TC39), "self-link"
            let classes = elem.value().classes().collect::<Vec<_>>();
            if classes.contains(&"secno")
                || classes.contains(&"secnum")
                || classes.contains(&"self-link")
            {
                continue;
            }
            // Get text from other elements (like <span class="content">)
            text_parts.push(elem.text().collect::<String>());
        } else if let Some(text) = node.value().as_text() {
            text_parts.push(text.to_string());
        }
    }

    let result = text_parts.join("").trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Extract the section number from a heading element.
/// Returns the text of the first `span.secno` (Bikeshed/Wattsi) or `span.secnum` (ecmarkup),
/// trimmed and with a trailing `.` removed.
/// Examples: "4.7.1." → "4.7.1", "7.1.17" → "7.1.17", "§ 4.7.1." → "§ 4.7.1".
pub(crate) fn extract_heading_number(element: &scraper::ElementRef) -> Option<String> {
    for child in element.children() {
        if let Some(elem) = scraper::ElementRef::wrap(child) {
            let classes: Vec<_> = elem.value().classes().collect();
            if classes.contains(&"secno") || classes.contains(&"secnum") {
                let text = elem.text().collect::<String>();
                let text = text.trim();
                if text.is_empty() {
                    return None;
                }
                let text = text.strip_suffix('.').unwrap_or(text).trim().to_string();
                if text.is_empty() {
                    return None;
                }
                return Some(text);
            }
        }
    }
    None
}

/// Get the depth (2-6) from a heading tag name
fn heading_depth(tag: &str) -> Option<u8> {
    match tag {
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

/// Parse a single heading element into a ParsedSection
pub fn parse_heading_element(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Result<Option<ParsedSection>> {
    let anchor = match element.value().attr("id") {
        Some(id) => id.to_string(),
        None => return Ok(None), // No id, skip this heading
    };

    let title = extract_heading_title(element);
    let number = extract_heading_number(element);
    let depth = heading_depth(element.value().name())
        .ok_or_else(|| anyhow::anyhow!("Invalid heading tag: {}", element.value().name()))?;

    // Extract content between this heading and the next heading/definition
    let content_text = extract_heading_content(element, depth, converter);

    Ok(Some(ParsedSection {
        anchor,
        title,
        content_text,
        section_type: SectionType::Heading,
        parent_anchor: None,
        prev_anchor: None,
        next_anchor: None,
        depth: Some(depth),
        number,
    }))
}

/// Parse a single dfn element into a ParsedSection
/// Determines whether it's a Definition, Algorithm, or IDL based on context
pub fn parse_dfn_element(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Result<Option<ParsedSection>> {
    let anchor = match element.value().attr("id") {
        Some(id) => id.to_string(),
        None => return Ok(None), // No id, skip this dfn
    };

    // Skip dfns that are inside algorithm content (e.g., inside <ol> steps)
    // These are part of the algorithm's markdown content, not separate sections
    if is_inside_algorithm_content(element) {
        return Ok(None);
    }

    // Skip parameter dfns. Each generator marks them explicitly, so both checks are
    // positive signals rather than guesses:
    // - Wattsi wraps the parameter name in <var>: <dfn data-dfn-for="navigate"><var>url</var></dfn>
    // - Bikeshed sets data-dfn-type="argument": <dfn data-dfn-for="Window/open(url)" data-dfn-type="argument">url</dfn>
    // data-dfn-for alone means nothing here: Wattsi puts it on exported concepts such as
    // <dfn data-dfn-for="navigable" id="nav-document" data-export>active document</dfn>.
    // The <var> must be the whole name: an algorithm's own name may mention a parameter, as in
    // <dfn id="fire-a-synthetic-pointer-event">Firing a synthetic pointer event named <var>e</var></dfn>.
    if is_var_only(element) || element.value().attr("data-dfn-type") == Some("argument") {
        return Ok(None);
    }

    // Extract text content (including nested elements like <code>)
    let title = element.text().collect::<String>().trim().to_string();
    let title = if title.is_empty() { None } else { Some(title) };

    // Determine section type based on context
    // (parameter dfns already skipped above)
    let section_type = if is_algorithm_dfn(element) {
        SectionType::Algorithm
    } else if is_idl_type(element) {
        SectionType::Idl
    } else {
        SectionType::Definition
    };

    // Extract content based on section type
    let content_text = match section_type {
        SectionType::Definition => extract_definition_content(element, converter),
        SectionType::Algorithm => extract_algorithm_content(element, converter),
        SectionType::Idl => extract_idl_content(element),
        _ => None,
    };
    let content_text = match (
        content_text,
        continuation_content(element, &anchor, converter),
    ) {
        (Some(own), Some(more)) => Some(format!("{}\n\n{more}", own.trim_end())),
        (content, _) => content,
    };

    Ok(Some(ParsedSection {
        anchor,
        title,
        content_text,
        section_type,
        parent_anchor: None,
        prev_anchor: None,
        next_anchor: None,
        depth: None,
        number: None,
    }))
}

/// Steps given for this dfn right after its definition, in paragraphs that link back to it
/// rather than defining it again: an IDL attribute's setter after its getter.
fn continuation_content(
    element: &scraper::ElementRef,
    anchor: &str,
    converter: &Converter,
) -> Option<String> {
    use super::markdown;

    let block = enclosing_block(element)?;
    let is_list = |e: &scraper::ElementRef| matches!(e.value().name(), "ol" | "ul" | "dl");
    let mut last = std::iter::once(*block)
        .chain(block.ancestors())
        .filter_map(scraper::ElementRef::wrap)
        .find(is_algorithm_div)
        .unwrap_or(block);
    if let Some(list) = last
        .next_siblings()
        .find_map(scraper::ElementRef::wrap)
        .filter(is_list)
    {
        last = list;
    }

    let mut parts = Vec::new();
    while let Some(next) = last.next_siblings().find_map(scraper::ElementRef::wrap) {
        if is_algorithm_div(&next) {
            let intro = next.children().find_map(scraper::ElementRef::wrap);
            if intro.and_then(|p| continued_anchor(&p)) != Some(anchor) {
                break;
            }
            parts.extend(extract_from_algorithm_div(&next, converter));
            last = next;
        } else if continued_anchor(&next) == Some(anchor) {
            let intro = markdown::element_to_markdown(&next, converter);
            last = next;
            match next
                .next_siblings()
                .find_map(scraper::ElementRef::wrap)
                .filter(is_list)
            {
                Some(list) => {
                    parts.push(format!(
                        "{}\n\n{}",
                        intro.trim(),
                        render_list(&list, converter)
                    ));
                    last = list;
                }
                None => parts.push(intro.trim().to_string()),
            }
        } else {
            break;
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Extract content for a definition (dfn not in algorithm, not IDL)
/// Finds the enclosing block-level element and converts to markdown
fn extract_definition_content(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Option<String> {
    use super::markdown;

    // Find the enclosing block-level element (p, div, dd, etc.)
    let mut current = element.parent();
    while let Some(node) = current {
        if let Some(parent_elem) = scraper::ElementRef::wrap(node) {
            let tag_name = parent_elem.value().name();
            // Block-level elements that can contain definitions
            if matches!(tag_name, "p" | "div" | "dd" | "dt" | "li" | "section") {
                if let Some(item) = sole_definition_of_list_item(&parent_elem) {
                    // Render the children, not the <li>/<dd> itself, to keep the leading
                    // bullet out of the definition's content.
                    return Some(markdown::element_to_markdown_from_html(
                        &item.inner_html(),
                        converter,
                    ));
                }
                let own = markdown::element_to_markdown(&parent_elem, converter);
                return Some(match introduced_list(&parent_elem) {
                    Some(list) => format!(
                        "{}\n\n{}",
                        own.trim(),
                        render_list(&list, converter).trim_end()
                    ),
                    None => own,
                });
            }
        }
        current = node.parent();
    }

    // Fallback: just use the dfn's text
    Some(element.text().collect::<String>().trim().to_string())
}

/// The list a definition's paragraph introduces with a trailing colon ("A `StaticRange` is
/// valid if all of the following are true:"), which completes the definition. A list that
/// defines terms of its own ("Each navigable has:") is left to those terms.
fn introduced_list<'a>(block: &scraper::ElementRef<'a>) -> Option<scraper::ElementRef<'a>> {
    if block.value().name() != "p" || !block_text(block).ends_with(':') {
        return None;
    }
    let list = block
        .next_siblings()
        .find_map(scraper::ElementRef::wrap)
        .filter(|e| matches!(e.value().name(), "ol" | "ul" | "dl"))?;
    let dfn_selector = scraper::Selector::parse("dfn[id]").ok()?;
    list.select(&dfn_selector).next().is_none().then_some(list)
}

fn render_list(list: &scraper::ElementRef, converter: &Converter) -> String {
    use super::algorithms;
    match list.value().name() {
        "ol" => algorithms::render_algorithm_ol(list, converter),
        "dl" => algorithms::render_algorithm_dl(list, converter),
        _ => algorithms::render_ul(list, 0, converter),
    }
}

/// Given the block that directly encloses a dfn, return the <li>/<dd> the definition owns
/// outright, if there is one.
///
/// A Wattsi property list item spreads a definition over several paragraphs — the definition,
/// then notes and caveats about it — so the enclosing <p> alone truncates it. The list item is
/// only the right scope when it defines a single term; with several dfns the narrower <p> keeps
/// them from collapsing onto identical content.
fn sole_definition_of_list_item<'a>(
    block: &scraper::ElementRef<'a>,
) -> Option<scraper::ElementRef<'a>> {
    if block.value().name() != "p" {
        return None;
    }
    let item = scraper::ElementRef::wrap(block.parent()?)?;
    if !matches!(item.value().name(), "li" | "dd") {
        return None;
    }
    let dfn_selector = scraper::Selector::parse("dfn[id]").ok()?;
    (item.select(&dfn_selector).count() == 1).then_some(item)
}

/// Extract content for an algorithm (dfn inside div.algorithm or with sibling <ol>)
/// Handles both Bikeshed (div.algorithm) and Wattsi (sibling ol) patterns
fn extract_algorithm_content(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Option<String> {
    use super::{algorithms, markdown};

    let mut current = element.parent();
    while let Some(node) = current {
        if let Some(parent_elem) = scraper::ElementRef::wrap(node) {
            // Bikeshed/Wattsi div pattern: div.algorithm or div[data-algorithm]
            if parent_elem.value().name() == "div" {
                let classes: Vec<_> = parent_elem.value().classes().collect();
                let is_algo_div = classes.contains(&"algorithm")
                    || parent_elem.value().attr("data-algorithm").is_some();
                if is_algo_div {
                    return extract_from_algorithm_div(&parent_elem, converter);
                }
            }

            // Wattsi sibling pattern: <p>To <dfn>foo</dfn>:</p><ol>...</ol>
            if matches!(parent_elem.value().name(), "p" | "dd" | "li") {
                let intro = markdown::element_to_markdown(&parent_elem, converter);

                let mut sibling = node.next_sibling();
                while let Some(sib_node) = sibling {
                    if let Some(sib_elem) = scraper::ElementRef::wrap(sib_node) {
                        match sib_elem.value().name() {
                            "ol" => {
                                let steps = algorithms::render_algorithm_ol(&sib_elem, converter);
                                return Some(format!("{}\n\n{}", intro.trim(), steps));
                            }
                            "ul" => {
                                let steps = algorithms::render_ul(&sib_elem, 0, converter);
                                return Some(format!("{}\n\n{}", intro.trim(), steps));
                            }
                            "dl" => {
                                let steps = algorithms::render_algorithm_dl(&sib_elem, converter);
                                return Some(format!("{}\n\n{}", intro.trim(), steps));
                            }
                            "p" | "div" | "h2" | "h3" | "h4" | "h5" | "h6" => break,
                            _ => {}
                        }
                    }
                    sibling = sib_node.next_sibling();
                }
            }
        }
        current = node.parent();
    }

    // A one-sentence algorithm ("The foo() method steps are to …") is its own body.
    extract_definition_content(element, converter)
}

/// Extract algorithm content from a div.algorithm or div[data-algorithm] container.
/// Properly separates the intro paragraph(s) from the list body (<ol>, <ul>, or <dl>).
fn extract_from_algorithm_div(div: &scraper::ElementRef, converter: &Converter) -> Option<String> {
    use super::{algorithms, markdown};

    // Find the first list element (<ol>, <ul>, or <dl>)
    let list_tag = div.children().find_map(|child| {
        scraper::ElementRef::wrap(child).filter(|e| matches!(e.value().name(), "ol" | "ul" | "dl"))
    });

    // Build intro HTML from children before the first list
    let mut intro_html = String::new();
    for child in div.children() {
        if let Some(child_elem) = scraper::ElementRef::wrap(child) {
            if matches!(child_elem.value().name(), "ol" | "ul" | "dl") {
                break;
            }
            intro_html.push_str(&child_elem.html());
        } else if let Some(text) = child.value().as_text() {
            intro_html.push_str(text);
        }
    }

    let intro = converter.convert(&intro_html).trim().to_string();

    match list_tag {
        Some(list_elem) => {
            let steps = match list_elem.value().name() {
                "ol" => algorithms::render_algorithm_ol(&list_elem, converter),
                "ul" => algorithms::render_ul(&list_elem, 0, converter),
                "dl" => algorithms::render_algorithm_dl(&list_elem, converter),
                _ => unreachable!(),
            };
            Some(format!("{}\n\n{}", intro, steps))
        }
        None => {
            // No list body (e.g. single-sentence definitions inside div[data-algorithm])
            let md = markdown::element_to_markdown(div, converter);
            let trimmed = md.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    }
}

/// Extract content for an IDL type (dfn with data-dfn-type)
/// Finds the parent <pre> block and extracts IDL
fn extract_idl_content(element: &scraper::ElementRef) -> Option<String> {
    use super::idl;

    // Find the parent <pre> element
    let mut current = element.parent();
    while let Some(node) = current {
        if let Some(parent_elem) = scraper::ElementRef::wrap(node) {
            if parent_elem.value().name() == "pre" {
                let idl_text = idl::extract_idl_text(&parent_elem);
                return Some(idl_text);
            }
        }
        current = node.parent();
    }

    None
}

/// Parse a generic anchor-bearing element (tr, dt, section, li) into a ParsedSection.
/// W3C specs use these as named targets that don't fit the dfn/heading pattern.
/// Convert an element's full HTML to trimmed markdown, or `None` if it renders
/// empty. Shared by the anchor and grammar-production parsers, which both use
/// the element's own HTML as its content.
fn element_content_text(element: &scraper::ElementRef, converter: &Converter) -> Option<String> {
    let md = super::markdown::element_to_markdown_from_html(&element.html(), converter);
    let trimmed = md.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn parse_anchor_element(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Result<Option<ParsedSection>> {
    let anchor = match element.value().attr("id") {
        Some(id) => id.to_string(),
        None => return Ok(None),
    };

    let title_text = element.text().collect::<String>();
    let title_text = title_text.trim();
    let title = if title_text.is_empty() {
        None
    } else {
        let truncated = if title_text.len() > 120 {
            let boundary = title_text
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= 120)
                .last()
                .unwrap_or(0);
            format!("{}…", &title_text[..boundary])
        } else {
            title_text.to_string()
        };
        Some(truncated)
    };

    let content_text = element_content_text(element, converter);

    Ok(Some(ParsedSection {
        anchor,
        title,
        content_text,
        section_type: crate::model::SectionType::Definition,
        parent_anchor: None,
        prev_anchor: None,
        next_anchor: None,
        depth: None,
        number: None,
    }))
}

/// Parse an ecmarkup `<emu-clause>` or `<emu-annex>` element into a ParsedSection.
/// TC39 specs use these custom elements instead of standard headings.
/// The section ID is on the emu-clause, with an `<h1>` child containing the title.
pub fn parse_emu_clause_element(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Result<Option<ParsedSection>> {
    let anchor = match element.value().attr("id") {
        Some(id) => id.to_string(),
        None => return Ok(None),
    };

    // Find the direct <h1> child to extract title and depth
    let h1 = element
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .find(|c| c.value().name() == "h1");

    let (title, depth, number) = match h1 {
        Some(h1_elem) => {
            let title = extract_heading_title(&h1_elem);
            let depth = extract_secnum_depth(&h1_elem);
            let number = extract_heading_number(&h1_elem);
            (title, depth, number)
        }
        None => (None, None, None),
    };

    // Classify: if the emu-clause has a type attribute, it's an algorithm-like operation
    let section_type = if element.value().attr("type").is_some() {
        SectionType::Algorithm
    } else {
        SectionType::Heading
    };

    let content_text = extract_emu_clause_content(element, converter);

    Ok(Some(ParsedSection {
        anchor,
        title,
        content_text,
        section_type,
        parent_anchor: None,
        prev_anchor: None,
        next_anchor: None,
        depth,
        number,
    }))
}

/// Parse an ecmarkup `<emu-production>` element into a ParsedSection.
/// TC39/ecmarkup specs emit grammar productions as
/// `<emu-production name="ImportCall" id="prod-ImportCall">…</emu-production>`,
/// where the `id` is the canonical `#prod-*` anchor and `name` is the
/// nonterminal's name. The production's right-hand side becomes the content.
pub fn parse_emu_production_element(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Result<Option<ParsedSection>> {
    let anchor = match element.value().attr("id") {
        Some(id) => id.to_string(),
        None => return Ok(None), // No id, skip this production
    };

    // Prefer the nonterminal name (e.g. "ImportCall"); fall back to text.
    let title = element
        .value()
        .attr("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let text = element.text().collect::<String>().trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        });

    let content_text = element_content_text(element, converter);

    Ok(Some(ParsedSection {
        anchor,
        title,
        content_text,
        section_type: SectionType::Definition,
        parent_anchor: None,
        prev_anchor: None,
        next_anchor: None,
        depth: None,
        number: None,
    }))
}

/// Extract the depth from a secnum span inside a heading.
/// Parses `<span class="secnum">7.1.17</span>` → count parts → depth = parts + 1.
/// Returns None if no secnum is found.
fn extract_secnum_depth(heading: &scraper::ElementRef) -> Option<u8> {
    for child in heading.children() {
        if let Some(elem) = scraper::ElementRef::wrap(child) {
            let classes: Vec<_> = elem.value().classes().collect();
            if classes.contains(&"secnum") {
                let text = elem.text().collect::<String>();
                let text = text.trim();
                if text.is_empty() {
                    return None;
                }
                // Count parts: "7" → 1 part, "7.1" → 2 parts, "7.1.17" → 3 parts
                let parts = text.split('.').count();
                // Depth = parts + 1 to match h2=2 convention (top-level = depth 2)
                return Some((parts + 1).min(255) as u8);
            }
        }
    }
    None
}

/// Extract content from an ecmarkup emu-clause element.
/// Unlike standard headings where content is between siblings, emu-clause content
/// is nested inside the element as children. We skip the h1 (title) and child
/// emu-clause/emu-annex elements (sub-sections).
fn extract_emu_clause_content(
    element: &scraper::ElementRef,
    converter: &Converter,
) -> Option<String> {
    use super::{algorithms, markdown};

    let mut intro_html = String::new();
    let mut algo_steps: Option<String> = None;

    for child in element.children() {
        if let Some(child_elem) = scraper::ElementRef::wrap(child) {
            let tag = child_elem.value().name();

            // Skip title heading and sub-sections
            if tag == "h1" || tag == "emu-clause" || tag == "emu-annex" || tag == "emu-import" {
                continue;
            }

            // For emu-alg, use the dedicated algorithm renderer on its inner
            // <ol>. Source-form emu-alg (ECMA-262's committed spec.html) has no
            // built <ol> — its steps are markdown text — so fall back to the
            // source renderer, keeping algorithm bodies in the section content.
            if tag == "emu-alg" {
                if let Some(ol) = child_elem
                    .children()
                    .filter_map(scraper::ElementRef::wrap)
                    .find(|c| c.value().name() == "ol")
                {
                    algo_steps = Some(algorithms::render_algorithm_ol(&ol, converter));
                } else {
                    let steps = algorithms::render_algorithm_source(&child_elem);
                    if !steps.is_empty() {
                        algo_steps = Some(steps);
                    }
                }
                continue;
            }

            // Skip legacy ID spans (empty <span id="...">)
            if tag == "span" && child_elem.value().attr("id").is_some() {
                let text = child_elem.text().collect::<String>();
                if text.trim().is_empty() {
                    continue;
                }
            }

            intro_html.push_str(&child_elem.html());
        }
    }

    let intro = markdown::element_to_markdown_from_html(&intro_html, converter);
    let intro = intro.trim();

    match (intro.is_empty(), algo_steps) {
        (true, None) => None,
        (true, Some(steps)) => Some(steps),
        (false, None) => Some(intro.to_string()),
        (false, Some(steps)) => Some(format!("{}\n\n{}", intro, steps)),
    }
}

/// Collect all ID'd headings from HTML
#[cfg(test)]
pub fn collect_headings(html: &str) -> Result<Vec<ParsedSection>> {
    let document = Html::parse_document(html);
    let converter = crate::parse::markdown::Converter::new("https://test.example.com");
    let mut sections = Vec::new();

    // Select all headings with an id attribute (h2, h3, h4, h5, h6)
    let selector = Selector::parse("h2[id], h3[id], h4[id], h5[id], h6[id]")
        .map_err(|e| anyhow::anyhow!("Invalid selector: {:?}", e))?;

    for element in document.select(&selector) {
        if let Some(section) = parse_heading_element(&element, &converter)? {
            // Clear content for tests that expect None (tree building tests)
            // Real parsing in parse_spec will extract content
            sections.push(ParsedSection {
                content_text: None,
                ..section
            });
        }
    }

    Ok(sections)
}

/// Check if a dfn sits inside the body of an algorithm (i.e. part of its steps).
/// Such dfns belong to the algorithm's markdown content, not to a section of their own.
pub(crate) fn is_inside_algorithm_content(element: &scraper::ElementRef) -> bool {
    // The dfn may sit in a sublist; the pattern below is anchored on the outermost list.
    let mut outermost_list = None;
    let mut current = element.parent();
    while let Some(node) = current {
        if let Some(elem) = scraper::ElementRef::wrap(node) {
            match elem.value().name() {
                "ol" | "ul" => outermost_list = Some(node),
                // Bikeshed: <div class="algorithm">...<ol>...</ol></div>
                // Wattsi:   <div data-algorithm=""><p>To <dfn>foo</dfn>:</p><ol>...</ol></div>
                "div" => {
                    let is_algo_div = elem.value().classes().any(|c| c == "algorithm")
                        || elem.value().attr("data-algorithm").is_some();
                    if is_algo_div {
                        return outermost_list.is_some();
                    }
                }
                _ => {}
            }
        }
        current = node.parent();
    }

    // Wattsi sibling pattern: <p>To <dfn>foo</dfn>:</p><ol>...</ol>. Only <ol> qualifies —
    // a <ul> after a dfn is a property list ("Each navigable has: <ul>...</ul>"), whose
    // items define concepts of their own rather than algorithm steps.
    let list = match outermost_list {
        Some(node) if scraper::ElementRef::wrap(node).is_some_and(|e| e.value().name() == "ol") => {
            node
        }
        _ => return false,
    };

    let dfn_selector = match scraper::Selector::parse("dfn[id]") {
        Ok(sel) => sel,
        Err(_) => return false,
    };
    let mut prev_sibling = list.prev_sibling();
    while let Some(prev_node) = prev_sibling {
        if let Some(prev_elem) = scraper::ElementRef::wrap(prev_node) {
            if matches!(prev_elem.value().name(), "p" | "dd" | "li")
                && prev_elem.select(&dfn_selector).next().is_some()
            {
                return true;
            }
            // Stop at block elements
            if matches!(
                prev_elem.value().name(),
                "p" | "div" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                break;
            }
        }
        prev_sibling = prev_node.prev_sibling();
    }
    false
}

/// Whether an element's content is a single `<var>` and nothing else but whitespace.
fn is_var_only(element: &scraper::ElementRef) -> bool {
    let mut vars = 0;
    for child in element.children() {
        match child.value() {
            scraper::Node::Element(e) if e.name() == "var" => vars += 1,
            scraper::Node::Element(_) => return false,
            scraper::Node::Text(t) if !t.trim().is_empty() => return false,
            _ => {}
        }
    }
    vars == 1
}

pub(crate) fn is_algorithm_div(element: &scraper::ElementRef) -> bool {
    element.value().name() == "div"
        && (element.value().classes().any(|c| c == "algorithm")
            || element.value().attr("data-algorithm").is_some())
}

fn regex(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).expect("valid regex"))
}

/// Whether a dfn names an algorithm, i.e. its definition comes with a body of steps.
///
/// Beyond Bikeshed's `abstract-op` dfn type, neither generator's markup settles this: Wattsi
/// wraps nearly every paragraph of prose in `div[data-algorithm]`, and a list after a dfn is
/// as often a set of conditions, struct items or grammar components as it is steps. So the
/// dfn's own block decides, and it qualifies when it
/// - says it has steps ("The foo() method steps are to …", "… run these steps:"),
/// - declares an algorithm ("To foo …", "When the user agent is to foo …"), or
/// - introduces a following `<ol>` (or switch `<dl>`) that isn't a grammar or a field's value.
///
/// A getter written as "The foo attribute must return …" has no steps and stays a definition.
fn is_algorithm_dfn(element: &scraper::ElementRef) -> bool {
    match element.value().attr("data-dfn-type") {
        Some("enum-value") => return false,
        Some("abstract-op") => return true,
        _ => {}
    }
    let Some(block) = enclosing_block(element) else {
        return false;
    };
    let text = defining_text(&block);
    // In a paragraph defining several terms, only the dfn's own sentence speaks for it:
    // "Each stack has an associated <dfn>backup element queue</dfn> …. To <dfn>process the
    // backup element queue</dfn> …, run these steps:".
    let sentence = defines_several(&block).then(|| dfn_sentence(&block, element));
    let own = sentence.as_deref().unwrap_or(&text);
    if names_steps(own) || declares_algorithm(own, element) || has_signature(&block) {
        return true;
    }
    match following_body_list(element, &block) {
        Some("ol") => !introduces_non_steps_list(&text),
        Some("dl") => {
            static RE: OnceLock<Regex> = OnceLock::new();
            regex(
                &RE,
                r"(?i)\balgorithm\b|\bsteps?\b|\bas follows\b|\bswitch(?:ing)?\s+on\b|\bfollowing list\b",
            )
            .is_match(&text)
        }
        _ => false,
    }
}

/// The text that defines a dfn. A dfn placed directly in a div is defined by the div's
/// leading inline content, not by the paragraphs and lists that follow it.
fn defining_text(block: &scraper::ElementRef) -> String {
    if block.value().name() != "div" {
        return block_text(block);
    }
    let mut text = String::new();
    for child in block.children() {
        match child.value() {
            scraper::Node::Text(t) => text.push_str(t),
            scraper::Node::Element(e) if is_block_level(e.name()) => break,
            scraper::Node::Element(_) => {
                if let Some(e) = scraper::ElementRef::wrap(child) {
                    text.extend(e.text());
                }
            }
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_block_level(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "ol" | "ul" | "dl" | "table" | "pre" | "blockquote" | "figure" | "section"
    )
}

/// WebGPU's algorithm divs open with the dfn, then "**Arguments:**" and "**Returns:**".
fn has_signature(block: &scraper::ElementRef) -> bool {
    is_algorithm_div(block)
        && block
            .children()
            .filter_map(scraper::ElementRef::wrap)
            .filter(|c| c.value().name() == "p")
            .any(|p| {
                let text = block_text(&p);
                text.starts_with("Arguments:") || text.starts_with("Returns:")
            })
}

/// Whether a block defines more than one term (parameters aside).
fn defines_several(block: &scraper::ElementRef) -> bool {
    let Ok(dfn_selector) = scraper::Selector::parse("dfn[id]") else {
        return false;
    };
    block
        .select(&dfn_selector)
        .filter(|d| !is_var_only(d) && d.value().attr("data-dfn-type") != Some("argument"))
        .nth(1)
        .is_some()
}

/// The sentence of a block's text that contains the dfn.
fn dfn_sentence(block: &scraper::ElementRef, element: &scraper::ElementRef) -> String {
    let mut before = String::new();
    for node in block.descendants() {
        if node.id() == element.id() {
            break;
        }
        if let Some(t) = node.value().as_text() {
            before.push_str(t);
        }
    }
    let before = before.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = block_text(block);
    let mut at = before.len().min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let start = text[..at].rfind(". ").map_or(0, |i| i + 2);
    let end = text[at..].find(". ").map_or(text.len(), |i| at + i + 1);
    text[start..end].to_string()
}

/// The innermost block that holds a dfn's defining sentence.
/// None for a dfn in a `<pre>` (IDL, grammar), which has no defining sentence.
pub(crate) fn enclosing_block<'a>(
    element: &scraper::ElementRef<'a>,
) -> Option<scraper::ElementRef<'a>> {
    element
        .ancestors()
        .filter_map(scraper::ElementRef::wrap)
        .find(|e| matches!(e.value().name(), "p" | "dd" | "dt" | "li" | "div" | "pre"))
        .filter(|e| e.value().name() != "pre")
}

fn block_text(block: &scraper::ElementRef) -> String {
    block
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// "The foo getter steps are to …", "… steps are:", "… consists of the following steps",
/// "… must return the result of running the document open steps".
fn names_steps(text: &str) -> bool {
    static ARE_TO: OnceLock<Regex> = OnceLock::new();
    static OTHER: OnceLock<Regex> = OnceLock::new();
    // "… means those steps are to be run" describes steps rather than giving them.
    regex(&ARE_TO, r"(?i)\bsteps?\b[^.]*?\b(?:are|is)\s+to\b\W*(\w*)")
        .captures_iter(text)
        .any(|caps| !caps[1].eq_ignore_ascii_case("be"))
        || regex(
            &OTHER,
            r"(?i)\bsteps?\b[^.]*?\b(?:are|is)\s*:|\bsteps\s*:|\b(?:these|the following)\s+(?:sub)?steps\b|\bmust\b[^.]*\brun(?:ning)?\s+(?:the|these)\b[^.]*\bsteps\b|\bmeans\s+running\b",
        )
        .is_match(text)
}

/// "To <dfn>foo</dfn> …" (Infra's algorithm declaration), the older "When the user agent is
/// to <dfn>foo</dfn> …" and "When the steps below require the UA to <dfn>foo</dfn> …", and
/// "<dfn>Foo</dfn>, given …, is to return …" / "The <dfn>foo</dfn> algorithm, given …, returns …".
fn declares_algorithm(text: &str, element: &scraper::ElementRef) -> bool {
    static WHEN: OnceLock<Regex> = OnceLock::new();
    static GIVEN: OnceLock<Regex> = OnceLock::new();
    let name = block_text(element);
    if name.is_empty() {
        return false;
    }
    // Each form has to declare the dfn itself, not merely mention it: "When the parser is
    // to operate on a stream that has <dfn>a known definite encoding</dfn>", "To ensure
    // reactions are triggered, we introduce <dfn>[CEReactions]</dfn>", "To run steps
    // <dfn>in parallel</dfn> means …".
    let when = regex(
        &WHEN,
        r"(?i)^(?:when|if)\b[^,.]*?\b(?:(?:is|are)\s+(?:required\s+|asked\s+)?|requires?\s+(?:the\s+)?(?:user agent|UA)\s+)to\s|^when asked to\s",
    )
    .find(text)
    .is_some_and(|m| {
        // The verb phrase after "is to", give or take a verb and an article.
        let rest = &text[m.end()..];
        rest.find(&name)
            .is_some_and(|at| rest[..at].split_whitespace().count() <= 3)
    });
    let given = regex(
        &GIVEN,
        r"(?i)^[^.]*?\bgiven\b[^.]*?,\s*(?:(?:is|are)\s+to|returns?)\b",
    )
    .is_match(text)
        && text
            .find(&name)
            .is_some_and(|at| text[..at].split_whitespace().count() <= 1);
    // The "To" sentence may follow another in the same paragraph: "A script element has
    // steps to run when the result is ready …. To <dfn>mark as ready</dfn> …:".
    when || given
        || text.split(". ").enumerate().any(|(i, sentence)| {
            let Some(rest) = sentence.strip_prefix("To ") else {
                return false;
            };
            let clause = rest.split([',', ':']).next().unwrap_or(rest);
            // Past the first sentence, "To disambiguate from a valid URL string it can also
            // be referred to as a URL record" must not declare "URL".
            let near = i == 0
                || clause
                    .find(&name)
                    .is_some_and(|at| clause[..at].split_whitespace().count() <= 3);
            near && clause.contains(&name)
                && !clause.contains(" we ")
                && !clause.contains(" means ")
        })
}

/// A paragraph that gives more steps for a section defined elsewhere, pointing back to it
/// instead of carrying a dfn: "The <a href=#dom-history-scroll-restoration>scrollRestoration</a>
/// setter steps are:". Returns the anchor it links back to.
pub(crate) fn continued_anchor<'a>(block: &scraper::ElementRef<'a>) -> Option<&'a str> {
    static STEPS_NEXT: OnceLock<Regex> = OnceLock::new();
    if block.value().name() != "p" {
        return None;
    }
    let text = block_text(block);
    if !names_steps(&text) {
        return None;
    }
    // A step saying "If <a>cond</a> is true, run these steps:" continues nothing.
    let in_list = block
        .ancestors()
        .filter_map(scraper::ElementRef::wrap)
        .any(|a| matches!(a.value().name(), "li" | "dd" | "dt"));
    let dfn_selector = scraper::Selector::parse("dfn[id]").ok()?;
    if in_list || block.select(&dfn_selector).next().is_some() {
        return None;
    }
    let link_selector = scraper::Selector::parse("a[href^='#']").ok()?;
    let link = block.select(&link_selector).next()?;
    let link_text = block_text(&link);
    let at = text.find(&link_text)?;
    if text[..at].split_whitespace().count() > 3
        || !regex(&STEPS_NEXT, r"^\s*(?:\S+\s+){0,2}steps\b")
            .is_match(&text[at + link_text.len()..])
    {
        return None;
    }
    Some(link.value().attr("href")?.trim_start_matches('#'))
}

/// The list that serves as the dfn's body: the next list sibling of its block, or, when the
/// dfn opens an algorithm div, the div's top-level lists after the block, of which an `<ol>`
/// wins (Bikeshed may put a `<ul>` of inputs before the steps).
fn following_body_list(
    element: &scraper::ElementRef,
    block: &scraper::ElementRef,
) -> Option<&'static str> {
    let list_name = |e: &scraper::ElementRef| match e.value().name() {
        "ol" => Some("ol"),
        "ul" => Some("ul"),
        "dl" => Some("dl"),
        _ => None,
    };
    if !is_algorithm_div(block) {
        for sibling in block.next_siblings().filter_map(scraper::ElementRef::wrap) {
            if let Some(name) = list_name(&sibling) {
                return Some(name);
            }
            if matches!(
                sibling.value().name(),
                "p" | "div" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                break;
            }
        }
    }
    // The dfn may sit in a paragraph of the div, or directly in the div.
    let (div, lists): (_, Vec<_>) = if is_algorithm_div(block) {
        (*block, block.children().collect())
    } else {
        let div = scraper::ElementRef::wrap(block.parent()?).filter(is_algorithm_div)?;
        (div, block.next_siblings().collect())
    };
    let dfn_selector = scraper::Selector::parse("dfn[id]").ok()?;
    let owner = div.select(&dfn_selector).find(|d| {
        d.ancestors()
            .take_while(|a| a.id() != div.id())
            .filter_map(scraper::ElementRef::wrap)
            .all(|a| !matches!(a.value().name(), "ol" | "ul" | "dl"))
    })?;
    if owner.id() != element.id() {
        return None;
    }
    let names: Vec<_> = lists
        .into_iter()
        .filter_map(scraper::ElementRef::wrap)
        .filter_map(|e| list_name(&e))
        .collect();
    ["ol", "dl", "ul"].into_iter().find(|n| names.contains(n))
}

/// Grammars ("… if it consists of the following components"), enumerations ("… are the
/// following") and field declarations ("Each Window object has a … list") also precede an
/// `<ol>`, which then isn't a body of steps.
fn introduces_non_steps_list(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(
        &RE,
        r"(?i)\bconsists?\s+of\b|\bmatches the following\b|\bare the following\b|\bthe following (?:components|pattern)\b|^(?:each|an?|the)\b[^.]*\bhas\s+(?:an?\s+)?(?:associated\s+)?[^.]*?(?:\.|\binitially\b)",
    )
    .is_match(text)
}

/// Check if a dfn element is an IDL type definition
fn is_idl_type(element: &scraper::ElementRef) -> bool {
    if let Some(dfn_type) = element.value().attr("data-dfn-type") {
        matches!(
            dfn_type,
            "interface" | "dictionary" | "enum" | "callback" | "callback interface" | "typedef"
        )
    } else {
        false
    }
}

/// Collect all ID'd IDL type definitions from HTML
#[cfg(test)]
pub fn collect_idl(html: &str) -> Result<Vec<ParsedSection>> {
    let document = Html::parse_document(html);
    let mut sections = Vec::new();

    // Select all dfn elements with an id and data-dfn-type attribute
    let selector = Selector::parse("dfn[id][data-dfn-type]")
        .map_err(|e| anyhow::anyhow!("Invalid selector: {:?}", e))?;

    for element in document.select(&selector) {
        // Only collect IDL type definitions (interface, dictionary, enum, etc.)
        if !is_idl_type(&element) {
            continue;
        }

        let anchor = element
            .value()
            .attr("id")
            .ok_or_else(|| anyhow::anyhow!("IDL type missing id"))?
            .to_string();

        // Extract text content (including nested elements like <code>)
        let title = element.text().collect::<String>().trim().to_string();
        let title = if title.is_empty() { None } else { Some(title) };

        sections.push(ParsedSection {
            anchor,
            title,
            content_text: None, // Will be extracted in a later pass
            section_type: SectionType::Idl,
            parent_anchor: None, // Will be computed in tree building
            prev_anchor: None,   // Will be computed in tree building
            next_anchor: None,   // Will be computed in tree building
            depth: None,         // IDL types don't have depth
            number: None,
        });
    }

    Ok(sections)
}

/// Collect all ID'd algorithms from HTML (dfn elements inside div.algorithm)
#[cfg(test)]
pub fn collect_algorithms(html: &str) -> Result<Vec<ParsedSection>> {
    let document = Html::parse_document(html);
    let mut sections = Vec::new();

    // Select all definitions with an id attribute inside algorithm divs
    let selector = Selector::parse("div.algorithm dfn[id]")
        .map_err(|e| anyhow::anyhow!("Invalid selector: {:?}", e))?;

    for element in document.select(&selector) {
        let anchor = element
            .value()
            .attr("id")
            .ok_or_else(|| anyhow::anyhow!("Algorithm missing id"))?
            .to_string();

        // Extract text content (including nested elements like <code>)
        let title = element.text().collect::<String>().trim().to_string();
        let title = if title.is_empty() { None } else { Some(title) };

        sections.push(ParsedSection {
            anchor,
            title,
            content_text: None, // Will be extracted in a later pass
            section_type: SectionType::Algorithm,
            parent_anchor: None, // Will be computed in tree building
            prev_anchor: None,   // Will be computed in tree building
            next_anchor: None,   // Will be computed in tree building
            depth: None,         // Algorithms don't have depth
            number: None,
        });
    }

    Ok(sections)
}

/// Collect all ID'd definitions from HTML (dfn elements NOT inside div.algorithm and NOT IDL types)
#[cfg(test)]
pub fn collect_definitions(html: &str) -> Result<Vec<ParsedSection>> {
    let document = Html::parse_document(html);
    let mut sections = Vec::new();

    // Select all definitions with an id attribute
    let selector =
        Selector::parse("dfn[id]").map_err(|e| anyhow::anyhow!("Invalid selector: {:?}", e))?;

    for element in document.select(&selector) {
        // Skip definitions that are inside algorithm divs (those are algorithms)
        if is_algorithm_dfn(&element) {
            continue;
        }

        // Skip IDL type definitions (those are IDL)
        if is_idl_type(&element) {
            continue;
        }

        let anchor = element
            .value()
            .attr("id")
            .ok_or_else(|| anyhow::anyhow!("Definition missing id"))?
            .to_string();

        // Extract text content (including nested elements like <code>)
        let title = element.text().collect::<String>().trim().to_string();
        let title = if title.is_empty() { None } else { Some(title) };

        sections.push(ParsedSection {
            anchor,
            title,
            content_text: None, // Will be extracted in a later pass
            section_type: SectionType::Definition,
            parent_anchor: None, // Will be computed in tree building
            prev_anchor: None,   // Will be computed in tree building
            next_anchor: None,   // Will be computed in tree building
            depth: None,         // Definitions don't have depth
            number: None,
        });
    }

    Ok(sections)
}

/// Build parent/child/sibling relationships for a flat list of sections
pub fn build_section_tree(mut sections: Vec<ParsedSection>) -> Vec<ParsedSection> {
    // First pass: compute parent relationships
    for i in 0..sections.len() {
        if let Some(current_depth) = sections[i].depth {
            // This is a heading - find parent heading with depth < current
            for j in (0..i).rev() {
                if let Some(parent_depth) = sections[j].depth {
                    if parent_depth < current_depth {
                        sections[i].parent_anchor = Some(sections[j].anchor.clone());
                        break;
                    }
                }
            }
        } else {
            // This is a non-heading (definition, algorithm, IDL)
            // Parent is the most recent heading (any heading)
            for j in (0..i).rev() {
                if sections[j].depth.is_some() {
                    sections[i].parent_anchor = Some(sections[j].anchor.clone());
                    break;
                }
            }
        }
    }

    // Second pass: compute prev/next sibling relationships
    for i in 0..sections.len() {
        let current_depth = sections[i].depth;
        let current_parent = sections[i].parent_anchor.clone();

        // Look backwards for prev sibling (same depth, same parent)
        for j in (0..i).rev() {
            if sections[j].depth == current_depth && sections[j].parent_anchor == current_parent {
                sections[i].prev_anchor = Some(sections[j].anchor.clone());
                break;
            }
        }

        // Look forwards for next sibling (same depth, same parent)
        for j in (i + 1)..sections.len() {
            if sections[j].depth == current_depth && sections[j].parent_anchor == current_parent {
                sections[i].next_anchor = Some(sections[j].anchor.clone());
                break;
            }
        }
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bikeshed_heading_parsing() {
        let html = include_str!("../../tests/fixtures/headings/bikeshed_heading.html");
        let sections = collect_headings(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "trees");
        assert_eq!(section.title, Some("Trees".to_string()));
        assert_eq!(section.section_type, SectionType::Heading);
        assert_eq!(section.depth, Some(3));
    }

    #[test]
    fn test_wattsi_heading_parsing() {
        let html = include_str!("../../tests/fixtures/headings/wattsi_heading.html");
        let sections = collect_headings(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "abstract");
        assert_eq!(
            section.title,
            Some("Where does this specification fit?".to_string())
        );
        assert_eq!(section.section_type, SectionType::Heading);
        assert_eq!(section.depth, Some(3));
    }

    #[test]
    fn test_multiple_heading_levels() {
        let html = r#"
            <h2 id="section-1">Section 1</h2>
            <h3 id="section-1-1">Section 1.1</h3>
            <h4 id="section-1-1-1">Section 1.1.1</h4>
            <h2 id="section-2">Section 2</h2>
        "#;

        let sections = collect_headings(html).unwrap();
        assert_eq!(sections.len(), 4);

        assert_eq!(sections[0].anchor, "section-1");
        assert_eq!(sections[0].depth, Some(2));

        assert_eq!(sections[1].anchor, "section-1-1");
        assert_eq!(sections[1].depth, Some(3));

        assert_eq!(sections[2].anchor, "section-1-1-1");
        assert_eq!(sections[2].depth, Some(4));

        assert_eq!(sections[3].anchor, "section-2");
        assert_eq!(sections[3].depth, Some(2));
    }

    #[test]
    fn test_heading_without_id_ignored() {
        let html = r#"
            <h2 id="has-id">With ID</h2>
            <h2>Without ID</h2>
        "#;

        let sections = collect_headings(html).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].anchor, "has-id");
    }

    #[test]
    fn test_build_section_tree_simple_nesting() {
        let html = r#"
            <h2 id="s1">Section 1</h2>
            <h3 id="s1-1">Section 1.1</h3>
            <h3 id="s1-2">Section 1.2</h3>
            <h4 id="s1-2-1">Section 1.2.1</h4>
            <h2 id="s2">Section 2</h2>
        "#;

        let sections = collect_headings(html).unwrap();
        let tree = build_section_tree(sections);

        // s1: no parent, no prev, next=s2
        assert_eq!(tree[0].parent_anchor, None);
        assert_eq!(tree[0].prev_anchor, None);
        assert_eq!(tree[0].next_anchor, Some("s2".to_string()));

        // s1-1: parent=s1, no prev, next=s1-2
        assert_eq!(tree[1].parent_anchor, Some("s1".to_string()));
        assert_eq!(tree[1].prev_anchor, None);
        assert_eq!(tree[1].next_anchor, Some("s1-2".to_string()));

        // s1-2: parent=s1, prev=s1-1, no next
        assert_eq!(tree[2].parent_anchor, Some("s1".to_string()));
        assert_eq!(tree[2].prev_anchor, Some("s1-1".to_string()));
        assert_eq!(tree[2].next_anchor, None);

        // s1-2-1: parent=s1-2, no prev, no next
        assert_eq!(tree[3].parent_anchor, Some("s1-2".to_string()));
        assert_eq!(tree[3].prev_anchor, None);
        assert_eq!(tree[3].next_anchor, None);

        // s2: no parent, prev=s1, no next
        assert_eq!(tree[4].parent_anchor, None);
        assert_eq!(tree[4].prev_anchor, Some("s1".to_string()));
        assert_eq!(tree[4].next_anchor, None);
    }

    #[test]
    fn test_build_section_tree_flat_structure() {
        let html = r#"
            <h2 id="a">A</h2>
            <h2 id="b">B</h2>
            <h2 id="c">C</h2>
        "#;

        let sections = collect_headings(html).unwrap();
        let tree = build_section_tree(sections);

        // a: no parent, no prev, next=b
        assert_eq!(tree[0].parent_anchor, None);
        assert_eq!(tree[0].prev_anchor, None);
        assert_eq!(tree[0].next_anchor, Some("b".to_string()));

        // b: no parent, prev=a, next=c
        assert_eq!(tree[1].parent_anchor, None);
        assert_eq!(tree[1].prev_anchor, Some("a".to_string()));
        assert_eq!(tree[1].next_anchor, Some("c".to_string()));

        // c: no parent, prev=b, no next
        assert_eq!(tree[2].parent_anchor, None);
        assert_eq!(tree[2].prev_anchor, Some("b".to_string()));
        assert_eq!(tree[2].next_anchor, None);
    }

    #[test]
    fn test_build_section_tree_single_heading() {
        let html = r#"<h2 id="only">Only Section</h2>"#;

        let sections = collect_headings(html).unwrap();
        let tree = build_section_tree(sections);

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].parent_anchor, None);
        assert_eq!(tree[0].prev_anchor, None);
        assert_eq!(tree[0].next_anchor, None);
    }

    #[test]
    fn test_build_section_tree_skip_levels() {
        // Test when heading levels are skipped (h2 -> h4, skipping h3)
        let html = r#"
            <h2 id="top">Top</h2>
            <h4 id="nested">Nested (skipped h3)</h4>
            <h2 id="next">Next Top</h2>
        "#;

        let sections = collect_headings(html).unwrap();
        let tree = build_section_tree(sections);

        // nested: parent should still be 'top' (nearest lower depth)
        assert_eq!(tree[1].parent_anchor, Some("top".to_string()));
        assert_eq!(tree[1].prev_anchor, None); // no siblings at depth 4
        assert_eq!(tree[1].next_anchor, None);
    }

    #[test]
    fn test_bikeshed_definition_parsing() {
        let html = include_str!("../../tests/fixtures/definitions/bikeshed_definition.html");
        let sections = collect_definitions(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "concept-tree");
        assert_eq!(section.title, Some("tree".to_string()));
        assert_eq!(section.section_type, SectionType::Definition);
        assert_eq!(section.depth, None);
    }

    #[test]
    fn test_wattsi_definition_parsing() {
        let html = include_str!("../../tests/fixtures/definitions/wattsi_definition.html");
        let sections = collect_definitions(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "in-parallel");
        assert_eq!(section.title, Some("in parallel".to_string()));
        assert_eq!(section.section_type, SectionType::Definition);
        assert_eq!(section.depth, None);
    }

    #[test]
    fn test_definition_with_code() {
        let html = include_str!("../../tests/fixtures/definitions/definition_with_code.html");
        let sections = collect_definitions(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "x-that");
        assert_eq!(section.title, Some("createElement".to_string()));
        assert_eq!(section.section_type, SectionType::Definition);
    }

    #[test]
    fn test_definition_without_id_ignored() {
        let html = r#"
            <dfn id="has-id">With ID</dfn>
            <dfn>Without ID</dfn>
        "#;

        let sections = collect_definitions(html).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].anchor, "has-id");
    }

    #[test]
    fn test_multiple_definitions() {
        let html = r#"
            <p>A <dfn id="def-1">first term</dfn> and a <dfn id="def-2">second term</dfn>.</p>
            <p>Also a <dfn id="def-3">third term</dfn>.</p>
        "#;

        let sections = collect_definitions(html).unwrap();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].anchor, "def-1");
        assert_eq!(sections[1].anchor, "def-2");
        assert_eq!(sections[2].anchor, "def-3");
    }

    fn first_emu_production(html: &str) -> Option<ParsedSection> {
        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-production").unwrap();
        let element = document.select(&selector).next().unwrap();
        parse_emu_production_element(&element, &converter).unwrap()
    }

    #[test]
    fn test_emu_production_parsed() {
        // ecmarkup grammar production: id is the #prod-* anchor, name is the
        // nonterminal, and the right-hand side becomes the content.
        let html = r#"
            <emu-production name="ImportCall" params="Yield, Await" id="prod-ImportCall">
                <emu-nt>ImportCall</emu-nt>
                <emu-geq>:</emu-geq>
                <emu-t>import</emu-t>
                <emu-t>(</emu-t>
                <emu-nt>AssignmentExpression</emu-nt>
                <emu-t>)</emu-t>
            </emu-production>
        "#;

        let section = first_emu_production(html).expect("production should parse");
        assert_eq!(section.anchor, "prod-ImportCall");
        assert_eq!(section.title, Some("ImportCall".to_string()));
        assert_eq!(section.section_type, SectionType::Definition);
        assert_eq!(section.depth, None);
        let content = section
            .content_text
            .expect("production should have content");
        assert!(
            content.contains("AssignmentExpression"),
            "content should render the RHS, got: {content}"
        );
    }

    #[test]
    fn test_emu_production_without_id_skipped() {
        let html = r#"<emu-production name="Foo"><emu-nt>Foo</emu-nt></emu-production>"#;
        assert!(
            first_emu_production(html).is_none(),
            "a production without an id has no queryable anchor and must be skipped"
        );
    }

    #[test]
    fn test_emu_production_title_falls_back_to_text() {
        // No name attribute -> title is derived from the element text.
        let html = r#"<emu-production id="prod-Bar"><emu-nt>Bar</emu-nt><emu-geq>:</emu-geq><emu-t>baz</emu-t></emu-production>"#;
        let section = first_emu_production(html).expect("production should parse");
        assert_eq!(section.anchor, "prod-Bar");
        let title = section.title.expect("title should fall back to text");
        assert!(
            title.contains("Bar"),
            "title should include text, got: {title}"
        );
    }

    #[test]
    fn test_bikeshed_algorithm_parsing() {
        let html = include_str!("../../tests/fixtures/algorithms/bikeshed_algorithm.html");
        let sections = collect_algorithms(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "concept-ordered-set-parser");
        assert_eq!(section.title, Some("ordered set parser".to_string()));
        assert_eq!(section.section_type, SectionType::Algorithm);
        assert_eq!(section.depth, None);
    }

    #[test]
    fn test_algorithm_vs_definition_distinction() {
        let html =
            include_str!("../../tests/fixtures/algorithms/mixed_definitions_algorithms.html");

        // Collect algorithms (dfn inside div.algorithm)
        let algorithms = collect_algorithms(html).unwrap();
        assert_eq!(algorithms.len(), 1);
        assert_eq!(algorithms[0].anchor, "algorithm-def");
        assert_eq!(algorithms[0].section_type, SectionType::Algorithm);

        // Collect definitions (dfn NOT inside div.algorithm)
        let definitions = collect_definitions(html).unwrap();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].anchor, "standalone-def");
        assert_eq!(definitions[0].section_type, SectionType::Definition);
        assert_eq!(definitions[1].anchor, "another-standalone");
        assert_eq!(definitions[1].section_type, SectionType::Definition);

        // No overlap: the dfn inside algorithm div should not appear in definitions
        let def_anchors: Vec<_> = definitions.iter().map(|d| &d.anchor).collect();
        assert!(!def_anchors.contains(&&"algorithm-def".to_string()));
    }

    #[test]
    fn test_algorithm_without_dfn() {
        // Some algorithms might not have a dfn, just the algorithm div
        let html = r#"
            <div class="algorithm" data-algorithm="no dfn">
                <p>This algorithm has no dfn element.</p>
                <ol><li>Step 1</li></ol>
            </div>
        "#;

        let sections = collect_algorithms(html).unwrap();
        assert_eq!(sections.len(), 0); // No dfn[id], so nothing to index
    }

    #[test]
    fn test_idl_interface_parsing() {
        let html = include_str!("../../tests/fixtures/idl/interface.html");
        let sections = collect_idl(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "event");
        assert_eq!(section.title, Some("Event".to_string()));
        assert_eq!(section.section_type, SectionType::Idl);
        assert_eq!(section.depth, None);
    }

    #[test]
    fn test_idl_dictionary_parsing() {
        let html = include_str!("../../tests/fixtures/idl/dictionary.html");
        let sections = collect_idl(html).unwrap();

        assert_eq!(sections.len(), 1);
        let section = &sections[0];

        assert_eq!(section.anchor, "eventinit");
        assert_eq!(section.title, Some("EventInit".to_string()));
        assert_eq!(section.section_type, SectionType::Idl);
        assert_eq!(section.depth, None);
    }

    #[test]
    fn test_idl_vs_definition_distinction() {
        let html = include_str!("../../tests/fixtures/idl/mixed_idl_definitions.html");

        // Collect IDL types (dfn with data-dfn-type="interface", "dictionary", etc.)
        let idl = collect_idl(html).unwrap();
        assert_eq!(idl.len(), 2);
        assert_eq!(idl[0].anchor, "myinterface");
        assert_eq!(idl[0].section_type, SectionType::Idl);
        assert_eq!(idl[1].anchor, "mydict");
        assert_eq!(idl[1].section_type, SectionType::Idl);

        // Collect definitions (dfn NOT IDL types and NOT in algorithm divs)
        let definitions = collect_definitions(html).unwrap();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].anchor, "regular-term");
        assert_eq!(definitions[0].section_type, SectionType::Definition);
        assert_eq!(definitions[1].anchor, "another-term");
        assert_eq!(definitions[1].section_type, SectionType::Definition);

        // No overlap: IDL types should not appear in definitions
        let def_anchors: Vec<_> = definitions.iter().map(|d| &d.anchor).collect();
        assert!(!def_anchors.contains(&&"myinterface".to_string()));
        assert!(!def_anchors.contains(&&"mydict".to_string()));
    }

    #[test]
    fn test_idl_without_data_dfn_type_ignored() {
        let html = r#"
            <pre class="idl">
                <dfn id="has-type" data-dfn-type="interface">WithType</dfn>
                <dfn id="no-type">WithoutType</dfn>
            </pre>
        "#;

        let sections = collect_idl(html).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].anchor, "has-type");
    }

    #[test]
    fn test_wattsi_algorithm_pattern() {
        // Test Wattsi-style algorithm: <p>To <dfn>foo</dfn>:</p><ol>...</ol>
        // (as opposed to Bikeshed's <div class="algorithm"><p>To <dfn>foo</dfn>:</p><ol>...</ol></div>)
        let html = include_str!("../../tests/fixtures/algorithms/wattsi_navigate.html");
        let converter = crate::parse::markdown::Converter::new("https://html.spec.whatwg.org");

        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut algorithms = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                algorithms.push(section);
            }
        }

        assert_eq!(algorithms.len(), 1, "Should detect one algorithm");
        let algo = &algorithms[0];

        assert_eq!(algo.anchor, "navigate");
        assert_eq!(algo.title, Some("navigate".to_string()));
        assert_eq!(
            algo.section_type,
            SectionType::Algorithm,
            "Should be classified as Algorithm, not Definition"
        );

        // Check that content includes both intro and steps (now markdown)
        let content = algo.content_text.as_ref().unwrap();
        assert!(content.contains("navigate"), "Should include intro text");
        assert!(content.contains("1. "), "Should include first step");
        assert!(content.contains("2. "), "Should include second step");
        // Check for nested step (step 4 has sub-steps in the fixture)
        assert!(
            content.contains("    1. "),
            "Should include nested step with indentation"
        );
    }

    #[test]
    fn test_wattsi_dl_switch_algorithm_pattern() {
        // Test Wattsi-style algorithm with <dl class="switch"> body instead of <ol>:
        // <p>To <dfn>foo</dfn>, run the first matching steps:</p><dl class="switch">...</dl>
        // Regression test: this pattern was previously misclassified as a plain definition,
        // returning only the intro sentence.
        let html = include_str!("../../tests/fixtures/algorithms/wattsi_dl_switch.html");
        let converter = crate::parse::markdown::Converter::new("https://html.spec.whatwg.org");

        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut algorithms = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                algorithms.push(section);
            }
        }

        assert_eq!(algorithms.len(), 1, "Should detect one algorithm");
        let algo = &algorithms[0];

        assert_eq!(algo.anchor, "get-the-focusable-area");
        assert_eq!(
            algo.section_type,
            SectionType::Algorithm,
            "dl.switch pattern should be classified as Algorithm, not Definition"
        );

        let content = algo.content_text.as_ref().unwrap();
        assert!(
            content.contains("get the focusable area"),
            "Should include intro text"
        );
        assert!(
            content.contains("area"),
            "Should include area element case from dl"
        );
        assert!(
            content.contains("shadow host"),
            "Should include shadow host case from dl"
        );
        assert!(
            content.contains("Return null"),
            "Should include the Otherwise/Return null case"
        );
        // The shadow host dd has an <ol> with numbered steps
        assert!(
            content.contains("1."),
            "Should include numbered steps from ol inside dd"
        );
    }

    #[test]
    fn test_dfn_inside_algorithm_content_skipped() {
        // Dfns that appear inside algorithm <ol> content should NOT be collected as separate sections
        // They're part of the algorithm's markdown content
        let html = r#"
            <h2 id="algorithms">Algorithms</h2>
            <p>To <dfn id="do-something">do something</dfn> with <var>input</var>:</p>
            <ol>
                <li><p>Let <var>result</var> be the result of calling <dfn id="helper">helper</dfn>.</p></li>
                <li><p>Return <var>result</var>.</p></li>
            </ol>
            <p>The <dfn id="outside-def">outside definition</dfn> is separate.</p>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://test.example.com");
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        // Should only collect "do-something" (the algorithm) and "outside-def"
        // "helper" inside the <ol> should be skipped
        assert_eq!(
            sections.len(),
            2,
            "Should collect 2 sections (algorithm + outside def), not the helper inside <ol>"
        );

        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();
        assert!(
            anchors.contains(&"do-something"),
            "Should include the algorithm-defining dfn"
        );
        assert!(
            anchors.contains(&"outside-def"),
            "Should include the outside definition"
        );
        assert!(
            !anchors.contains(&"helper"),
            "Should NOT include dfn inside algorithm <ol>"
        );
    }

    #[test]
    fn test_dfn_inside_bikeshed_algorithm_content_skipped() {
        // Same test but for Bikeshed div.algorithm pattern
        let html = r#"
            <h2 id="algorithms">Algorithms</h2>
            <div class="algorithm">
                <p>To <dfn id="process">process</dfn> the <var>data</var>:</p>
                <ol>
                    <li><p>Let <var>x</var> be a new <dfn id="internal-thing">internal thing</dfn>.</p></li>
                    <li><p>Return <var>x</var>.</p></li>
                </ol>
            </div>
            <p>A <dfn id="external-term">external term</dfn> here.</p>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://test.example.com");
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        // Should only collect "process" (the algorithm) and "external-term"
        assert_eq!(
            sections.len(),
            2,
            "Should collect 2 sections, not the internal-thing inside <ol>"
        );

        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();
        assert!(anchors.contains(&"process"));
        assert!(anchors.contains(&"external-term"));
        assert!(
            !anchors.contains(&"internal-thing"),
            "Should NOT include dfn inside algorithm <ol>"
        );
    }

    #[test]
    fn test_parameter_dfns_skipped() {
        // Wattsi wraps parameter names in <var>. Those dfns are part of the parent
        // algorithm's signature, not standalone sections, so they must not be collected.
        let html = r#"
            <h2 id="algorithms">Algorithms</h2>
            <p>To <dfn id="navigate">navigate</dfn> with <dfn data-dfn-for="navigate" id="param1"><var>url</var></dfn>
            and <dfn id="param2"><var>options</var></dfn>:</p>
            <ol>
                <li><p>Do something.</p></li>
            </ol>
            <p>A standalone <dfn id="regular-def">definition</dfn>.</p>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://test.example.com");
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        // Should only collect "navigate" (algorithm) and "regular-def" (standalone definition);
        // "param1" and "param2" are <var>-wrapped parameters
        assert_eq!(
            sections.len(),
            2,
            "Should collect 2 sections (algorithm + regular def)"
        );

        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();
        assert!(
            anchors.contains(&"navigate"),
            "Should include the algorithm"
        );
        assert!(
            anchors.contains(&"regular-def"),
            "Should include standalone definition"
        );
        assert!(
            !anchors.contains(&"param1"),
            "Should NOT include parameter dfn containing <var>"
        );
        assert!(
            !anchors.contains(&"param2"),
            "Should NOT include parameter dfn containing <var>"
        );
    }

    #[test]
    fn test_property_dfns_with_dfn_for_and_dfn_type_kept() {
        // dfns with data-dfn-for AND data-dfn-type="dfn" are property definitions,
        // not parameters. They should be indexed.
        // Real example from DOM spec: <dfn data-dfn-for="tree" data-dfn-type="dfn" id="concept-tree-parent">parent</dfn>
        let html = r#"
            <h2 id="trees">Trees</h2>
            <p>An object that <dfn class="dfn-paneled" data-dfn-type="dfn" data-export id="concept-tree">participates</dfn>
            in a tree has a <dfn class="dfn-paneled" data-dfn-for="tree" data-dfn-type="dfn" data-export id="concept-tree-parent">parent</dfn>,
            which is either null or an object, and has
            <dfn class="dfn-paneled" data-dfn-for="tree" data-dfn-type="dfn" data-export id="concept-tree-child">children</dfn>,
            which is an ordered set of objects.</p>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://test.example.com");
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();
        assert!(
            anchors.contains(&"concept-tree"),
            "Should include dfn without data-dfn-for"
        );
        assert!(
            anchors.contains(&"concept-tree-parent"),
            "Should include property dfn with data-dfn-for + data-dfn-type"
        );
        assert!(
            anchors.contains(&"concept-tree-child"),
            "Should include property dfn with data-dfn-for + data-dfn-type"
        );
    }

    fn collect_dfn_sections(html: &str, base_url: &str) -> Vec<ParsedSection> {
        let converter = crate::parse::markdown::Converter::new(base_url);
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();
        document
            .select(&selector)
            .filter_map(|element| parse_dfn_element(&element, &converter).unwrap())
            .collect()
    }

    #[test]
    fn test_wattsi_property_list_dfns_kept() {
        // "Each navigable has: <ul>...</ul>" — Wattsi's pattern for the properties of a
        // concept. Every <li> defines a queryable term, and the terms carry data-dfn-for
        // without data-dfn-type. Sublists of a sibling-pattern <ol> stay excluded.
        let sections = collect_dfn_sections(
            include_str!("../../tests/fixtures/definitions/wattsi_property_list.html"),
            "https://html.spec.whatwg.org",
        );
        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();

        for kept in [
            "navigable",
            "nav-id",
            "nav-parent",
            "is-closing",
            "allowed-to-perform-a-navigation-or-history-update",
            "nav-document",
            "close-a-top-level-traversable",
        ] {
            assert!(anchors.contains(&kept), "Should include {kept}");
        }

        assert!(
            !anchors.contains(&"close-traversable"),
            "Should NOT include <var>-wrapped parameter"
        );
        assert!(
            !anchors.contains(&"closing-navigable"),
            "Should NOT include dfn inside algorithm <ol> steps"
        );
        assert!(
            !anchors.contains(&"closing-timestamp"),
            "Should NOT include dfn inside a <ul> nested in algorithm <ol> steps"
        );
        assert_eq!(anchors.len(), 7, "Unexpected sections: {anchors:?}");

        let by_anchor = |a: &str| sections.iter().find(|s| s.anchor == a).unwrap();

        let allowed = by_anchor("allowed-to-perform-a-navigation-or-history-update");
        assert_eq!(allowed.section_type, SectionType::Definition);
        assert!(
            allowed
                .content_text
                .as_ref()
                .unwrap()
                .contains("implementation-defined"),
            "Property definition should carry its own prose"
        );

        // data-dfn-for without data-dfn-type is an exported concept, not a parameter
        assert_eq!(
            by_anchor("nav-parent").section_type,
            SectionType::Definition
        );
        assert_eq!(
            by_anchor("nav-document").section_type,
            SectionType::Definition
        );
        assert_eq!(
            by_anchor("close-a-top-level-traversable").section_type,
            SectionType::Algorithm
        );
    }

    #[test]
    fn test_definition_content_spans_the_whole_list_item() {
        // A Wattsi property list item is often several <p>s: the definition, then notes and
        // caveats about it. All of them belong to the definition.
        let sections = collect_dfn_sections(
            include_str!("../../tests/fixtures/definitions/wattsi_property_list.html"),
            "https://html.spec.whatwg.org",
        );
        let content = |anchor: &str| {
            sections
                .iter()
                .find(|s| s.anchor == anchor)
                .unwrap()
                .content_text
                .clone()
                .unwrap()
        };

        let is_closing = content("is-closing");
        assert!(
            is_closing.contains("boolean, initially false"),
            "Missing the definition itself: {is_closing}"
        );
        assert!(
            is_closing.contains("top-level traversable navigables"),
            "Missing the note paragraph: {is_closing}"
        );

        let allowed = content("allowed-to-perform-a-navigation-or-history-update");
        assert!(
            allowed.contains("implementation-defined"),
            "Missing the definition itself: {allowed}"
        );
        assert!(
            allowed.contains("invoked too many"),
            "Missing the note paragraph: {allowed}"
        );

        // A <li> that is a single <p> must not gain a list bullet or trailing blank lines
        assert_eq!(
            content("nav-id"),
            "An **id**, a [new unique internal value](https://html.spec.whatwg.org#new-unique-internal-value)."
        );
    }

    #[test]
    fn test_argument_dfns_skipped() {
        // Bikeshed-generated W3C specs use data-dfn-type="argument" for function parameters.
        // These should be skipped, while method/attribute/interface/constructor dfns are kept.
        let html = r#"
            <h2 id="api">API</h2>
            <pre class="idl">
                <dfn data-dfn-type="interface" id="audiodecoder"><code>AudioDecoder</code></dfn>
                <dfn data-dfn-for="AudioDecoder" data-dfn-type="constructor" id="dom-audiodecoder-ctor"><code>AudioDecoder(init)</code></dfn>
                <dfn data-dfn-for="AudioDecoder/AudioDecoder(init)" data-dfn-type="argument" id="dom-audiodecoder-ctor-init"><code>init</code></dfn>
                <dfn data-dfn-for="AudioDecoder" data-dfn-type="method" id="dom-audiodecoder-configure"><code>configure(config)</code></dfn>
                <dfn data-dfn-for="AudioDecoder/configure(config)" data-dfn-type="argument" id="dom-audiodecoder-configure-config"><code>config</code></dfn>
                <dfn data-dfn-for="AudioDecoder" data-dfn-type="attribute" id="dom-audiodecoder-state"><code>state</code></dfn>
            </pre>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://test.example.com");
        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        let anchors: Vec<_> = sections.iter().map(|s| s.anchor.as_str()).collect();

        // Interface, constructor, method, attribute should be kept
        assert!(
            anchors.contains(&"audiodecoder"),
            "Interface should be kept"
        );
        assert!(
            anchors.contains(&"dom-audiodecoder-ctor"),
            "Constructor should be kept"
        );
        assert!(
            anchors.contains(&"dom-audiodecoder-configure"),
            "Method should be kept"
        );
        assert!(
            anchors.contains(&"dom-audiodecoder-state"),
            "Attribute should be kept"
        );

        // Arguments should be skipped
        assert!(
            !anchors.contains(&"dom-audiodecoder-ctor-init"),
            "Argument should be skipped"
        );
        assert!(
            !anchors.contains(&"dom-audiodecoder-configure-config"),
            "Argument should be skipped"
        );
    }

    #[test]
    fn test_wattsi_data_algorithm_divs_classified_by_body() {
        // Wattsi wraps predicates in <div data-algorithm=""> as well as algorithms. Only the
        // one with steps is an algorithm; a predicate's <ul> of conditions is part of its
        // definition's content.
        let html = include_str!("../../tests/fixtures/algorithms/wattsi_ul_algorithm.html");
        let converter = crate::parse::markdown::Converter::new("https://html.spec.whatwg.org");

        let document = Html::parse_document(html);
        let selector = Selector::parse("dfn[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_dfn_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        assert_eq!(sections.len(), 3, "Should detect all three dfns");

        let render_blocked = sections
            .iter()
            .find(|s| s.anchor == "render-blocked")
            .expect("render-blocked should be present");
        assert_eq!(render_blocked.section_type, SectionType::Definition);
        let content = render_blocked.content_text.as_ref().unwrap();
        assert!(
            content.contains("render-blocked"),
            "Should include intro text"
        );
        assert!(
            content.contains("render-blocking element set"),
            "Should include first condition from <ul>"
        );
        assert!(
            content.contains("implementation-defined"),
            "Should include second condition from <ul>"
        );

        let allows = sections
            .iter()
            .find(|s| s.anchor == "allows-adding-render-blocking-elements")
            .expect("allows-adding should be present");
        assert_eq!(allows.section_type, SectionType::Definition);

        let block_rendering = sections
            .iter()
            .find(|s| s.anchor == "block-rendering")
            .expect("block-rendering should be present");
        assert_eq!(
            block_rendering.section_type,
            SectionType::Algorithm,
            "div[data-algorithm] with <ol> should be Algorithm"
        );
        let content = block_rendering.content_text.as_ref().unwrap();
        assert!(content.contains("1. "), "Should include numbered steps");
    }

    fn section_types(html: &str) -> Vec<(String, SectionType)> {
        collect_dfn_sections(html, "https://html.spec.whatwg.org")
            .into_iter()
            .map(|s| (s.anchor, s.section_type))
            .collect()
    }

    #[test]
    fn test_dfns_without_steps_are_not_algorithms() {
        // Markup of the kinds of definitions Wattsi and Bikeshed put in algorithm divs, or
        // before a list, that have no steps.
        let html = r##"
            <div data-algorithm=""><p>The attribute's missing value default and invalid value default are both the <dfn id="attr-contenteditable-inherit-state">Inherit</dfn> state.</p></div>
            <div data-algorithm=""><p>The <code>canPlayType(type)</code> method must return the empty string if it cannot; it must return "<dfn data-dfn-for="CanPlayTypeResult" id="dom-canplaytyperesult-probably" data-dfn-type="enum-value"><code>probably</code></dfn>" if confident.</p></div>
            <div data-algorithm=""><p>The <dfn id="dom-texttrack-mode" data-dfn-type="attribute"><code>mode</code></dfn> getter steps are to return the string switching on this's mode:</p>
              <dl class="switch"><dt>The text track disabled mode</dt><dd>"<dfn data-dfn-for="TextTrackMode" id="dom-texttrack-disabled" data-dfn-type="enum-value"><code>disabled</code></dfn>"</dd></dl></div>
            <div data-algorithm=""><p>A <code>Document</code> is an <dfn id="unstyled-document">unstyled document</dfn> while it matches the following conditions:</p>
              <ul><li>The <code>Document</code> has no author style sheets.</li></ul></div>
            <div data-algorithm=""><p>A string is a <dfn id="valid-date-string">valid date string</dfn> representing a year <var>year</var>, month <var>month</var>, and day <var>day</var> if it consists of the following components in the given order:</p>
              <ol><li>A valid month string</li><li>A U+002D HYPHEN-MINUS character (-)</li></ol></div>
            <div data-algorithm=""><p>The <dfn data-dfn-for="DataTransferItem" id="dom-datatransferitem-type" data-dfn-type="attribute"><code>type</code></dfn> attribute must return the empty string if the <code>DataTransferItem</code> object is in the disabled mode.</p></div>
            <p>Each media element has an <dfn id="assigned-media-provider-object">assigned media provider object</dfn>, which is a media provider object or null, initially null.</p>
            <dl class="domintro"><dt><code>media.srcObject</code></dt><dd>Allows the media element to be assigned a media provider object.</dd></dl>
            <div class="algorithm" data-algorithm=""><p>A <code>StaticRange</code> is <dfn data-dfn-for="StaticRange" id="staticrange-valid" data-export="">valid</dfn> if all of the following are true:</p>
              <ul><li>Its start and end are in the same node tree.</li></ul></div>
            <p>A <dfn id="no-cors-safelisted-request-header-name" data-export="">no-CORS-safelisted request-header name</dfn> is a header name that is a byte-case-insensitive match for one of</p>
            <ul><li>`Accept`</li></ul>
        "##;
        let types = section_types(html);
        for (anchor, section_type) in &types {
            let expected = if anchor == "dom-texttrack-mode" {
                SectionType::Algorithm
            } else {
                SectionType::Definition
            };
            assert_eq!(section_type, &expected, "{anchor}");
        }
        assert_eq!(types.len(), 10, "{types:?}");
    }

    #[test]
    fn test_dfns_with_steps_are_algorithms() {
        let html = r##"
            <p>The <dfn data-dfn-for="Node" id="dom-node-textcontent" data-dfn-type="attribute"><code>textContent</code></dfn> getter steps are to return the result of running get text content with this.</p>
            <p>The <dfn id="dom-namednodemap-setnameditem" data-dfn-type="method"><code>setNamedItem(attr)</code></dfn> method steps are to return the result of setting an attribute given attr and element.</p>
            <div data-algorithm=""><p>The <code>checkValidity()</code> method, when invoked, must run the <dfn id="check-validity-steps">check validity steps</dfn> on this element.</p></div>
            <p>To <dfn id="set-text-content">set text content</dfn> with a node <var>node</var> and a string <var>value</var>, do as defined below, switching on the interface <var>node</var> implements:</p>
            <dl class="switch"><dt>Element</dt><dd>String replace all.</dd></dl>
            <p>When a user agent is to <dfn id="announce-the-connection">announce the connection</dfn>, the user agent must queue a task.</p>
            <p><dfn id="byte-serializing-a-request-origin">Byte-serializing a request origin</dfn>, given a request <var>request</var>, is to return the result of serializing a request origin with <var>request</var>.</p>
            <p>The <dfn id="concept-domain-to-ascii-parser">domain parser ToASCII</dfn> algorithm, given a scalar value string <var>domain</var>, returns the result of running Unicode ToASCII with <var>domain</var>.</p>
            <div class="algorithm"><p>To check if the environment settings object <var>environment</var> is <dfn id="is-offline">offline</dfn>:</p>
              <ul><li>If the user agent assumes it does not have internet connectivity, then return true.</li></ul></div>
            <p>To get a byte sequence <var>bytes</var> <dfn id="byte-sequence-as-a-body">as a body</dfn>, return the body of the result of safely extracting <var>bytes</var>.</p>
            <p>When the steps below require the UA to <dfn id="generate-implied-end-tags">generate implied end tags</dfn>, then, while the current node is a dd element, the UA must pop the current node.</p>
            <p>A script element has <dfn id="steps-to-run-when-the-result-is-ready">steps to run when the result is ready</dfn>, which are a series of steps or null, initially null. To <dfn id="mark-as-ready">mark as ready</dfn> a script element <var>el</var> given a <var>result</var>:</p>
            <ol><li>Set el's result to result.</li></ol>
            <div data-algorithm=""><p>The <dfn id="inner-navigate-event-firing-algorithm">inner navigate event firing algorithm</dfn> consists of the following steps, given a <var>navigation</var>:</p>
              <ol><li>If navigation has entries and events disabled, then return true.</li></ol></div>
            <p>The <dfn id="dom-range-collapse" data-dfn-type="method">collapse(toStart)</dfn> method steps are to, if toStart is true, set end to start; otherwise set start to end.</p>
            <p>The <dfn id="dom-document-open" data-dfn-type="method">open(unused1, unused2)</dfn> method must return the result of running the document open steps with this.</p>
            <p>When a fetch group <var>fetchGroup</var> is <dfn id="concept-fetch-group-terminate">terminated</dfn>:</p>
            <ol><li>For each fetch record, terminate it.</li></ol>
            <div data-algorithm=""><p>The <dfn id="rules-for-parsing-a-legacy-colour-value">rules for parsing a legacy color value</dfn>, given a string input, return a CSS color or failure.</p>
              <p class="note">Some obsolete legacy attributes parse colors.</p>
              <ol><li>If input is the empty string, then return failure.</li></ol></div>
        "##;
        let types = section_types(html);
        for (anchor, section_type) in &types {
            // A field whose value is steps, in the same paragraph as an algorithm
            let expected = if anchor == "steps-to-run-when-the-result-is-ready" {
                SectionType::Definition
            } else {
                SectionType::Algorithm
            };
            assert_eq!(section_type, &expected, "{anchor}");
        }
        assert_eq!(types.len(), 17, "{types:?}");
    }

    #[test]
    fn test_declaration_forms_must_declare_the_dfn() {
        let html = r##"
            <p>To ensure custom element reactions are triggered appropriately, we introduce the <dfn id="cereactions">[CEReactions]</dfn> IDL extended attribute.</p>
            <p>To supplement the above extended attributes we also introduce <dfn id="xattr-reflectrange">[ReflectRange]</dfn>.</p>
            <p>To run steps <dfn id="in-parallel">in parallel</dfn> means those steps are to be run, one after another.</p>
            <p>When the HTML parser is to operate on an input byte stream that has <dfn id="a-known-definite-encoding">a known definite encoding</dfn>, then the character encoding is that encoding.</p>
            <div class="example"><p>For these examples, we'll use a fake worklet, whose steps are to paint.</p>
            <pre><code class="idl">interface <dfn id="fakeworkletglobalscope" data-dfn-type="interface">FakeWorkletGlobalScope</dfn> {};</code></pre></div>
        "##;
        assert_eq!(
            section_types(html),
            vec![
                ("cereactions".to_string(), SectionType::Definition),
                ("xattr-reflectrange".to_string(), SectionType::Definition),
                ("in-parallel".to_string(), SectionType::Definition),
                (
                    "a-known-definite-encoding".to_string(),
                    SectionType::Definition
                ),
                ("fakeworkletglobalscope".to_string(), SectionType::Idl),
            ]
        );
    }

    #[test]
    fn test_definition_content_includes_its_condition_list() {
        let html = r##"
            <div class="algorithm" data-algorithm=""><p>A <code>StaticRange</code> is <dfn id="staticrange-valid">valid</dfn> if all of the following are true:</p>
              <ul><li>Its start and end are in the same node tree.</li><li>Its start offset is between 0 and its start node's length.</li></ul></div>
            <p>Each <a href="#navigable">navigable</a> has:</p>
            <ul><li><p>An <dfn id="nav-id">id</dfn>.</p></li></ul>
        "##;
        let sections = collect_dfn_sections(html, "https://dom.spec.whatwg.org");
        let valid = sections[0].content_text.as_deref().unwrap();
        assert!(valid.starts_with("A `StaticRange` is **valid**"), "{valid}");
        assert!(valid.contains("same node tree"), "{valid}");
        assert!(valid.contains("start node's length"), "{valid}");
        assert_eq!(sections[1].content_text.as_deref(), Some("An **id**."));
    }

    #[test]
    fn test_attribute_content_includes_its_setter_steps() {
        let html = r##"
            <div data-algorithm="">
            <p>The <dfn data-dfn-for="History" id="dom-history-scroll-restoration" data-dfn-type="attribute"><code>scrollRestoration</code></dfn> getter steps are:</p>
            <ol><li><p>Return this's scroll restoration mode.</p></li></ol>
            </div>
            <div data-algorithm="">
            <p>The <code><a href="#dom-history-scroll-restoration">scrollRestoration</a></code> setter steps are:</p>
            <ol><li><p>Set this's scroll restoration mode to the given value.</p></li></ol>
            </div>
            <div data-algorithm="">
            <p>The <dfn data-dfn-for="History" id="dom-history-state" data-dfn-type="attribute"><code>state</code></dfn> getter steps are:</p>
            <ol><li><p>Return this's state.</p></li></ol>
            </div>
        "##;
        let sections = collect_dfn_sections(html, "https://html.spec.whatwg.org");
        let content = sections[0].content_text.as_deref().unwrap();
        assert!(content.contains("getter steps are"), "{content}");
        assert!(content.contains("to the given value"), "{content}");
        assert!(!content.contains("state"), "{content}");
    }

    #[test]
    fn test_algorithm_name_mentioning_a_parameter_is_indexed() {
        let html = r##"
            <div data-algorithm="">
            <p><dfn id="fire-a-synthetic-pointer-event">Firing a synthetic pointer event named
            <var>e</var></dfn> at <var>target</var>, with an optional <var>not trusted flag</var>, means
            running these steps:</p>
            <ol><li><p>Let <var>event</var> be the result of creating an event.</p></li></ol>
            </div>
        "##;
        assert_eq!(
            section_types(html),
            vec![(
                "fire-a-synthetic-pointer-event".to_string(),
                SectionType::Algorithm
            )]
        );
    }

    // -- TC39/ecmarkup emu-clause tests --

    #[test]
    fn test_emu_clause_prose_section() {
        let html = r#"
            <emu-clause id="sec-overview">
                <h1><span class="secnum">4</span> Overview</h1>
                <p>This section contains a non-normative overview of the ECMAScript language.</p>
            </emu-clause>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        assert_eq!(section.anchor, "sec-overview");
        assert_eq!(section.title, Some("Overview".to_string()));
        assert_eq!(section.depth, Some(2)); // "4" = 1 part → depth 2
        assert_eq!(section.section_type, SectionType::Heading);
        assert!(section.content_text.is_some());
        assert!(section
            .content_text
            .as_ref()
            .unwrap()
            .contains("non-normative overview"));
    }

    #[test]
    fn test_emu_clause_algorithm_section() {
        let html = r#"
            <emu-clause id="sec-tostring" type="abstract operation" aoid="ToString">
                <h1><span class="secnum">7.1.17</span> ToString ( <var>argument</var> )</h1>
                <p>The abstract operation ToString converts argument to a String.</p>
                <emu-alg>
                    <ol>
                        <li>If <var>argument</var> is a String, return <var>argument</var>.</li>
                        <li>If <var>argument</var> is <emu-val>undefined</emu-val>, return "undefined".</li>
                        <li>If <var>argument</var> is <emu-val>null</emu-val>, return "null".</li>
                    </ol>
                </emu-alg>
            </emu-clause>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        assert_eq!(section.anchor, "sec-tostring");
        assert_eq!(section.title, Some("ToString ( argument )".to_string()));
        assert_eq!(section.depth, Some(4)); // "7.1.17" = 3 parts → depth 4
        assert_eq!(section.section_type, SectionType::Algorithm);

        let content = section.content_text.unwrap();
        assert!(
            content.contains("converts argument"),
            "Should have intro prose"
        );
        assert!(content.contains("1."), "Should have algorithm steps");
    }

    #[test]
    fn test_emu_clause_nested_sections_excluded_from_content() {
        let html = r#"
            <emu-clause id="sec-parent">
                <h1><span class="secnum">23</span> Parent Section</h1>
                <p>Intro text for the parent.</p>
                <emu-clause id="sec-child">
                    <h1><span class="secnum">23.1</span> Child Section</h1>
                    <p>This should NOT appear in parent content.</p>
                </emu-clause>
            </emu-clause>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();

        let mut sections = Vec::new();
        for element in document.select(&selector) {
            if let Some(section) = parse_emu_clause_element(&element, &converter).unwrap() {
                sections.push(section);
            }
        }

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].anchor, "sec-parent");
        assert_eq!(sections[1].anchor, "sec-child");

        // Parent content should NOT include child section content
        let parent_content = sections[0].content_text.as_ref().unwrap();
        assert!(parent_content.contains("Intro text"));
        assert!(!parent_content.contains("should NOT appear"));
    }

    #[test]
    fn test_secnum_depth_derivation() {
        // Helper to quickly test depth extraction
        fn depth_from_html(secnum: &str) -> Option<u8> {
            let html = format!(r#"<h1><span class="secnum">{}</span> Title</h1>"#, secnum);
            let document = Html::parse_document(&html);
            let selector = Selector::parse("h1").unwrap();
            let h1 = document.select(&selector).next().unwrap();
            extract_secnum_depth(&h1)
        }

        assert_eq!(depth_from_html("4"), Some(2)); // 1 part → depth 2
        assert_eq!(depth_from_html("4.3"), Some(3)); // 2 parts → depth 3
        assert_eq!(depth_from_html("7.1.17"), Some(4)); // 3 parts → depth 4
        assert_eq!(depth_from_html("23.1.3.30"), Some(5)); // 4 parts → depth 5
        assert_eq!(depth_from_html("A"), Some(2)); // annex, 1 part
        assert_eq!(depth_from_html("A.1"), Some(3)); // annex sub
        assert_eq!(depth_from_html("A.1.2"), Some(4)); // annex deep
    }

    #[test]
    fn test_emu_clause_secnum_stripped_from_title() {
        let html = r#"
            <emu-clause id="sec-test">
                <h1><span class="secnum">7.1.17</span> ToString ( <var>argument</var> )</h1>
            </emu-clause>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        // Title should not contain "7.1.17"
        let title = section.title.unwrap();
        assert!(
            !title.contains("7.1.17"),
            "secnum should be stripped: {}",
            title
        );
        assert!(
            title.contains("ToString"),
            "Title should have function name: {}",
            title
        );
    }

    #[test]
    fn test_emu_annex_parsed() {
        let html = r#"
            <emu-annex id="sec-additional-built-in-properties">
                <h1><span class="secnum">B</span> Additional Built-in Properties</h1>
                <p>Annex content here.</p>
            </emu-annex>
        "#;

        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-annex[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        assert_eq!(section.anchor, "sec-additional-built-in-properties");
        assert_eq!(
            section.title,
            Some("Additional Built-in Properties".to_string())
        );
        assert_eq!(section.depth, Some(2)); // "B" = 1 part → depth 2
    }

    // -- Integration tests using real TC39 HTML fixtures --

    #[test]
    fn test_ecmarkup_fixture_tostring_algorithm() {
        let html = include_str!("../../tests/fixtures/ecmarkup/tostring.html");
        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        assert_eq!(section.anchor, "sec-tostring");
        assert_eq!(section.title, Some("ToString ( argument )".to_string()));
        assert_eq!(section.depth, Some(4)); // "7.1.17" = 3 parts → depth 4
        assert_eq!(section.section_type, SectionType::Algorithm);

        let content = section.content_text.as_ref().unwrap();

        // Intro prose should be a single flowing paragraph
        assert!(
            content.contains("The abstract operation ToString takes argument *argument*"),
            "Intro should have italic var: {}",
            &content[..200]
        );
        assert!(
            content.contains("[ECMAScript language value](https://tc39.es/ecma262#sec-ecmascript-language-types)"),
            "emu-xref links should be inline markdown links"
        );

        // Algorithm steps should be numbered, one per line, no broken lines
        assert!(
            content.contains("1. If *argument* [is a String]("),
            "Step 1 should be on a single line with inline link"
        );
        assert!(
            content.contains("2. If *argument* [is a Symbol]("),
            "Step 2 should follow immediately"
        );
        assert!(
            content.contains("3. If *argument* is undefined, return \"undefined\"."),
            "Step 3: emu-val should render inline"
        );
        assert!(
            content.contains("10. Let *primValue* be ?"),
            "Step 10 should have var and link inline"
        );
        assert!(
            content.contains("10. Let *primValue*") && content.contains("[ToPrimitive]("),
            "Step 10 should have ToPrimitive link"
        );
        assert!(
            content.contains("12. Return ?") && content.contains("[ToString]("),
            "Step 12 should have recursive call"
        );

        // Steps should be on individual lines, not broken across multiple lines
        for i in 1..=12 {
            let prefix = format!("{}. ", i);
            let matches: Vec<_> = content
                .lines()
                .filter(|l| {
                    let trimmed = l.trim_start();
                    trimmed.starts_with(&prefix)
                        || (i >= 10 && trimmed.starts_with(&format!("{}.", i)))
                })
                .collect();
            assert!(
                !matches.is_empty(),
                "Step {} should appear on its own line",
                i
            );
        }
    }

    #[test]
    fn test_ecmarkup_fixture_undefined_type_prose() {
        let html = include_str!("../../tests/fixtures/ecmarkup/undefined_type.html");
        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let element = document.select(&selector).next().unwrap();

        let section = parse_emu_clause_element(&element, &converter)
            .unwrap()
            .unwrap();

        assert_eq!(
            section.anchor,
            "sec-ecmascript-language-types-undefined-type"
        );
        assert_eq!(section.title, Some("The Undefined Type".to_string()));
        assert_eq!(section.depth, Some(4)); // "6.1.1" = 3 parts → depth 4
        assert_eq!(section.section_type, SectionType::Heading);

        let content = section.content_text.as_ref().unwrap();

        // Should be a single paragraph, no spurious newlines from emu-val
        assert!(
            content.contains("The Undefined type has exactly one value, called undefined."),
            "emu-val should render inline as plain text: {}",
            content
        );
        assert!(
            content.contains("the value undefined."),
            "Second emu-val should also be inline"
        );
        // Should be a single line (one paragraph)
        let line_count = content.lines().count();
        assert!(
            line_count <= 2,
            "Simple prose should be 1-2 lines, got {}: {}",
            line_count,
            content
        );
    }

    #[test]
    fn extract_heading_number_bikeshed_secno() {
        // Bikeshed/Wattsi span.secno with trailing dot
        let html = r#"<h3 id="trees"><span class="secno">4.7.1. </span>Trees</h3>"#;
        let document = Html::parse_document(html);
        let selector = Selector::parse("h3").unwrap();
        let elem = document.select(&selector).next().unwrap();
        assert_eq!(
            extract_heading_number(&elem),
            Some("4.7.1".to_string()),
            "trailing dot and space should be stripped"
        );
    }

    #[test]
    fn extract_heading_number_ecmarkup_secnum() {
        // ecmarkup span.secnum without trailing dot
        let html = r#"<h1><span class="secnum">7.1.17</span>ToNumber</h1>"#;
        let document = Html::parse_document(html);
        let selector = Selector::parse("h1").unwrap();
        let elem = document.select(&selector).next().unwrap();
        assert_eq!(
            extract_heading_number(&elem),
            Some("7.1.17".to_string()),
            "number without trailing dot should survive unchanged"
        );
    }

    #[test]
    fn extract_heading_number_absent() {
        // No secno/secnum span: returns None
        let html = r#"<h2 id="intro">Introduction</h2>"#;
        let document = Html::parse_document(html);
        let selector = Selector::parse("h2").unwrap();
        let elem = document.select(&selector).next().unwrap();
        assert_eq!(extract_heading_number(&elem), None);
    }

    #[test]
    fn parse_heading_element_carries_number() {
        // A heading with a secno should have its number extracted.
        let html = r#"<h2 id="browsing"><span class="secno">7.4. </span>Browsing the web</h2>"#;
        let converter = crate::parse::markdown::Converter::new("https://html.spec.whatwg.org");
        let document = Html::parse_document(html);
        let selector = Selector::parse("h2[id]").unwrap();
        let elem = document.select(&selector).next().unwrap();
        let section = parse_heading_element(&elem, &converter).unwrap().unwrap();
        assert_eq!(section.number, Some("7.4".to_string()));
        assert_eq!(section.title, Some("Browsing the web".to_string()));
    }

    #[test]
    fn parse_emu_clause_element_carries_number() {
        // An emu-clause with a secnum in its h1 should have its number extracted.
        let html = r#"<emu-clause id="sec-tostringnumber" type="abstract operation">
          <h1><span class="secnum">7.1.17</span>ToString(<var>argument</var>)</h1>
          <p>Converts argument to a String value.</p>
        </emu-clause>"#;
        let converter = crate::parse::markdown::Converter::new("https://tc39.es/ecma262");
        let document = Html::parse_document(html);
        let selector = Selector::parse("emu-clause[id]").unwrap();
        let elem = document.select(&selector).next().unwrap();
        let section = parse_emu_clause_element(&elem, &converter)
            .unwrap()
            .unwrap();
        assert_eq!(section.number, Some("7.1.17".to_string()));
    }
}
