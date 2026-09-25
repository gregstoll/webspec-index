//! Independent minimal section extractor (design §5.4–§5.6).
//!
//! Deliberately shares no boundary logic with `parse::sections`: it takes section types and scope
//! anchors from the index and decides everything else from the source HTML itself.

use std::collections::{HashMap, HashSet};

use ego_tree::{NodeId, NodeRef};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use scraper::{ElementRef, Html, Node, Selector};
use webspec_index::model::SectionType;

pub type N<'a> = NodeRef<'a, Node>;

/// Tags whose `id` the indexer turns into a section (`parse_generic_html`).
pub const ANCHOR_SELECTOR: &str =
    "h2[id], h3[id], h4[id], h5[id], h6[id], dfn[id], emu-clause[id], \
     emu-annex[id], emu-production[id], tr[id], dt[id], section[id], li[id]";

pub const CALLOUT_CLASSES: [&str; 6] = ["note", "example", "warning", "XXX", "issue", "advisement"];

pub fn has_class(e: &ElementRef, c: &str) -> bool {
    e.value().classes().any(|x| x == c)
}

pub fn elname<'a>(n: &N<'a>) -> Option<&'a str> {
    n.value().as_element().map(|e| e.name())
}

pub fn heading_depth(tag: &str) -> Option<u8> {
    match tag {
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

pub fn is_algo_div(e: &ElementRef) -> bool {
    e.value().name() == "div"
        && (has_class(e, "algorithm") || e.value().attr("data-algorithm").is_some())
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

/// A parsed source document plus the lookups every invariant needs.
pub struct SourceDoc {
    pub doc: Html,
    pub spec: String,
    pub base_url: String,
    pub is_rfc: bool,
    anchors: HashMap<String, NodeId>,
    any_id: HashMap<String, NodeId>,
    order: HashMap<NodeId, usize>,
    nodes: Vec<NodeId>,
}

impl SourceDoc {
    pub fn new(html: &str, spec: &str, base_url: &str) -> Self {
        let doc = Html::parse_document(html);
        let is_rfc = doc.select(&sel("html.RFC")).next().is_some();
        let mut anchors = HashMap::new();
        for e in doc.select(&sel(ANCHOR_SELECTOR)) {
            if let Some(id) = e.value().attr("id") {
                anchors.entry(id.to_string()).or_insert(e.id());
            }
        }
        let mut any_id = HashMap::new();
        let mut order = HashMap::new();
        let mut nodes = Vec::new();
        for (i, n) in doc.root_element().descendants().enumerate() {
            order.insert(n.id(), i);
            nodes.push(n.id());
            if let Some(id) = n.value().as_element().and_then(|e| e.attr("id")) {
                any_id.entry(id.to_string()).or_insert(n.id());
            }
        }
        SourceDoc {
            doc,
            spec: spec.to_string(),
            base_url: base_url.to_string(),
            is_rfc,
            anchors,
            any_id,
            order,
            nodes,
        }
    }

    pub fn node(&self, id: NodeId) -> N<'_> {
        self.doc.tree.get(id).expect("node id from this document")
    }

    /// The element the indexer made a section from: first in document order among anchor tags.
    pub fn locate(&self, anchor: &str) -> Option<ElementRef<'_>> {
        self.anchors
            .get(anchor)
            .and_then(|id| ElementRef::wrap(self.node(*id)))
    }

    pub fn id_element(&self, id: &str) -> Option<ElementRef<'_>> {
        self.any_id
            .get(id)
            .and_then(|n| ElementRef::wrap(self.node(*n)))
    }

    pub fn pos(&self, n: &N) -> usize {
        self.order[&n.id()]
    }

    pub fn node_at(&self, pos: usize) -> N<'_> {
        self.node(self.nodes[pos])
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn abs(&self, href: &str) -> String {
        if href.starts_with('#') {
            format!("{}{href}", self.base_url)
        } else {
            href.to_string()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Heading,
    HeadingWrapped,
    RfcSection,
    EmuClause,
    Element,
    AlgoDiv,
    AlgoSibling,
    AlgoNone,
    DefItem,
    DefBlock,
    DefDfnOnly,
    IdlPre,
    IdlNoPre,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Heading => "heading",
            Shape::HeadingWrapped => "heading-wrapped",
            Shape::RfcSection => "rfc-section",
            Shape::EmuClause => "emu-clause",
            Shape::Element => "anchor-element",
            Shape::AlgoDiv => "algo-div",
            Shape::AlgoSibling => "algo-sibling",
            Shape::AlgoNone => "algo-none",
            Shape::DefItem => "def-item",
            Shape::DefBlock => "def-block",
            Shape::DefDfnOnly => "def-dfn-only",
            Shape::IdlPre => "idl-pre",
            Shape::IdlNoPre => "idl-no-pre",
        }
    }

    pub fn is_heading(self) -> bool {
        matches!(self, Shape::Heading | Shape::HeadingWrapped)
    }

    pub fn has_region(self) -> bool {
        self != Shape::AlgoNone
    }
}

/// The source nodes a section's content is supposed to cover (§5.5).
pub struct Region<'a> {
    pub roots: Vec<N<'a>>,
    pub shape: Shape,
    /// Nodes that stand for the section in a fixture: the region plus the anchor element where the
    /// region does not contain it, with `excluded` children left out.
    pub fixture_roots: Vec<N<'a>>,
    pub excluded: HashSet<NodeId>,
}

fn contains_heading_anchor(e: &ElementRef) -> bool {
    e.descendants()
        .filter_map(ElementRef::wrap)
        .any(|d| heading_depth(d.value().name()).is_some() && d.value().attr("id").is_some())
}

pub fn region<'a>(src: &SourceDoc, el: ElementRef<'a>, ty: SectionType) -> Region<'a> {
    let tag = el.value().name();
    let simple = |roots: Vec<N<'a>>, shape| Region {
        fixture_roots: roots.clone(),
        roots,
        shape,
        excluded: HashSet::new(),
    };

    if heading_depth(tag).is_some() {
        // ReSpec wraps each heading in div.header-wrapper with its self-link; the section's prose
        // follows the wrapper.
        let start = el
            .parent()
            .and_then(ElementRef::wrap)
            .filter(|p| p.value().name() == "div" && has_class(p, "header-wrapper"))
            .unwrap_or(el);
        let mut roots = Vec::new();
        let mut cur = start.next_sibling();
        while let Some(n) = cur {
            if let Some(e) = ElementRef::wrap(n) {
                let name = e.value().name();
                if heading_depth(name).is_some() || name == "h1" {
                    break;
                }
                if name == "dfn" && e.value().attr("id").is_some() {
                    break;
                }
                if contains_heading_anchor(&e) {
                    break;
                }
                roots.push(n);
            } else if n.value().is_text() {
                roots.push(n);
            }
            cur = n.next_sibling();
        }
        let mut fixture_roots = vec![*start];
        fixture_roots.extend(roots.iter().copied());
        return Region {
            roots,
            shape: if start.id() == el.id() {
                Shape::Heading
            } else {
                Shape::HeadingWrapped
            },
            fixture_roots,
            excluded: HashSet::new(),
        };
    }

    if src.is_rfc && tag == "section" {
        let mut excluded = HashSet::new();
        let roots = el
            .children()
            .filter(|n| match elname(n) {
                Some(t) if t == "section" || heading_depth(t).is_some() => {
                    if t == "section" {
                        excluded.insert(n.id());
                    }
                    false
                }
                _ => true,
            })
            .collect();
        return Region {
            roots,
            shape: Shape::RfcSection,
            fixture_roots: vec![*el],
            excluded,
        };
    }

    if tag == "emu-clause" || tag == "emu-annex" {
        let mut excluded = HashSet::new();
        let roots = el
            .children()
            .filter(|n| match elname(n) {
                Some("emu-clause" | "emu-annex") => {
                    excluded.insert(n.id());
                    false
                }
                Some("h1" | "emu-import") => false,
                Some(_) => true,
                None => false,
            })
            .collect();
        return Region {
            roots,
            shape: Shape::EmuClause,
            fixture_roots: vec![*el],
            excluded,
        };
    }

    if tag != "dfn" {
        return simple(vec![*el], Shape::Element);
    }

    let definition = || -> Region<'a> {
        for a in el.ancestors() {
            let Some(e) = ElementRef::wrap(a) else {
                continue;
            };
            if matches!(
                e.value().name(),
                "p" | "div" | "dd" | "dt" | "li" | "section"
            ) {
                if e.value().name() == "p" {
                    if let Some(item) = a.parent().and_then(ElementRef::wrap) {
                        if matches!(item.value().name(), "li" | "dd")
                            && item.select(&sel("dfn[id]")).count() == 1
                        {
                            return Region {
                                roots: item.children().collect(),
                                shape: Shape::DefItem,
                                fixture_roots: vec![*item],
                                excluded: HashSet::new(),
                            };
                        }
                    }
                }
                return simple(vec![a], Shape::DefBlock);
            }
        }
        simple(vec![*el], Shape::DefDfnOnly)
    };

    match ty {
        SectionType::Idl => {
            if let Some(pre) = el.ancestors().find(|a| elname(a) == Some("pre")) {
                return simple(vec![pre], Shape::IdlPre);
            }
            let mut r = definition();
            r.shape = Shape::IdlNoPre;
            r
        }
        SectionType::Algorithm => {
            if let Some(div) = el
                .ancestors()
                .filter_map(ElementRef::wrap)
                .find(|e| is_algo_div(e))
            {
                if div.select(&sel("dfn[id]")).next().map(|d| d.id()) == Some(el.id()) {
                    return simple(vec![*div], Shape::AlgoDiv);
                }
            }
            if let Some((block, list)) = sibling_list(el) {
                // A later algorithm of an algorithm div needs the div around it in a fixture: the
                // earlier dfn is what makes it a later one.
                let fixture_roots = match el
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .find(|e| is_algo_div(e))
                {
                    Some(div) => vec![*div],
                    None => vec![block, list],
                };
                return Region {
                    roots: vec![block, list],
                    shape: Shape::AlgoSibling,
                    fixture_roots,
                    excluded: HashSet::new(),
                };
            }
            let ctx = classification_context(el);
            Region {
                roots: Vec::new(),
                shape: Shape::AlgoNone,
                fixture_roots: ctx,
                excluded: HashSet::new(),
            }
        }
        _ => definition(),
    }
}

/// Everything a heading's section spans, subsections included: following siblings until a heading
/// of the same or higher level or a sibling `dfn[id]`.
pub fn heading_extent<'a>(el: ElementRef<'a>) -> Vec<N<'a>> {
    let Some(depth) = heading_depth(el.value().name()) else {
        return Vec::new();
    };
    let start = el
        .parent()
        .and_then(ElementRef::wrap)
        .filter(|p| p.value().name() == "div" && has_class(p, "header-wrapper"))
        .unwrap_or(el);
    let mut out = Vec::new();
    let mut cur = start.next_sibling();
    while let Some(n) = cur {
        if let Some(e) = ElementRef::wrap(n) {
            let name = e.value().name();
            if heading_depth(name).is_some_and(|d| d <= depth)
                || (name == "dfn" && e.value().attr("id").is_some())
            {
                break;
            }
            out.push(n);
        }
        cur = n.next_sibling();
    }
    out
}

/// The dfn's enclosing `p`/`dd`/`li` and its first following `ol`/`ul`/`dl` sibling, stopping at
/// `p`/`div`/`h*`.
pub fn sibling_list<'a>(el: ElementRef<'a>) -> Option<(N<'a>, N<'a>)> {
    for a in el.ancestors() {
        let Some(e) = ElementRef::wrap(a) else {
            continue;
        };
        if matches!(e.value().name(), "p" | "dd" | "li") {
            let mut sib = a.next_sibling();
            while let Some(s) = sib {
                match elname(&s) {
                    Some("ol" | "ul" | "dl") => return Some((a, s)),
                    Some("p" | "div" | "h2" | "h3" | "h4" | "h5" | "h6") => break,
                    _ => {}
                }
                sib = s.next_sibling();
            }
        }
    }
    None
}

/// What made the product classify a dfn as an algorithm: the enclosing algorithm div, else the
/// enclosing block plus its list sibling, else the dfn's parent.
pub fn classification_context<'a>(el: ElementRef<'a>) -> Vec<N<'a>> {
    if let Some(div) = el
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|e| is_algo_div(e))
    {
        return vec![*div];
    }
    for a in el.ancestors() {
        let Some(e) = ElementRef::wrap(a) else {
            continue;
        };
        if matches!(e.value().name(), "p" | "div" | "dd" | "li") {
            let mut sib = a.next_sibling();
            while let Some(s) = sib {
                match elname(&s) {
                    Some("ol" | "ul" | "dl") => return vec![a, s],
                    Some("p" | "div" | "h2" | "h3" | "h4" | "h5" | "h6") => break,
                    _ => {}
                }
                sib = s.next_sibling();
            }
        }
    }
    vec![el.parent().unwrap_or(*el)]
}

/// T1's classification rule (§6): a dfn is an algorithm's subject when it is the first `dfn[id]` of
/// an algorithm div, or its enclosing block's next list sibling is an `ol`.
pub fn is_algorithm_subject(el: ElementRef) -> bool {
    if let Some(div) = el
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|e| is_algo_div(e))
    {
        if div.select(&sel("dfn[id]")).next().map(|d| d.id()) == Some(el.id()) {
            return true;
        }
    }
    matches!(sibling_list(el), Some((_, list)) if elname(&list) == Some("ol"))
}

// ── Link and text classification (§5.6) ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Normal,
    InNote,
    InPre,
    InDel,
    InDropSpan,
    SelfLink,
    Biblio,
}

impl Cat {
    /// Expected as a markdown link in full-link content.
    pub fn expected_link(self) -> bool {
        matches!(self, Cat::Normal | Cat::InNote)
    }
    /// Counts as a reference-bearing link (R1/R2, S3).
    pub fn is_ref_link(self) -> bool {
        !matches!(self, Cat::SelfLink | Cat::Biblio)
    }
}

#[derive(Clone, Copy, Default)]
struct WalkCtx {
    note: bool,
    pre: bool,
    del: bool,
    dropspan: bool,
    selflink: bool,
}

pub struct SrcLink<'a> {
    pub href: String,
    pub abs: String,
    pub cat: Cat,
    pub node: N<'a>,
}

pub struct SrcText<'a> {
    pub links: Vec<SrcLink<'a>>,
    pub text: String,
    /// (byte offset into `text`, element containing the text node starting there)
    spans: Vec<(usize, N<'a>)>,
}

pub fn is_callout(e: &ElementRef) -> bool {
    e.value().name() == "emu-note"
        || (matches!(e.value().name(), "div" | "dd" | "p")
            && CALLOUT_CLASSES.iter().any(|c| has_class(e, c)))
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
            | "thead"
            | "tbody"
            | "tfoot"
            | "section"
            | "aside"
            | "blockquote"
            | "pre"
            | "figure"
            | "figcaption"
            | "br"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "hr"
            | "details"
            | "summary"
            | "caption"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "article"
            | "address"
            | "center"
            | "emu-note"
            | "emu-alg"
            | "emu-table"
            | "emu-figure"
            | "emu-example"
            | "emu-grammar"
            | "emu-production"
            | "emu-rhs"
    )
}

/// Classify every link and collect the visible text of `roots`.
pub fn walk<'a>(src: &SourceDoc, roots: &[N<'a>]) -> SrcText<'a> {
    let mut out = SrcText {
        links: Vec::new(),
        text: String::new(),
        spans: Vec::new(),
    };
    for r in roots {
        walk_node(src, *r, WalkCtx::default(), &mut out);
    }
    out
}

fn walk_node<'a>(src: &SourceDoc, n: N<'a>, ctx: WalkCtx, out: &mut SrcText<'a>) {
    match n.value() {
        Node::Text(t) => {
            if !(ctx.del || ctx.dropspan || ctx.selflink) {
                if let Some(p) = n.parent() {
                    out.spans.push((out.text.len(), p));
                }
                out.text.push_str(t);
            }
        }
        Node::Element(_) => {
            let e = ElementRef::wrap(n).expect("element");
            let name = e.value().name();
            // Form controls are page UI (MDN panels, example widgets), not spec prose.
            if matches!(
                name,
                "script" | "style" | "template" | "button" | "select" | "textarea"
            ) {
                return;
            }
            let mut c = ctx;
            c.note |= is_callout(&e);
            c.pre |= name == "pre";
            c.del |= name == "del";
            let emu_note_label = has_class(&e, "note")
                && n.parent()
                    .and_then(|p| elname(&p))
                    .is_some_and(|p| p == "emu-note");
            c.dropspan |= name == "span"
                && (has_class(&e, "secno") || has_class(&e, "secnum") || emu_note_label);
            if name == "a" {
                if let Some(h) = e.value().attr("href") {
                    let cat = if has_class(&e, "self-link") {
                        Cat::SelfLink
                    } else if e.value().attr("data-link-type") == Some("biblio") {
                        Cat::Biblio
                    } else if ctx.del {
                        Cat::InDel
                    } else if ctx.dropspan {
                        Cat::InDropSpan
                    } else if ctx.pre {
                        Cat::InPre
                    } else if ctx.note {
                        Cat::InNote
                    } else {
                        Cat::Normal
                    };
                    out.links.push(SrcLink {
                        href: h.to_string(),
                        abs: src.abs(h),
                        cat,
                        node: n,
                    });
                }
                c.selflink |= has_class(&e, "self-link");
            }
            let block = is_block(name);
            if block {
                out.text.push(' ');
            }
            for ch in n.children() {
                walk_node(src, ch, c, out);
            }
            if block {
                out.text.push(' ');
            }
        }
        _ => {}
    }
}

impl<'a> SrcText<'a> {
    pub fn expected_links(&self) -> Vec<String> {
        self.links
            .iter()
            .filter(|l| l.cat.expected_link())
            .map(|l| l.abs.clone())
            .collect()
    }

    /// Words with the element containing the text node each word starts in.
    pub fn words_with_nodes(&self) -> Vec<(String, Option<N<'a>>)> {
        let mut out = Vec::new();
        let mut start = None;
        let text = &self.text;
        let push = |s: usize, e: usize, out: &mut Vec<(String, Option<N<'a>>)>| {
            let w = alnum(&text[s..e]);
            if !w.is_empty() {
                let idx = self.spans.partition_point(|(off, _)| *off <= s);
                let node = idx.checked_sub(1).map(|i| self.spans[i].1);
                out.push((w, node));
            }
        };
        for (i, ch) in text.char_indices() {
            if ch.is_whitespace() {
                if let Some(s) = start.take() {
                    push(s, i, &mut out);
                }
            } else if start.is_none() {
                start = Some(i);
            }
        }
        if let Some(s) = start {
            push(s, text.len(), &mut out);
        }
        out
    }
}

pub fn alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn words(t: &str) -> Vec<String> {
    t.split_whitespace()
        .map(alnum)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Multiset difference `src - got`, sorted.
pub fn missing(src: &[String], got: &[String]) -> Vec<String> {
    let mut have: HashMap<&str, i64> = HashMap::new();
    for g in got {
        *have.entry(g.as_str()).or_default() += 1;
    }
    let mut out = Vec::new();
    for s in src {
        let e = have.entry(s.as_str()).or_default();
        if *e > 0 {
            *e -= 1;
        } else {
            out.push(s.clone());
        }
    }
    out.sort();
    out
}

/// Positions in `src` of the items `missing` (a sub-multiset of `src`) reports. An order-aware diff
/// against `got` picks the occurrences that were actually dropped; a repeated item is otherwise
/// ambiguous.
pub fn lost_positions(src: &[String], got: &[String], missing: &[String]) -> Vec<usize> {
    let ops = similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        src,
        got,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
    );
    let mut deleted = Vec::new();
    for op in ops {
        if let similar::DiffOp::Delete {
            old_index, old_len, ..
        }
        | similar::DiffOp::Replace {
            old_index, old_len, ..
        } = op
        {
            deleted.extend(old_index..old_index + old_len);
        }
    }
    let mut used = HashSet::new();
    let mut out = Vec::new();
    for m in missing {
        let pick = deleted
            .iter()
            .copied()
            .find(|i| &src[*i] == m && !used.contains(i))
            .or_else(|| {
                (0..src.len())
                    .rev()
                    .find(|i| &src[*i] == m && !used.contains(i))
            });
        if let Some(i) = pick {
            used.insert(i);
            out.push(i);
        }
    }
    out.sort();
    out
}

// ── Markdown side ───────────────────────────────────────────────────────────────────────────────

pub const NOTE_LABELS: [&str; 4] = ["Note:", "Example:", "Warning:", "Issue:"];

#[derive(Default, Debug)]
pub struct Md {
    pub links: Vec<String>,
    /// Links inside labelled note blockquotes.
    pub note_links: Vec<String>,
    pub text: String,
    pub ol_items: usize,
    pub ol_depth: usize,
}

pub fn md_info(md: &str) -> Md {
    let mut out = Md::default();
    let mut list_stack: Vec<bool> = Vec::new();
    let mut bq_is_note: Vec<bool> = Vec::new();
    let mut bq_first_text = false;
    for ev in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => {
                bq_is_note.push(false);
                bq_first_text = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                bq_is_note.pop();
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                if bq_is_note.iter().any(|x| *x) {
                    out.note_links.push(dest_url.to_string());
                }
                out.links.push(dest_url.to_string());
            }
            Event::Start(Tag::List(start)) => list_stack.push(start.is_some()),
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                out.text.push(' ');
                if list_stack.last() == Some(&true) {
                    out.ol_items += 1;
                    out.ol_depth = out.ol_depth.max(list_stack.iter().filter(|o| **o).count());
                }
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                if !bq_is_note.is_empty() && bq_first_text {
                    bq_first_text = false;
                    if NOTE_LABELS.contains(&t.trim()) {
                        *bq_is_note.last_mut().expect("inside blockquote") = true;
                        continue;
                    }
                }
                out.text.push_str(&t);
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::CodeBlock,
            ) => out.text.push(' '),
            Event::Start(Tag::Paragraph | Tag::TableCell) => out.text.push(' '),
            Event::SoftBreak | Event::HardBreak => out.text.push(' '),
            _ => {}
        }
    }
    out
}

// ── Steps ───────────────────────────────────────────────────────────────────────────────────────

/// Top-level `ol`s of a region: `ol` elements with no `ol` ancestor inside the region, outside
/// `pre` and `del`.
pub fn body_ols<'a>(roots: &[N<'a>]) -> Vec<N<'a>> {
    fn rec<'a>(n: N<'a>, out: &mut Vec<N<'a>>) {
        match elname(&n) {
            Some("ol") => out.push(n),
            Some("pre" | "del" | "script" | "style") => {}
            Some(_) => n.children().for_each(|c| rec(c, out)),
            None => {}
        }
    }
    let mut out = Vec::new();
    for r in roots {
        rec(*r, &mut out);
    }
    out
}

/// Numbered steps under an `ol`: every `li` whose parent is an `ol`, and the maximum `ol` nesting.
pub fn ol_steps(ol: N) -> (usize, usize) {
    fn rec(n: N, depth: usize, count: &mut usize, maxd: &mut usize) {
        for ch in n.children() {
            if let Some(name) = elname(&ch) {
                if matches!(name, "pre" | "del") {
                    continue;
                }
                if name == "li" && elname(&n) == Some("ol") {
                    *count += 1;
                    *maxd = (*maxd).max(depth);
                }
                let nd = if name == "ol" { depth + 1 } else { depth };
                rec(ch, nd, count, maxd);
            }
        }
    }
    let (mut c, mut d) = (0, 0);
    rec(ol, 1, &mut c, &mut d);
    (c, d)
}

pub fn steps_of(ols: &[N]) -> (usize, usize) {
    ols.iter()
        .map(|o| ol_steps(*o))
        .fold((0, 0), |(c, d), (c2, d2)| (c + c2, d.max(d2)))
}

// ── IR links ────────────────────────────────────────────────────────────────────────────────────

/// Every `LinkSpan`-shaped object (`id`, `href`, `span`) anywhere in a serialized algorithm,
/// deduplicated by link id.
pub fn ir_link_hrefs(v: &serde_json::Value) -> Vec<String> {
    fn rec(v: &serde_json::Value, seen: &mut HashSet<String>, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                if let (Some(id), Some(href), Some(_)) = (
                    m.get("id").and_then(|x| x.as_str()),
                    m.get("href").and_then(|x| x.as_str()),
                    m.get("span"),
                ) {
                    if seen.insert(id.to_string()) {
                        out.push(href.to_string());
                    }
                }
                m.values().for_each(|x| rec(x, seen, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| rec(x, seen, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    rec(v, &mut HashSet::new(), &mut out);
    out
}

// ── Construct paths (§9) ────────────────────────────────────────────────────────────────────────

/// Construct labels of inline markup, dropped by `SectionCtx::block_construct`.
pub const INLINE_LABELS: [&str; 12] = [
    "a", "code", "var", "dfn", "sup", "sub", "kbd", "samp", "s", "del", "ins", "img",
];

const INLINE_EMU: [&str; 10] = [
    "emu-xref",
    "emu-val",
    "emu-const",
    "emu-nt",
    "emu-t",
    "emu-eqn",
    "emu-rhs",
    "emu-geq",
    "emu-mods",
    "emu-opt",
];

fn construct_label(e: &ElementRef) -> Option<String> {
    let name = e.value().name();
    let classed = |base: &str, classes: &[&str]| {
        classes
            .iter()
            .find(|c| has_class(e, c))
            .map(|c| format!("{base}.{c}"))
    };
    match name {
        "a" | "ol" | "ul" | "table" | "th" | "td" | "dt" | "dd" | "code" | "var" | "pre"
        | "blockquote" | "details" | "summary" | "figure" | "figcaption" | "caption" | "math"
        | "svg" | "del" | "ins" | "sup" | "sub" | "dfn" | "h1" | "h2" | "h3" | "h4" | "h5"
        | "h6" | "iframe" | "img" | "object" | "button" | "select" | "textarea" | "input"
        | "kbd" | "samp" | "s" | "noscript" => Some(name.to_string()),
        "dl" => Some(
            classed("dl", &["switch", "props", "domintro", "def"]).unwrap_or_else(|| "dl".into()),
        ),
        "span" => classed(name, &["note", "secno", "secnum"]),
        "p" | "div" | "aside" | "section" => {
            if name == "div" && is_algo_div(e) {
                return Some("div.algorithm".into());
            }
            if CALLOUT_CLASSES.iter().any(|c| has_class(e, c)) {
                return Some("callout".into());
            }
            classed(name, &["header-wrapper", "idl", "domintro"])
        }
        n if n.starts_with("emu-") && !INLINE_EMU.contains(&n) => Some(n.to_string()),
        _ => None,
    }
}

/// Tag chain from `node` up to (and including) the region root, structural vocabulary only.
pub fn construct_path(node: N, region_roots: &HashSet<NodeId>) -> String {
    let mut labels = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        if let Some(e) = ElementRef::wrap(n) {
            if let Some(l) = construct_label(&e) {
                labels.push(l);
            }
        }
        if region_roots.contains(&n.id()) {
            break;
        }
        cur = n.parent();
    }
    labels.reverse();
    labels.dedup();
    let start = labels.len().saturating_sub(4);
    let path = labels[start..].join(">");
    if path.is_empty() {
        "text".into()
    } else {
        path
    }
}

// ── Serialization for findings and fixtures ─────────────────────────────────────────────────────

const VOID: [&str; 13] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

fn escape_text(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}

fn open_tag(e: &scraper::node::Element, out: &mut String) {
    out.push('<');
    out.push_str(e.name());
    for (k, v) in e.attrs() {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        for ch in v.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out.push('>');
}

pub fn serialize(n: N, excluded: &HashSet<NodeId>, out: &mut String) {
    if excluded.contains(&n.id()) {
        return;
    }
    match n.value() {
        Node::Text(t) => {
            let raw = n
                .parent()
                .and_then(|p| elname(&p))
                .is_some_and(|p| matches!(p, "script" | "style"));
            if raw {
                out.push_str(t);
            } else {
                escape_text(t, out);
            }
        }
        Node::Element(e) => {
            open_tag(e, out);
            if VOID.contains(&e.name()) {
                return;
            }
            for c in n.children() {
                serialize(c, excluded, out);
            }
            out.push_str("</");
            out.push_str(e.name());
            out.push('>');
        }
        _ => {}
    }
}

/// A standalone document holding `roots` inside shallow copies of their ancestors, so that
/// ancestor-dependent classification (algorithm divs, `emu-clause`, `html.RFC`) is preserved.
pub fn fixture_html(roots: &[N], excluded: &HashSet<NodeId>) -> String {
    let Some(first) = roots.first() else {
        return String::new();
    };
    let ancestors: Vec<N> = first
        .ancestors()
        .filter(|a| a.value().is_element())
        .collect();
    let mut out = String::from("<!DOCTYPE html>\n");
    for a in ancestors.iter().rev() {
        open_tag(a.value().as_element().expect("element"), &mut out);
        out.push('\n');
    }
    for r in roots {
        serialize(*r, excluded, &mut out);
        out.push('\n');
    }
    for a in &ancestors {
        out.push_str("</");
        out.push_str(a.value().as_element().expect("element").name());
        out.push_str(">\n");
    }
    out
}

pub fn excerpt(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut b = max;
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    format!("{}…", &s[..b])
}

/// A short excerpt of rendered markdown around the first occurrence of `needle`.
pub fn rendered_excerpt(content: &str, needle: &str) -> Option<String> {
    let i = content.find(needle)?;
    let mut s = i.saturating_sub(80);
    while !content.is_char_boundary(s) {
        s -= 1;
    }
    let mut e = (i + needle.len() + 80).min(content.len());
    while !content.is_char_boundary(e) {
        e += 1;
    }
    Some(content[s..e].to_string())
}

pub fn outer_html_excerpt(n: N, max: usize) -> String {
    let mut s = String::new();
    serialize(n, &HashSet::new(), &mut s);
    excerpt(&s, max)
}
