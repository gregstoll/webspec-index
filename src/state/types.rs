//! IDL type collection (§6.2): builds a `TypeTable` from IDL definitions and concept dfns.
#![allow(dead_code)]
use crate::model::ParsedIdlDefinition;
use crate::parse::idl::extract_idl_text;
use crate::state::model::{
    AnchorRole, AnchorTarget, ConceptAliasBasis, SuperBasis, SuperEdge, TypeAnchor, TypeDef,
    TypeKey, TypeKind,
};
use ego_tree::NodeRef;
use regex::Regex;
use scraper::{ElementRef, Html, Node, Selector};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

/// A concept dfn that may alias an IDL type. Produced by `declare.rs::concept_dfns`.
pub(crate) struct ConceptDfn {
    pub id: String,
    pub name: String,
    pub names: Vec<String>,
}

/// All type information extracted from one spec snapshot (§6.2).
#[derive(Default)]
pub(crate) struct TypeTable {
    pub types: BTreeMap<TypeKey, TypeDef>,
    /// Maps local anchor → type key (same-spec anchors only).
    pub by_anchor: HashMap<String, TypeKey>,
    /// Every IDL name seen in this spec (used for concept-alias matching).
    pub idl_names: HashSet<String>,
}

fn kind_priority(k: &TypeKind) -> u8 {
    match k {
        TypeKind::IdlMixin | TypeKind::IdlDictionary => 3,
        TypeKind::IdlInterface => 2,
        TypeKind::Element => 1,
        TypeKind::InfraStruct => 1,
        TypeKind::Concept => 0,
    }
}

impl TypeTable {
    /// Ensure an IDL type entry exists for `name`; never downgrade to a lower-priority kind.
    fn ensure_idl(&mut self, name: &str, kind: TypeKind) -> TypeKey {
        let key = TypeKey::Idl(name.to_string());
        self.idl_names.insert(name.to_string());
        let entry = self.types.entry(key.clone()).or_insert_with(|| TypeDef {
            key: key.clone(),
            name: name.to_string(),
            kind: kind.clone(),
            anchors: Vec::new(),
            supertypes: Vec::new(),
        });
        if kind_priority(&kind) > kind_priority(&entry.kind) {
            entry.kind = kind;
        }
        key
    }

    /// Push a `TypeAnchor` if the anchor is not already present; record in `by_anchor`.
    fn add_anchor(&mut self, key: TypeKey, spec: &str, anchor: &str, role: AnchorRole) {
        let target = AnchorTarget {
            spec: spec.to_string(),
            anchor: anchor.to_string(),
        };
        let type_def = self
            .types
            .get_mut(&key)
            .expect("key must exist before add_anchor");
        let already = type_def
            .anchors
            .iter()
            .any(|a| a.target.anchor == anchor && a.target.spec == spec);
        if !already {
            type_def.anchors.push(TypeAnchor { target, role });
        }
        self.by_anchor.insert(anchor.to_string(), key);
    }

    /// Add a supertype edge if not already present (dedup by target + basis).
    fn add_supertype(&mut self, key: &TypeKey, edge: SuperEdge) {
        if let Some(type_def) = self.types.get_mut(key) {
            if !type_def
                .supertypes
                .iter()
                .any(|e| e.target == edge.target && e.basis == edge.basis)
            {
                type_def.supertypes.push(edge);
            }
        }
    }

    /// Scan IDL text for interface/dictionary declarations and `NAME includes MIXIN` statements.
    fn scan_idl_text(&mut self, text: &str) {
        static IFACE_RE: OnceLock<Regex> = OnceLock::new();
        let iface_re = IFACE_RE.get_or_init(|| {
            Regex::new(r"(?m)\b(?:partial\s+)?interface\s+(mixin\s+)?([A-Za-z_]\w*)\s*(?::\s*([A-Za-z_]\w*))?\s*\{").unwrap()
        });
        static DICT_RE: OnceLock<Regex> = OnceLock::new();
        let dict_re = DICT_RE.get_or_init(|| {
            Regex::new(r"(?m)\bdictionary\s+([A-Za-z_]\w*)\s*(?::\s*([A-Za-z_]\w*))?\s*\{").unwrap()
        });
        static INCLUDES_RE: OnceLock<Regex> = OnceLock::new();
        let includes_re = INCLUDES_RE.get_or_init(|| {
            Regex::new(r"(?m)\b([A-Za-z_]\w*)\s+includes\s+([A-Za-z_]\w*)\s*;").unwrap()
        });

        for cap in iface_re.captures_iter(text) {
            let is_mixin = cap.get(1).is_some();
            let name = cap[2].to_string();
            let kind = if is_mixin {
                TypeKind::IdlMixin
            } else {
                TypeKind::IdlInterface
            };
            let key = self.ensure_idl(&name, kind);
            if let Some(parent) = cap.get(3).map(|m| m.as_str().to_string()) {
                self.ensure_idl(&parent, TypeKind::IdlInterface);
                let parent_key = TypeKey::Idl(parent);
                self.add_supertype(
                    &key,
                    SuperEdge {
                        target: parent_key,
                        basis: SuperBasis::IdlInheritance,
                    },
                );
            }
        }

        for cap in dict_re.captures_iter(text) {
            let name = cap[1].to_string();
            let key = self.ensure_idl(&name, TypeKind::IdlDictionary);
            if let Some(parent) = cap.get(2).map(|m| m.as_str().to_string()) {
                self.ensure_idl(&parent, TypeKind::IdlDictionary);
                let parent_key = TypeKey::Idl(parent);
                self.add_supertype(
                    &key,
                    SuperEdge {
                        target: parent_key,
                        basis: SuperBasis::IdlInheritance,
                    },
                );
            }
        }

        for cap in includes_re.captures_iter(text) {
            let name = cap[1].to_string();
            let mixin = cap[2].to_string();
            let key = self.ensure_idl(&name, TypeKind::IdlInterface);
            self.ensure_idl(&mixin, TypeKind::IdlInterface);
            let mixin_key = TypeKey::Idl(mixin);
            self.add_supertype(
                &key,
                SuperEdge {
                    target: mixin_key,
                    basis: SuperBasis::IdlIncludes,
                },
            );
        }
    }

    /// Add `ConceptAlias(NameMatch)` anchors for concept dfns one of whose normalized names
    /// (text or `data-lt`, a trailing plural `s` tolerated) matches an IDL interface name in the
    /// same spec.
    fn add_concept_aliases(&mut self, spec: &str, concept_dfns: &[ConceptDfn]) {
        let by_normalized: HashMap<String, String> = self
            .idl_names
            .iter()
            .map(|n| (n.to_lowercase(), n.clone()))
            .collect();
        for dfn in concept_dfns {
            let matched = dfn.names.iter().find_map(|name| {
                let normalized = name.to_lowercase().replace(' ', "");
                by_normalized.get(&normalized).or_else(|| {
                    normalized
                        .strip_suffix('s')
                        .and_then(|singular| by_normalized.get(singular))
                })
            });
            if let Some(idl_name) = matched.cloned() {
                let key = TypeKey::Idl(idl_name.clone());
                self.ensure_idl(&idl_name, TypeKind::IdlInterface);
                self.add_anchor(
                    key,
                    spec,
                    &dfn.id,
                    AnchorRole::ConceptAlias(ConceptAliasBasis::NameMatch),
                );
            }
        }
    }

    /// Explicitly associate a concept dfn with a type (`Evidence` basis).
    ///
    /// If `name` (normalized: lowercase, spaces removed) matches an IDL name in this spec,
    /// adds a `ConceptAlias(Evidence)` anchor to that IDL type. Otherwise creates a new
    /// `TypeKey::Anchor` type with a `Defining` anchor.
    pub fn add_concept(&mut self, spec: &str, anchor: &str, name: &str, kind: TypeKind) -> TypeKey {
        let normalized = name.to_lowercase().replace(' ', "");
        let idl_name = self
            .idl_names
            .iter()
            .find(|n| n.to_lowercase() == normalized)
            .cloned();
        if let Some(idl_name) = idl_name {
            let key = TypeKey::Idl(idl_name);
            self.add_anchor(
                key.clone(),
                spec,
                anchor,
                AnchorRole::ConceptAlias(ConceptAliasBasis::Evidence),
            );
            key
        } else {
            let key = TypeKey::Anchor(AnchorTarget {
                spec: spec.to_string(),
                anchor: anchor.to_string(),
            });
            let entry = self.types.entry(key.clone()).or_insert_with(|| TypeDef {
                key: key.clone(),
                name: name.to_string(),
                kind: kind.clone(),
                anchors: Vec::new(),
                supertypes: Vec::new(),
            });
            if kind_priority(&kind) > kind_priority(&entry.kind) {
                entry.kind = kind;
            }
            self.add_anchor(key.clone(), spec, anchor, AnchorRole::Defining);
            key
        }
    }

    /// Record evidence (a `data-dfn-for` that resolves by IDL name) that the concept dfn `anchor`
    /// is the existing type `key`: upgrades a `NameMatch` alias, or replaces the `Concept` type
    /// keyed by the anchor itself. Returns false when the anchor belongs to some other type.
    pub fn alias_concept(&mut self, key: &TypeKey, spec: &str, anchor: &str) -> bool {
        let target = AnchorTarget {
            spec: spec.to_string(),
            anchor: anchor.to_string(),
        };
        let own = TypeKey::Anchor(target.clone());
        match self.by_anchor.get(anchor) {
            Some(current) if current == key => {}
            Some(current)
                if *current == own
                    && self.types.get(&own).map(|t| &t.kind) == Some(&TypeKind::Concept) =>
            {
                self.types.remove(&own);
            }
            Some(_) => return false,
            None => {}
        }
        let Some(type_def) = self.types.get_mut(key) else {
            return false;
        };
        let role = AnchorRole::ConceptAlias(ConceptAliasBasis::Evidence);
        match type_def
            .anchors
            .iter_mut()
            .find(|a| a.target.spec == spec && a.target.anchor == anchor)
        {
            Some(existing) => existing.role = role,
            None => type_def.anchors.push(TypeAnchor { target, role }),
        }
        self.by_anchor.insert(anchor.to_string(), key.clone());
        true
    }

    /// Walk `dl.element` blocks, find the nearest preceding heading, and add an
    /// `ElementDefinition` anchor from the heading id to the element's IDL interface type.
    fn add_element_definitions(&mut self, document: &Html, spec: &str, _base_url: &str) {
        static DL_SEL: OnceLock<Selector> = OnceLock::new();
        let dl_sel = DL_SEL.get_or_init(|| Selector::parse("dl.element").unwrap());

        for dl in document.select(dl_sel) {
            let heading_id = preceding_heading_id(&dl);
            let Some(heading_id) = heading_id else {
                continue;
            };
            let Some(interface_name) = element_interface_name(&dl) else {
                continue;
            };
            let key = self.ensure_idl(&interface_name, TypeKind::Element);
            self.add_anchor(key, spec, &heading_id, AnchorRole::ElementDefinition);
        }
    }
}

/// Collect `pre` and `code` elements with class `idl`, skipping `code.idl` nested inside
/// a `pre.idl` (they are the same block).
fn idl_blocks(document: &Html) -> Vec<ElementRef<'_>> {
    static SEL: OnceLock<Selector> = OnceLock::new();
    let sel = SEL.get_or_init(|| Selector::parse("pre, code").unwrap());
    let mut out = Vec::new();
    for el in document.select(sel) {
        if !el.value().classes().any(|c| c == "idl") {
            continue;
        }
        if el.value().name() == "code" {
            let inside_pre_idl = el
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|a| a.value().name() == "pre" && a.value().classes().any(|c| c == "idl"));
            if inside_pre_idl {
                continue;
            }
        }
        out.push(el);
    }
    out
}

/// Walk descendants of an IDL block; return `(interface_name, anchor)` for each Wattsi-style
/// `dfn[id]` (without `data-dfn-type`) that follows the text `partial interface [mixin]`.
fn wattsi_partials(block: &ElementRef<'_>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut buf = String::new();
    walk_for_partials(**block, &mut buf, &mut out);
    out
}

fn walk_for_partials(node: NodeRef<'_, Node>, buf: &mut String, out: &mut Vec<(String, String)>) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => buf.push_str(t),
            Node::Element(e) => {
                if e.name() == "dfn" && e.attr("id").is_some() && e.attr("data-dfn-type").is_none()
                {
                    let words: Vec<&str> = buf.split_whitespace().collect();
                    let is_partial_iface = words.ends_with(&["partial", "interface"])
                        || words.ends_with(&["partial", "interface", "mixin"]);
                    if is_partial_iface {
                        let el = ElementRef::wrap(child).unwrap();
                        let name = el
                            .text()
                            .collect::<String>()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let id = e.attr("id").unwrap().to_string();
                        out.push((name, id));
                    }
                }
                walk_for_partials(child, buf, out);
            }
            _ => {}
        }
    }
}

/// Find the id of the nearest preceding `h2`–`h6` sibling.
fn preceding_heading_id(el: &ElementRef<'_>) -> Option<String> {
    let mut sib = el.prev_sibling();
    while let Some(node) = sib {
        if let Some(sibling) = ElementRef::wrap(node) {
            match sibling.value().name() {
                "h2" | "h3" | "h4" | "h5" | "h6" => {
                    return sibling.value().attr("id").map(str::to_string);
                }
                _ => {}
            }
        }
        sib = node.prev_sibling();
    }
    None
}

/// Extract the IDL interface name from a `dl.element`'s "DOM interface:" dd.
/// Tries IDL text first (`interface NAME`), then first `<a>` text matching `^[A-Z]\w*$`.
fn element_interface_name(dl: &ElementRef<'_>) -> Option<String> {
    static LINK_SEL: OnceLock<Selector> = OnceLock::new();
    let link_sel = LINK_SEL.get_or_init(|| Selector::parse("a").unwrap());
    static IFACE_RE: OnceLock<Regex> = OnceLock::new();
    let iface_re = IFACE_RE.get_or_init(|| Regex::new(r"\binterface\s+([A-Za-z_]\w*)").unwrap());
    static NAME_RE: OnceLock<Regex> = OnceLock::new();
    let name_re = NAME_RE.get_or_init(|| Regex::new(r"^[A-Z]\w*$").unwrap());

    let mut dom_iface_dt = false;
    for child in dl.children().filter_map(ElementRef::wrap) {
        match child.value().name() {
            "dt" => {
                dom_iface_dt = child.text().collect::<String>().contains("DOM interface");
            }
            "dd" if dom_iface_dt => {
                // Try IDL blocks first
                for block in idl_blocks_in(&child) {
                    let text = extract_idl_text(&block);
                    if let Some(cap) = iface_re.captures(&text) {
                        return Some(cap[1].to_string());
                    }
                }
                // Fall back to first PascalCase link text
                for link in child.select(link_sel) {
                    let text: String = link
                        .text()
                        .collect::<String>()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if name_re.is_match(&text) {
                        return Some(text);
                    }
                }
                return None;
            }
            _ => {}
        }
    }
    None
}

/// Collect `pre` and `code` elements with class `idl` within a subtree, skipping nested ones.
fn idl_blocks_in<'a>(root: &'a ElementRef<'a>) -> Vec<ElementRef<'a>> {
    static SEL: OnceLock<Selector> = OnceLock::new();
    let sel = SEL.get_or_init(|| Selector::parse("pre, code").unwrap());
    let mut out = Vec::new();
    for el in root.select(sel) {
        if !el.value().classes().any(|c| c == "idl") {
            continue;
        }
        if el.value().name() == "code" {
            let inside_pre_idl = el
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|a| a.value().name() == "pre" && a.value().classes().any(|c| c == "idl"));
            if inside_pre_idl {
                continue;
            }
        }
        out.push(el);
    }
    out
}

/// Build a `TypeTable` from IDL definitions, IDL block text, concept dfns, and element definitions.
pub(crate) fn collect_types(
    document: &Html,
    spec: &str,
    base_url: &str,
    idl: &[ParsedIdlDefinition],
    concept_dfns: &[ConceptDfn],
) -> TypeTable {
    let mut table = TypeTable::default();

    // Seed from parsed IDL definitions
    for def in idl.iter().filter(|d| {
        matches!(
            d.kind.as_str(),
            "interface" | "dictionary" | "namespace" | "callback"
        )
    }) {
        let text = def.idl_text.as_deref().unwrap_or_default();
        let kind = if def.kind == "dictionary" {
            TypeKind::IdlDictionary
        } else if def.kind == "namespace" {
            TypeKind::IdlInterface
        } else if text.contains(&format!("mixin {}", def.name)) {
            TypeKind::IdlMixin
        } else {
            TypeKind::IdlInterface
        };
        table.ensure_idl(&def.name, kind);
        table.add_anchor(
            TypeKey::Idl(def.name.clone()),
            spec,
            &def.anchor,
            AnchorRole::Defining,
        );
    }

    // Scan all IDL blocks for inheritance, includes, and Wattsi partials
    for block in idl_blocks(document) {
        let text = extract_idl_text(&block);
        table.scan_idl_text(&text);
        for (name, anchor) in wattsi_partials(&block) {
            table.ensure_idl(&name, TypeKind::IdlInterface);
            table.add_anchor(TypeKey::Idl(name), spec, &anchor, AnchorRole::Partial);
        }
    }

    // Concept aliases (NameMatch)
    table.add_concept_aliases(spec, concept_dfns);

    // Element definition anchors
    table.add_element_definitions(document, spec, base_url);

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ParsedIdlDefinition;
    use scraper::Html;

    fn idl_def(anchor: &str, name: &str, kind: &str, text: &str) -> ParsedIdlDefinition {
        ParsedIdlDefinition {
            anchor: anchor.into(),
            name: name.into(),
            owner: None,
            kind: kind.into(),
            canonical_name: name.into(),
            idl_text: Some(text.into()),
        }
    }

    #[test]
    fn bikeshed_interfaces_inheritance_includes_and_aliases() {
        let html = r##"<pre class="def highlight idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> : EventTarget {};</pre>
        <pre class="def highlight idl">interface <dfn data-dfn-type="interface" id="interface-element">Element</dfn> : Node {};
        <a href="#interface-document">Document</a> includes <a href="#nonelementparentnode">NonElementParentNode</a>;</pre>
        <p>A <dfn id="concept-node">node</dfn> is …</p>"##;
        let doc = Html::parse_document(html);
        let idl = vec![
            idl_def(
                "interface-node",
                "Node",
                "interface",
                "interface Node : EventTarget {};",
            ),
            idl_def(
                "interface-element",
                "Element",
                "interface",
                "interface Element : Node {};",
            ),
        ];
        let concepts = vec![ConceptDfn {
            id: "concept-node".into(),
            name: "node".into(),
            names: vec!["node".into()],
        }];
        let t = collect_types(&doc, "DOM", "https://dom.spec.whatwg.org/", &idl, &concepts);

        let element = &t.types[&TypeKey::Idl("Element".into())];
        assert_eq!(
            element.supertypes[0],
            SuperEdge {
                target: TypeKey::Idl("Node".into()),
                basis: SuperBasis::IdlInheritance
            }
        );

        let document = &t.types[&TypeKey::Idl("Document".into())];
        assert!(document
            .supertypes
            .iter()
            .any(|e| e.basis == SuperBasis::IdlIncludes
                && e.target == TypeKey::Idl("NonElementParentNode".into())));

        assert_eq!(t.by_anchor["concept-node"], TypeKey::Idl("Node".into()));
        assert!(t.types[&TypeKey::Idl("Node".into())]
            .anchors
            .iter()
            .any(|a| a.role == AnchorRole::ConceptAlias(ConceptAliasBasis::NameMatch)));
    }

    #[test]
    fn name_match_uses_every_name_and_tolerates_plurals() {
        let html = r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> {};
        interface <dfn data-dfn-type="interface" id="interface-document">Document</dfn> {};
        interface <dfn data-dfn-type="interface" id="interface-shadowroot">ShadowRoot</dfn> {};</pre>"##;
        let doc = Html::parse_document(html);
        let idl = vec![
            idl_def("interface-node", "Node", "interface", "interface Node {};"),
            idl_def(
                "interface-document",
                "Document",
                "interface",
                "interface Document {};",
            ),
            idl_def(
                "interface-shadowroot",
                "ShadowRoot",
                "interface",
                "interface ShadowRoot {};",
            ),
        ];
        let concept = |id: &str, names: &[&str]| ConceptDfn {
            id: id.into(),
            name: names[0].into(),
            names: names.iter().map(|n| n.to_string()).collect(),
        };
        let concepts = vec![
            concept("concept-node", &["Nodes"]),
            concept("concept-document", &["documents", "document"]),
            concept("concept-shadow-root", &["shadow roots", "shadow root"]),
        ];
        let t = collect_types(&doc, "DOM", "https://dom.spec.whatwg.org/", &idl, &concepts);
        assert_eq!(t.by_anchor["concept-node"], TypeKey::Idl("Node".into()));
        assert_eq!(
            t.by_anchor["concept-document"],
            TypeKey::Idl("Document".into())
        );
        assert_eq!(
            t.by_anchor["concept-shadow-root"],
            TypeKey::Idl("ShadowRoot".into())
        );
    }

    #[test]
    fn wattsi_partial_interface_dfn_is_a_partial_anchor() {
        let html = r##"<pre><code class="idl"><c- b>partial</c-> <c- b>interface</c-> <dfn data-lt="" id="document"><c- g>Document</c-></dfn> {
          <c- b>includes</c-> };</code></pre><pre><code class="idl"><a href="#document">Document</a> includes <a href="#globaleventhandlers">GlobalEventHandlers</a>;</code></pre>"##;
        let doc = Html::parse_document(html);
        let t = collect_types(&doc, "HTML", "https://html.spec.whatwg.org/", &[], &[]);

        let document = &t.types[&TypeKey::Idl("Document".into())];
        assert!(document
            .anchors
            .iter()
            .any(|a| a.role == AnchorRole::Partial && a.target.anchor == "document"));
        assert_eq!(t.by_anchor["document"], TypeKey::Idl("Document".into()));
        assert!(document
            .supertypes
            .iter()
            .any(|e| e.target == TypeKey::Idl("GlobalEventHandlers".into())));
    }

    #[test]
    fn element_definition_anchor_resolves_to_interface() {
        let html = r##"<h4 id="the-img-element">The img element</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlimageelement">HTMLImageElement</dfn> : HTMLElement {};</code></pre></dd></dl>"##;
        let doc = Html::parse_document(html);
        let idl = vec![idl_def(
            "htmlimageelement",
            "HTMLImageElement",
            "interface",
            "interface HTMLImageElement : HTMLElement {};",
        )];
        let t = collect_types(&doc, "HTML", "https://html.spec.whatwg.org/", &idl, &[]);
        assert_eq!(
            t.by_anchor["the-img-element"],
            TypeKey::Idl("HTMLImageElement".into())
        );
    }

    #[test]
    fn add_concept_creates_evidence_alias() {
        // Scenario: YAML processing explicitly links a concept dfn to an IDL type.
        // Use FETCH's "request" / Request — NameMatch would fire here too, but add_concept
        // is the Evidence API regardless of how the name matches.
        let html = r##"<pre class="def highlight idl">interface Request {};</pre>"##;
        let doc = Html::parse_document(html);
        let idl = vec![idl_def(
            "request-if",
            "Request",
            "interface",
            "interface Request {};",
        )];
        let mut t = collect_types(&doc, "FETCH", "https://fetch.spec.whatwg.org/", &idl, &[]);

        let key = t.add_concept("FETCH", "concept-request", "request", TypeKind::Concept);
        assert_eq!(key, TypeKey::Idl("Request".into()));
        assert!(t.types[&key].anchors.iter().any(|a| {
            a.target.anchor == "concept-request"
                && a.role == AnchorRole::ConceptAlias(ConceptAliasBasis::Evidence)
        }));
        assert_eq!(
            t.by_anchor["concept-request"],
            TypeKey::Idl("Request".into())
        );
    }

    #[test]
    fn add_concept_without_idl_match_creates_anchor_type() {
        let doc = Html::parse_document("<html></html>");
        let mut t = collect_types(&doc, "HTML", "https://html.spec.whatwg.org/", &[], &[]);

        let key = t.add_concept("HTML", "concept-navigable", "navigable", TypeKind::Concept);
        assert_eq!(
            key,
            TypeKey::Anchor(AnchorTarget {
                spec: "HTML".into(),
                anchor: "concept-navigable".into()
            })
        );
        let type_def = &t.types[&key];
        assert!(type_def
            .anchors
            .iter()
            .any(|a| a.role == AnchorRole::Defining));
        assert_eq!(t.by_anchor["concept-navigable"], key);
    }
}
