// ITU-T Recommendation PDF parsing
//
// ITU-T Recommendations are distributed as PDF only (no free full-text
// HTML, unlike RFCs). This module derives ParsedSection entries from a
// PDF's outline/bookmark tree (clause structure) and per-page body text,
// sliced at the outline's own destination coordinates rather than at whole
// page boundaries — see `positioned_toc` and `extract_range_text` below.

use crate::model::{ParsedSection, SectionType};
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use pdf_extract::{MediaBox, OutputDev, OutputError, Transform};
use regex::Regex;
use std::collections::HashMap;

/// Parse an ITU-T Recommendation PDF into structured sections, one per
/// outline/bookmark entry. The clause number (e.g. "D.3.28", "7.4.3.1") is
/// the leading whitespace-delimited token of the outline title; the anchor
/// therefore matches how these clauses are normally cited in prose.
pub fn parse_itu_pdf(bytes: &[u8]) -> Result<Vec<ParsedSection>> {
    if !bytes.starts_with(b"%PDF-") {
        // A non-PDF response (HTML login/interstitial page, 404, etc.) is a
        // fetch-layer problem, not a malformed PDF — say so instead of
        // handing the raw bytes to lopdf, whose "invalid file header" error
        // reads as "we can't parse this PDF" when we didn't fetch one.
        let preview: String = String::from_utf8_lossy(&bytes[..bytes.len().min(120)])
            .chars()
            .take(120)
            .collect();
        anyhow::bail!(
            "Fetched content is not a PDF (no %PDF- header); got: {preview:?}. \
             The source likely returned a login/interstitial/error page instead of the file."
        );
    }
    let mut doc = Document::load_mem(bytes).context("Failed to parse ITU-T PDF")?;
    if doc.is_encrypted() {
        // Real-world PDFs (e.g. ISO/IEC standards distributed via Adobe)
        // are commonly encrypted with an empty user password purely to
        // restrict editing/printing — viewing (and thus our text/outline
        // extraction) is unrestricted, so an empty password decrypts them.
        allow_v1_encrypt_with_length(&mut doc);
        doc.decrypt("").map_err(|e| {
            anyhow::anyhow!(
                "ITU-T PDF is encrypted and could not be decrypted with an empty password: {}",
                e
            )
        })?;
    }
    let toc = positioned_toc(&doc).map_err(|e| {
        anyhow::anyhow!(
            "ITU-T PDF has no outline/bookmark tree (needed to derive clause anchors): {}",
            e
        )
    })?;
    if toc.is_empty() {
        anyhow::bail!("ITU-T PDF outline is empty; cannot derive any clause anchors");
    }

    let pages = extract_positioned_pages(&doc)
        .map_err(|e| anyhow::anyhow!("Failed to extract text from ITU-T PDF: {}", e))?;
    let last_page = pages.len().saturating_sub(1);
    let furniture = detect_furniture(&pages);

    let mut sections = Vec::with_capacity(toc.len());
    for (i, entry) in toc.iter().enumerate() {
        let (anchor, title) = split_anchor_title(&entry.title);
        if anchor.is_empty() {
            continue;
        }

        let start = cut_for(entry.page, entry.dest_top, &pages);
        let end = match toc.get(i + 1) {
            Some(next) => cut_for(next.page, next.dest_top, &pages),
            None => Cut {
                page: last_page,
                y: f64::INFINITY,
            },
        };
        let content_text = extract_range_text(&pages, start, end, &furniture);

        sections.push(ParsedSection {
            anchor,
            title: if title.is_empty() { None } else { Some(title) },
            content_text,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(entry.level.min(255) as u8),
            number: None,
        });
    }

    Ok(sections)
}

/// Work around an `lopdf` 0.36 decryption bug: it refuses to decrypt when
/// `/V 1` and `/Length` are both present in the Encrypt dictionary, even
/// though real producers legitimately include both (Adobe's own PDF32000
/// distribution does). Removing `/Length` is safe for `/V 1` — the spec
/// only ever allows a 40-bit key there regardless of what `/Length` says,
/// and that's exactly what lopdf falls back to once it's absent.
fn allow_v1_encrypt_with_length(doc: &mut Document) {
    let Ok(encrypt_id) = doc.trailer.get(b"Encrypt").and_then(Object::as_reference) else {
        return;
    };
    let Ok(dict) = doc.get_dictionary_mut(encrypt_id) else {
        return;
    };
    if matches!(dict.get(b"V"), Ok(Object::Integer(1))) {
        dict.remove(b"Length");
    }
}

/// Split an outline title into (anchor, display title): the leading
/// whitespace-delimited token is the clause number, the rest is the title.
/// "D.3.28 Mastering display..." -> ("D.3.28", "Mastering display...").
/// Deliberately does not special-case Annex nodes (e.g. "D  Annex D  ...")
/// — the same rule already yields the correct anchor for those.
fn split_anchor_title(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    match trimmed.split_once(char::is_whitespace) {
        Some((anchor, rest)) => (anchor.to_string(), rest.trim().to_string()),
        None => (trimmed.to_string(), String::new()),
    }
}

/// Collapse intra-line whitespace (including the double-spaces
/// `pdf-extract` produces for justified text) into single spaces, while
/// keeping one newline per source line. Syntax tables render as one line
/// per row already (`pdf-extract`'s line-break heuristic tracks the same
/// y-jumps a human reader would), so preserving that structure keeps a
/// table's field/descriptor columns readable instead of linearizing the
/// whole table into one run-on line.
fn collapse_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Outline walk with destination positions --------------------------
//
// `lopdf::Document::get_toc()` gives us (level, title, page) but silently
// discards the rest of the destination array: a `/GoTo` action's `/D` is
// `[page /XYZ left top zoom]` (or `[page /FitH top]` / `[page /FitBH top]`),
// and lopdf's `Destination` only ever keeps `page` and the type name. That
// `top` value is exactly the anchor position we need to slice a page at, so
// this walks the outline tree ourselves, mirroring lopdf's own traversal,
// to keep it.

/// One outline/bookmark entry with its destination's page and (when
/// derivable) vertical position on that page, in raw PDF user-space units
/// (origin bottom-left, increasing upward).
struct TocEntry {
    level: usize,
    title: String,
    page: usize,
    dest_top: Option<f64>,
}

fn positioned_toc(doc: &Document) -> Result<Vec<TocEntry>> {
    let catalog = doc.catalog()?;
    let outlines_dict = doc.get_dict_in_dict(catalog, b"Outlines")?;
    let named = named_destinations(doc);
    let page_id_to_num = page_id_to_num_map(doc);

    let mut entries = Vec::new();
    if let Ok(first) = doc.get_dict_in_dict(outlines_dict, b"First") {
        walk_outline(doc, first, 1, &named, &page_id_to_num, &mut entries);
    }
    Ok(entries)
}

fn walk_outline(
    doc: &Document,
    node: &Dictionary,
    level: usize,
    named: &HashMap<Vec<u8>, Vec<Object>>,
    page_id_to_num: &HashMap<ObjectId, usize>,
    out: &mut Vec<TocEntry>,
) {
    let mut node = node;
    loop {
        if let Some(entry) = outline_entry(doc, node, level, named, page_id_to_num) {
            out.push(entry);
        }
        if let Ok(first) = doc.get_dict_in_dict(node, b"First") {
            walk_outline(doc, first, level + 1, named, page_id_to_num, out);
        }
        match doc.get_dict_in_dict(node, b"Next") {
            Ok(next) => node = next,
            Err(_) => break,
        }
    }
}

fn outline_entry(
    doc: &Document,
    node: &Dictionary,
    level: usize,
    named: &HashMap<Vec<u8>, Vec<Object>>,
    page_id_to_num: &HashMap<ObjectId, usize>,
) -> Option<TocEntry> {
    let title = decode_pdf_string(node.get(b"Title").ok()?.as_str().ok()?);

    let dest_obj: &Object = match doc.get_dict_in_dict(node, b"A") {
        Ok(action) => {
            let command = action.get(b"S").ok()?.as_name().ok()?;
            if command != b"GoTo" && command != b"GoToR" {
                return None;
            }
            action.get(b"D").ok()?
        }
        Err(_) => node.get(b"Dest").ok()?,
    };

    let (page_id, dest_top) = resolve_dest(doc, dest_obj, named)?;
    let page = *page_id_to_num.get(&page_id)?;
    Some(TocEntry {
        level,
        title,
        page,
        dest_top,
    })
}

fn resolve_dest(
    doc: &Document,
    dest_obj: &Object,
    named: &HashMap<Vec<u8>, Vec<Object>>,
) -> Option<(ObjectId, Option<f64>)> {
    match dest_obj {
        Object::Array(arr) => dest_array_to_page_top(arr),
        Object::Reference(id) => resolve_dest(doc, doc.get_object(*id).ok()?, named),
        Object::String(key, _) | Object::Name(key) => dest_array_to_page_top(named.get(key)?),
        _ => None,
    }
}

fn dest_array_to_page_top(arr: &[Object]) -> Option<(ObjectId, Option<f64>)> {
    let page_id = arr.first()?.as_reference().ok()?;
    Some((page_id, dest_top_from_array(arr)))
}

/// Pull the vertical anchor position out of a `/D` destination array, for
/// the destination types that carry one directly. Other types (`/Fit`,
/// `/FitB`, plain page references, ...) return `None` — callers then fall
/// back to whole-page boundaries for that entry, same as before this fix.
fn dest_top_from_array(arr: &[Object]) -> Option<f64> {
    let kind = arr.get(1)?.as_name().ok()?;
    let idx = match kind {
        b"XYZ" => 3,
        b"FitH" | b"FitBH" => 2,
        _ => return None,
    };
    match arr.get(idx)? {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}

/// Decode a PDF string object per the spec: UTF-16BE/LE with a BOM, or
/// PDFDocEncoding (treated as Latin-1/UTF-8 lossy here, same as lopdf's
/// `get_toc()`).
fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xfe && bytes[1] == 0xff && bytes.len().is_multiple_of(2) {
        let units: Vec<u16> = bytes
            .chunks(2)
            .skip(1)
            .map(|x| ((x[0] as u16) << 8) | x[1] as u16)
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xfe && bytes.len().is_multiple_of(2) {
        let units: Vec<u16> = bytes
            .chunks(2)
            .skip(1)
            .map(|x| ((x[1] as u16) << 8) | x[0] as u16)
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).to_string()
}

fn page_id_to_num_map(doc: &Document) -> HashMap<ObjectId, usize> {
    doc.get_pages()
        .into_iter()
        .map(|(num, id)| (id, num as usize))
        .collect()
}

fn named_destinations(doc: &Document) -> HashMap<Vec<u8>, Vec<Object>> {
    let mut out = HashMap::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    if let Ok(tree) = doc.get_dict_in_dict(catalog, b"Dests") {
        collect_named_destinations(doc, tree, &mut out);
    }
    if let Ok(names) = doc.get_dict_in_dict(catalog, b"Names") {
        if let Ok(dests) = doc.get_dict_in_dict(names, b"Dests") {
            collect_named_destinations(doc, dests, &mut out);
        }
    }
    out
}

fn collect_named_destinations(
    doc: &Document,
    tree: &Dictionary,
    out: &mut HashMap<Vec<u8>, Vec<Object>>,
) {
    if let Ok(kids) = tree.get(b"Kids").and_then(|k| k.as_array()) {
        for kid in kids {
            if let Ok(id) = kid.as_reference() {
                if let Ok(dict) = doc.get_dictionary(id) {
                    collect_named_destinations(doc, dict, out);
                }
            }
        }
    }
    if let Ok(names) = tree.get(b"Names").and_then(|n| n.as_array()) {
        let mut it = names.iter();
        while let (Some(key_obj), Some(val_obj)) = (it.next(), it.next()) {
            let Ok(key_bytes) = key_obj.as_str() else {
                continue;
            };
            if let Some(arr) = resolve_named_value(doc, val_obj) {
                out.insert(key_bytes.to_vec(), arr);
            }
        }
    }
}

fn resolve_named_value(doc: &Document, val: &Object) -> Option<Vec<Object>> {
    let val = match val {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    match val {
        Object::Array(arr) => Some(arr.clone()),
        Object::Dictionary(dict) => dict.get(b"D").ok()?.as_array().ok().cloned(),
        _ => None,
    }
}

// --- Positioned per-page text extraction -------------------------------

/// A page's extracted text, plus a checkpoint list mapping byte offsets in
/// `text` to the (flipped, top-down) y-coordinate of the character at that
/// offset, in traversal order. `height` is the page's media box height,
/// needed to flip a destination's raw (bottom-up) `top` into the same
/// coordinate frame.
struct PagePosText {
    text: String,
    checkpoints: Vec<(usize, f64)>,
    height: f64,
}

fn extract_positioned_pages(doc: &Document) -> Result<Vec<Option<PagePosText>>, OutputError> {
    let pages_map = doc.get_pages();
    let max_page = pages_map.keys().max().copied().unwrap_or(0) as usize;
    let mut out: Vec<Option<PagePosText>> = (0..=max_page).map(|_| None).collect();
    for &page_num in pages_map.keys() {
        // `pdf_extract` panics (rather than returning `Err`) on some fonts
        // it can't fully parse — real producers hit this (e.g. certain CFF
        // fonts in Adobe's own ISO 32000-1 PDF). One unparseable page must
        // not take down extraction for the other ~1000; treat a caught
        // panic the same as an `Err`: skip that page, keep going.
        let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut output = PositionedTextOutput::new();
            pdf_extract::output_doc_page(doc, &mut output, page_num).map(|()| output)
        }));
        if let Ok(Ok(output)) = extracted {
            out[page_num as usize] = Some(PagePosText {
                text: output.text,
                checkpoints: output.checkpoints,
                height: output.page_height,
            });
        }
    }
    Ok(out)
}

/// A fork of `pdf_extract::PlainTextOutput` that additionally records,
/// after every character, the byte offset it was written at and its
/// flipped y-coordinate — giving us a way to slice the accumulated text at
/// an arbitrary vertical position instead of only at page boundaries.
struct PositionedTextOutput {
    text: String,
    checkpoints: Vec<(usize, f64)>,
    last_end: f64,
    last_y: f64,
    first_char: bool,
    flip_ctm: Transform,
    page_height: f64,
}

impl PositionedTextOutput {
    fn new() -> Self {
        PositionedTextOutput {
            text: String::new(),
            checkpoints: Vec::new(),
            last_end: 100000.,
            last_y: 0.,
            first_char: false,
            flip_ctm: Transform::row_major(1., 0., 0., 1., 0., 0.),
            page_height: 0.0,
        }
    }
}

impl OutputDev for PositionedTextOutput {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        self.page_height = media_box.ury - media_box.lly;
        self.flip_ctm = Transform::row_major(1., 0., 0., -1., 0., self.page_height);
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(euclid::vec2(font_size, font_size));
        let transformed_font_size =
            (transformed_font_size_vec.x * transformed_font_size_vec.y).sqrt();
        let (x, y) = (position.m31, position.m32);

        if self.first_char {
            if (y - self.last_y).abs() > transformed_font_size * 1.5 {
                self.text.push('\n');
            }
            if x < self.last_end && (y - self.last_y).abs() > transformed_font_size * 0.5 {
                self.text.push('\n');
            }
            if x > self.last_end + transformed_font_size * 0.1 {
                self.text.push(' ');
            }
        }

        let offset = self.text.len();
        self.text.push_str(char);
        self.checkpoints.push((offset, y));

        self.first_char = false;
        self.last_y = y;
        self.last_end = x + width * transformed_font_size;
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.first_char = true;
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

// --- Slicing at destination positions -----------------------------------

/// A slicing boundary: page number (1-indexed) and flipped (top-down)
/// y-coordinate on that page.
#[derive(Clone, Copy)]
struct Cut {
    page: usize,
    y: f64,
}

/// Build the `Cut` for an outline entry. A missing `dest_top` (destination
/// type without a vertical position) becomes y=0, i.e. "the very top of the
/// page" — for a start cut that includes the whole page, for an end cut
/// that excludes it entirely, both matching the pre-fix whole-page-range
/// behavior for that boundary.
fn cut_for(page: usize, dest_top: Option<f64>, pages: &[Option<PagePosText>]) -> Cut {
    let y = match dest_top {
        Some(top) => pages
            .get(page)
            .and_then(|p| p.as_ref())
            .map(|p| (p.height - top).max(0.0))
            .unwrap_or(0.0),
        None => 0.0,
    };
    Cut { page, y }
}

/// Extract the whitespace-collapsed text between two `Cut`s: same page ->
/// slice within it; different pages -> the tail of the start page, all
/// full pages strictly between, and the head of the end page. Running
/// headers/footers (`furniture`) are stripped before whitespace collapse.
fn extract_range_text(
    pages: &[Option<PagePosText>],
    start: Cut,
    end: Cut,
    furniture: &[Regex],
) -> Option<String> {
    let mut buf = String::new();
    if start.page == end.page {
        if let Some(page) = pages.get(start.page).and_then(|p| p.as_ref()) {
            let s = offset_for_y(page, start.y);
            let e = offset_for_y(page, end.y);
            if s < e {
                buf.push_str(&page.text[s..e]);
            }
        }
    } else {
        if let Some(page) = pages.get(start.page).and_then(|p| p.as_ref()) {
            buf.push_str(&page.text[offset_for_y(page, start.y)..]);
        }
        for p in (start.page + 1)..end.page {
            if let Some(page) = pages.get(p).and_then(|p| p.as_ref()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&page.text);
            }
        }
        if let Some(page) = pages.get(end.page).and_then(|p| p.as_ref()) {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&page.text[..offset_for_y(page, end.y)]);
        }
    }

    for re in furniture {
        if re.is_match(&buf) {
            buf = re.replace_all(&buf, " ").to_string();
        }
    }

    let collapsed = collapse_whitespace(&buf);
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

// --- Running header/footer detection ------------------------------------

/// Detect page furniture (running headers/footers) by looking at each
/// page's first and last non-empty line: a line that recurs, modulo its
/// page number, across a large fraction of pages is furniture, not clause
/// content. Returns one regex per detected pattern, each matching any
/// concrete instance regardless of that page's actual digits.
fn detect_furniture(pages: &[Option<PagePosText>]) -> Vec<Regex> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut page_total = 0usize;
    for page in pages.iter().flatten() {
        page_total += 1;
        for line in edge_lines(&page.text) {
            let normalized = digit_normalized(&line);
            if is_plausible_furniture(&normalized) {
                *counts.entry(normalized).or_insert(0) += 1;
            }
        }
    }

    let threshold = (page_total / 3).max(3);
    counts
        .into_iter()
        .filter(|(_, n)| *n >= threshold)
        .filter_map(|(normalized, _)| furniture_regex(&normalized).ok())
        .collect()
}

/// The first and last non-empty lines of a page's text — running
/// headers/footers live in the page margins, i.e. at the extremes.
fn edge_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut out = Vec::new();
    if let Some(first) = lines.first() {
        out.push((*first).to_string());
    }
    if lines.len() > 1 {
        if let Some(last) = lines.last() {
            out.push((*last).to_string());
        }
    }
    out
}

/// Replace every run of ASCII digits with a single `#` placeholder, so
/// e.g. page 34's and page 35's copy of the same running footer normalize
/// to the same string despite differing page numbers.
fn digit_normalized(line: &str) -> String {
    let mut out = String::new();
    let mut in_digits = false;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
                in_digits = true;
            }
        } else {
            in_digits = false;
            out.push(ch);
        }
    }
    out
}

/// A running header/footer is one line, not a paragraph, and must contain
/// some non-numeric text — an all-digit line (a lone page number) is too
/// generic to safely strip from every page that happens to have one.
fn is_plausible_furniture(normalized: &str) -> bool {
    normalized.chars().count() <= 80 && normalized.chars().any(|c| c.is_alphabetic())
}

/// Build a regex matching any concrete instance of a digit-normalized
/// furniture pattern (whatever digits substitute for each `#`), anchored
/// to a whole line so it can't match a citation of the same text embedded
/// mid-paragraph in real clause content.
fn furniture_regex(normalized: &str) -> std::result::Result<Regex, regex::Error> {
    let mut pattern = String::from(r"(?m)^\s*");
    for (i, part) in normalized.split('#').enumerate() {
        if i > 0 {
            pattern.push_str(r"\d+");
        }
        pattern.push_str(&regex::escape(part));
    }
    pattern.push_str(r"\s*$");
    Regex::new(&pattern)
}

/// Find the byte offset in `page.text` of the character whose y-coordinate
/// is closest to `target_y`, i.e. where a clause boundary at that y falls.
///
/// Deliberately *not* "first checkpoint reaching target_y in stream order":
/// running headers/footers are commonly emitted out of visual order (e.g.
/// the footer painted before the body text that starts higher up the same
/// page), so checkpoint y-values are not monotonic in offset order. Nearest
/// match is robust to that; a naive forward scan is not — it would latch
/// onto page furniture the moment target_y fell below the furniture's y.
fn offset_for_y(page: &PagePosText, target_y: f64) -> usize {
    if !target_y.is_finite() {
        return page.text.len();
    }
    page.checkpoints
        .iter()
        .min_by(|(_, y1), (_, y2)| {
            (y1 - target_y)
                .abs()
                .partial_cmp(&(y2 - target_y).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|&(offset, _)| offset)
        .unwrap_or_else(|| page.text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Builds a minimal synthetic PDF in memory: one page per string in
    // `pages`, and an outline built from `bookmarks` — each entry is
    // (title, page_index, parent_index_into_this_slice).
    fn build_test_pdf(pages: &[&str], bookmarks: &[(&str, usize, Option<usize>)]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Bookmark, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let page_ids: Vec<(u32, u16)> = pages
            .iter()
            .map(|text| {
                let content = Content {
                    operations: vec![
                        Operation::new("BT", vec![]),
                        Operation::new("Tf", vec!["F1".into(), 12.into()]),
                        Operation::new("Td", vec![72.into(), 700.into()]),
                        Operation::new("Tj", vec![Object::string_literal(*text)]),
                        Operation::new("ET", vec![]),
                    ],
                };
                let content_id =
                    doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
                doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => content_id,
                    "Resources" => resources_id,
                })
            })
            .collect();

        let kids: Vec<Object> = page_ids.iter().map(|id| (*id).into()).collect();
        let pages_dict = dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut bookmark_ids: Vec<u32> = Vec::with_capacity(bookmarks.len());
        for (title, page_idx, parent_idx) in bookmarks {
            let bm = Bookmark::new(
                (*title).to_string(),
                [0.0, 0.0, 0.0],
                0,
                page_ids[*page_idx],
            );
            let parent = parent_idx.map(|i| bookmark_ids[i]);
            bookmark_ids.push(doc.add_bookmark(bm, parent));
        }

        if let Some(outline_id) = doc.build_outline() {
            if let Ok(catalog) = doc.catalog_mut() {
                catalog.set("Outlines", outline_id);
            }
        }

        doc.compress();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    // Builds a single-page PDF with two lines of text at distinct y
    // positions, and two top-level outline bookmarks pointing at that same
    // page via `/XYZ` destinations whose `top` values fall between the two
    // lines — the real-world shape that ITU-T PDFs use, and the shape the
    // shared-page bug (#1) drops entirely.
    fn build_shared_page_test_pdf() -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(
                        "Clause one body text about widgets.",
                    )],
                ),
                Operation::new("ET", vec![]),
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![72.into(), 600.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(
                        "Clause two body text about gadgets.",
                    )],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
        });

        let pages_dict = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

        // Manual outline: two siblings, each a /GoTo action with an /XYZ
        // destination on the same page. `top` 720 sits above line one
        // (baseline y=700); `top` 620 sits above line two (baseline y=600).
        let outlines_id = doc.new_object_id();
        let bm_a_id = doc.new_object_id();
        let bm_b_id = doc.new_object_id();

        let action_a = doc.add_object(dictionary! {
            "S" => "GoTo",
            "D" => vec![page_id.into(), "XYZ".into(), 0.into(), 720.into(), 0.into()],
        });
        let action_b = doc.add_object(dictionary! {
            "S" => "GoTo",
            "D" => vec![page_id.into(), "XYZ".into(), 0.into(), 620.into(), 0.into()],
        });

        doc.objects.insert(
            bm_a_id,
            Object::Dictionary(dictionary! {
                "Title" => Object::string_literal("1 Widgets"),
                "Parent" => outlines_id,
                "A" => action_a,
                "Next" => bm_b_id,
            }),
        );
        doc.objects.insert(
            bm_b_id,
            Object::Dictionary(dictionary! {
                "Title" => Object::string_literal("2 Gadgets"),
                "Parent" => outlines_id,
                "A" => action_b,
                "Prev" => bm_a_id,
            }),
        );
        doc.objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => bm_a_id,
                "Last" => bm_b_id,
                "Count" => 2,
            }),
        );

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Outlines" => outlines_id,
        });
        doc.trailer.set("Root", catalog_id);

        doc.compress();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn test_parse_itu_pdf_shared_page_clauses_both_get_content() {
        let bytes = build_shared_page_test_pdf();
        let sections = parse_itu_pdf(&bytes).unwrap();
        assert_eq!(sections.len(), 2);

        assert_eq!(sections[0].anchor, "1");
        let content_a = sections[0].content_text.as_deref().unwrap_or_default();
        assert!(
            content_a.contains("widgets"),
            "clause 1 must contain its own text, got: {content_a:?}"
        );
        assert!(
            !content_a.contains("gadgets"),
            "clause 1 must not bleed into clause 2's text, got: {content_a:?}"
        );

        assert_eq!(sections[1].anchor, "2");
        let content_b = sections[1].content_text.as_deref().unwrap_or_default();
        assert!(
            content_b.contains("gadgets"),
            "clause 2 must contain its own text, got: {content_b:?}"
        );
        assert!(
            !content_b.contains("widgets"),
            "clause 2 must not bleed into clause 1's text, got: {content_b:?}"
        );
    }

    // Builds an `n`-page PDF where every page carries a running footer
    // ("Test Recommendation Footer <NNN>", digits varying per page) emitted
    // *after* the page's body text in the content stream — so a naive
    // tail-of-page slice would include it — plus one top-level bookmark per
    // page pointing at that page's body line via an `/XYZ` destination.
    fn build_multi_page_footer_test_pdf(n: usize) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        // Distinct wording per page (not just a digit substitution) — a
        // real leaf clause's own prose, unlike a running footer, doesn't
        // literally repeat itself save for a page number.
        const BODY_WORDS: [&str; 6] = [
            "widgets",
            "gadgets",
            "gizmos",
            "doodads",
            "trinkets",
            "contraptions",
        ];

        let mut page_ids = Vec::with_capacity(n);
        for i in 0..n {
            let body = format!(
                "Some clause body text discussing {}.",
                BODY_WORDS[i % BODY_WORDS.len()]
            );
            let footer = format!("Test Recommendation Footer {}", 100 + i);
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Td", vec![72.into(), 700.into()]),
                    Operation::new("Tj", vec![Object::string_literal(body)]),
                    Operation::new("ET", vec![]),
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Td", vec![72.into(), 40.into()]),
                    Operation::new("Tj", vec![Object::string_literal(footer)]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
            });
            page_ids.push(page_id);
        }

        let kids: Vec<Object> = page_ids.iter().map(|id| (*id).into()).collect();
        let pages_dict = dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

        // One top-level bookmark per page, `/XYZ` dest pointing just above
        // the body line (raw top=720, body baseline at raw y=700).
        let outlines_id = doc.new_object_id();
        let bm_ids: Vec<lopdf::ObjectId> = (0..n).map(|_| doc.new_object_id()).collect();
        for (i, &bm_id) in bm_ids.iter().enumerate() {
            let action = doc.add_object(dictionary! {
                "S" => "GoTo",
                "D" => vec![page_ids[i].into(), "XYZ".into(), 0.into(), 720.into(), 0.into()],
            });
            let mut dict = dictionary! {
                "Title" => Object::string_literal(format!("{} Clause{i}", i + 1)),
                "Parent" => outlines_id,
                "A" => action,
            };
            if i > 0 {
                dict.set("Prev", bm_ids[i - 1]);
            }
            if i + 1 < n {
                dict.set("Next", bm_ids[i + 1]);
            }
            doc.objects.insert(bm_id, Object::Dictionary(dict));
        }
        doc.objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => bm_ids[0],
                "Last" => *bm_ids.last().unwrap(),
                "Count" => n as i64,
            }),
        );

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Outlines" => outlines_id,
        });
        doc.trailer.set("Root", catalog_id);

        doc.compress();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn test_parse_itu_pdf_strips_running_footer() {
        const BODY_WORDS: [&str; 6] = [
            "widgets",
            "gadgets",
            "gizmos",
            "doodads",
            "trinkets",
            "contraptions",
        ];
        let bytes = build_multi_page_footer_test_pdf(6);
        let sections = parse_itu_pdf(&bytes).unwrap();
        assert_eq!(sections.len(), 6);

        for (i, section) in sections.iter().enumerate() {
            let content = section.content_text.as_deref().unwrap_or_default();
            assert!(
                content.contains(BODY_WORDS[i % BODY_WORDS.len()]),
                "clause {i} must keep its own body text, got: {content:?}"
            );
            assert!(
                !content.contains("Test Recommendation Footer"),
                "clause {i} must not retain the running footer, got: {content:?}"
            );
        }
    }

    #[test]
    fn test_extract_range_text_strips_furniture_across_page_boundary() {
        // Regression: joining page chunks with a plain space let a footer at
        // the bottom of one page and a header at the top of the next merge
        // into a single text line ("...Footer Text Header Text..."), which
        // the per-line-anchored furniture regexes can't match against —
        // both leaked into the clause content verbatim.
        let footer_re = furniture_regex("Page Footer Text").unwrap();
        let header_re = furniture_regex("Page Header Text").unwrap();
        let page1_text = "Body one line.\nPage Footer Text".to_string();
        let page2_text = "Page Header Text\nBody two line.".to_string();
        let pages = vec![
            None,
            Some(PagePosText {
                checkpoints: vec![(0, 100.0), ("Body one line.\n".len(), 800.0)],
                text: page1_text,
                height: 842.0,
            }),
            Some(PagePosText {
                checkpoints: vec![
                    (0, 50.0),
                    ("Page Header Text\n".len(), 200.0),
                    (page2_text.len(), 1000.0),
                ],
                text: page2_text.clone(),
                height: 842.0,
            }),
        ];
        let start = Cut { page: 1, y: 100.0 };
        let end = Cut { page: 2, y: 1000.0 };
        let result = extract_range_text(&pages, start, end, &[footer_re, header_re]).unwrap();
        assert!(!result.contains("Footer"), "footer leaked: {result:?}");
        assert!(!result.contains("Header"), "header leaked: {result:?}");
        assert!(result.contains("Body one line"));
        assert!(result.contains("Body two line"));
    }

    #[test]
    fn test_parse_itu_pdf_basic_structure() {
        let bytes = build_test_pdf(
            &[
                "Scope page text.",
                "Clause one body text about widgets.",
                "Clause two body text about gadgets.",
            ],
            &[
                ("1 Scope", 0, None),
                ("1.1 Widgets", 1, Some(0)),
                ("1.2 Gadgets", 2, Some(0)),
            ],
        );

        let sections = parse_itu_pdf(&bytes).unwrap();
        assert_eq!(sections.len(), 3);

        assert_eq!(sections[0].anchor, "1");
        assert_eq!(sections[0].title, Some("Scope".to_string()));
        assert_eq!(sections[0].depth, Some(1));

        assert_eq!(sections[1].anchor, "1.1");
        assert_eq!(sections[1].title, Some("Widgets".to_string()));
        assert_eq!(sections[1].depth, Some(2));
        let content1 = sections[1].content_text.as_ref().unwrap();
        assert!(content1.contains("widgets"));

        assert_eq!(sections[2].anchor, "1.2");
        assert_eq!(sections[2].title, Some("Gadgets".to_string()));
        let content2 = sections[2].content_text.as_ref().unwrap();
        assert!(content2.contains("gadgets"));
    }

    #[test]
    fn test_parse_itu_pdf_no_outline_errors() {
        let bytes = build_test_pdf(&["Just a page."], &[]);
        let result = parse_itu_pdf(&bytes);
        assert!(
            result.is_err(),
            "a PDF with no outline must fail loudly, not return zero sections silently"
        );
    }

    #[test]
    fn test_parse_itu_pdf_non_pdf_content_gives_clear_error() {
        // Regression: a fetch that lands on a login/interstitial/404 HTML
        // page (real-world case: ITU-T's dologin_pub.asp redirecting to a
        // choice/error page for some Recommendations) must not surface
        // lopdf's cryptic "invalid file header" — the actual problem is
        // that the fetch didn't get a PDF at all.
        let html = b"<html><head><title>Document Not Found</title></head></html>";
        let err = parse_itu_pdf(html).unwrap_err();
        assert!(
            err.to_string().contains("not a PDF"),
            "expected a clear non-PDF-content error, got: {err}"
        );
    }

    #[test]
    fn test_split_anchor_title_simple() {
        let (anchor, title) =
            split_anchor_title("D.3.28 Mastering display colour volume SEI message semantics");
        assert_eq!(anchor, "D.3.28");
        assert_eq!(
            title,
            "Mastering display colour volume SEI message semantics"
        );
    }

    #[test]
    fn test_split_anchor_title_annex_double_space() {
        // Real ITU-T H.265 outline title for the Annex D node: no
        // special-casing needed, the leading token is still correct.
        let (anchor, title) =
            split_anchor_title("D  Annex D  Supplemental enhancement information");
        assert_eq!(anchor, "D");
        assert_eq!(title, "Annex D  Supplemental enhancement information");
    }

    #[test]
    fn test_split_anchor_title_no_remainder() {
        let (anchor, title) = split_anchor_title("D.3.28");
        assert_eq!(anchor, "D.3.28");
        assert_eq!(title, "");
    }

    #[test]
    fn test_collapse_whitespace() {
        assert_eq!(collapse_whitespace("a   b\n\nc\t d"), "a b\nc d");
    }
}
