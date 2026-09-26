//! HTML reflection (§7.8): IDL attributes whose getter and setter read and
//! write a content attribute, from `[Reflect…]` extended attributes and from
//! "must reflect" prose.
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

use crate::model::ParsedIdlDefinition;
use crate::parse::steps::{resolve_href, AnchorTarget};
use crate::state::block::{flatten, innermost_block, norm, pattern, token_text, BlockToken};
use crate::state::model::{AnchorRole, Reflection, ReflectionBasis, SuperBasis, TypeKey};
use crate::state::types::{preceding_heading_id, TypeTable};

pub(crate) fn reflections(
    document: &Html,
    spec: &str,
    base_url: &str,
    idl: &[ParsedIdlDefinition],
    table: &TypeTable,
) -> Vec<Reflection> {
    let content = ContentAttributes::collect(document);
    let elements = Elements::new(table);
    let mut out = Vec::new();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut members: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
    for row in idl {
        let (Some(owner), Some(text)) = (row.owner.as_deref(), row.idl_text.as_deref()) else {
            continue;
        };
        if row.kind != "attribute" || !text.contains("Reflect") {
            continue;
        }
        let brackets = members
            .entry(text)
            .or_insert_with(|| bracketed_attributes(text));
        let Some(captures) = brackets
            .get(row.name.as_str())
            .and_then(|list| reflect_re().captures(list))
        else {
            continue;
        };
        let name = captures
            .get(2)
            .map_or_else(|| row.name.to_lowercase(), |m| m.as_str().to_string());
        let content_attribute = || content.find(&name, &elements.names(owner), spec);
        match seen.get(row.anchor.as_str()) {
            Some(&index) => {
                let earlier: &mut Reflection = &mut out[index];
                if earlier.content_attribute.is_none() {
                    earlier.content_attribute = content_attribute();
                }
            }
            None => {
                seen.insert(row.anchor.as_str(), out.len());
                out.push(Reflection {
                    idl_attribute: local(spec, &row.anchor),
                    idl_attribute_name: row.name.clone(),
                    content_attribute: content_attribute(),
                    content_attribute_name: name,
                    basis: ReflectionBasis::IdlExtendedAttribute(captures[1].to_string()),
                });
            }
        }
    }
    let reflected: HashSet<String> = seen.into_keys().map(str::to_string).collect();
    out.extend(
        prose_reflections(document, spec, base_url, idl, &elements, &content)
            .into_iter()
            .filter(|r| !reflected.contains(&r.idl_attribute.anchor)),
    );
    out
}

/// The label a reflection's basis carries on its site and in query answers.
pub(crate) fn basis_label(basis: &ReflectionBasis) -> String {
    match basis {
        ReflectionBasis::IdlExtendedAttribute(word) => format!("[{word}]"),
        ReflectionBasis::ProseReflect => "prose".to_string(),
    }
}

fn local(spec: &str, anchor: &str) -> AnchorTarget {
    AnchorTarget {
        spec: spec.to_string(),
        anchor: anchor.to_string(),
    }
}

fn reflect_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(Reflect\w*)(?:="?([\w-]+)"?)?"#).unwrap())
}

/// Attribute name → its extended-attribute list, for every bracketed attribute
/// member of one IDL block.
fn bracketed_attributes(text: &str) -> HashMap<&str, &str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\[([^\]]*)\]\s*(?:readonly\s+)?attribute\s+[^;]*?\b(\w+)\s*;").unwrap()
    });
    re.captures_iter(text)
        .map(|c| {
            let (_, [list, name]) = c.extract();
            (name, list)
        })
        .collect()
}

/// Element names by IDL interface, from `ElementDefinition` anchors.
struct Elements<'a> {
    table: &'a TypeTable,
    /// Supertype or included mixin → the interfaces inheriting or including it.
    subtypes: HashMap<&'a TypeKey, Vec<&'a TypeKey>>,
}

impl<'a> Elements<'a> {
    fn new(table: &'a TypeTable) -> Self {
        let mut subtypes: HashMap<&TypeKey, Vec<&TypeKey>> = HashMap::new();
        for ty in table.types.values() {
            for edge in &ty.supertypes {
                if matches!(
                    edge.basis,
                    SuperBasis::IdlInheritance | SuperBasis::IdlIncludes
                ) {
                    subtypes.entry(&edge.target).or_default().push(&ty.key);
                }
            }
        }
        Self { table, subtypes }
    }

    /// The elements an attribute of `interface` belongs to: those whose
    /// definition heading anchors it or an interface inheriting or including it
    /// (`the-a-element` → `a`), and `html-global` for `HTMLElement`.
    fn names(&self, interface: &str) -> Vec<String> {
        let start = TypeKey::Idl(interface.to_string());
        let mut names = Vec::new();
        let mut seen = HashSet::from([&start]);
        let mut queue = vec![&start];
        while let Some(key) = queue.pop() {
            if matches!(key, TypeKey::Idl(name) if name == "HTMLElement") {
                names.push("html-global".to_string());
                continue;
            }
            if let Some(ty) = self.table.types.get(key) {
                names.extend(
                    ty.anchors
                        .iter()
                        .filter(|a| a.role == AnchorRole::ElementDefinition)
                        .filter_map(|a| {
                            let name = a.target.anchor.strip_prefix("the-")?;
                            name.strip_suffix("-element").map(str::to_string)
                        }),
                );
            }
            for &sub in self.subtypes.get(key).into_iter().flatten() {
                if seen.insert(sub) {
                    queue.push(sub);
                }
            }
        }
        names
    }
}

/// `dfn[data-dfn-type="element-attr"]` in document order.
struct ContentAttributes {
    dfns: Vec<(String, Vec<String>, String)>,
}

impl ContentAttributes {
    fn collect(document: &Html) -> Self {
        static SEL: OnceLock<Selector> = OnceLock::new();
        let sel = SEL
            .get_or_init(|| Selector::parse(r#"dfn[data-dfn-type="element-attr"][id]"#).unwrap());
        let dfns = document
            .select(sel)
            .map(|dfn| {
                let v = dfn.value();
                let elements = v
                    .attr("data-dfn-for")
                    .unwrap_or_default()
                    .split(',')
                    .map(|e| e.trim().to_string())
                    .collect();
                let text = norm(&dfn.text().collect::<String>());
                (text, elements, v.attr("id").unwrap_or_default().to_string())
            })
            .collect();
        Self { dfns }
    }

    fn find(&self, name: &str, elements: &[String], spec: &str) -> Option<AnchorTarget> {
        self.dfns
            .iter()
            .find(|(text, dfn_for, _)| text == name && dfn_for.iter().any(|e| elements.contains(e)))
            .map(|(_, _, id)| local(spec, id))
    }
}

fn prose_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^The ⟦[LCD](\d+)⟧ IDL attribute must (?:reflect|⟦L\d+⟧) the (?:⟦[LC](\d+)⟧|([\w-]+)) content attribute",
        )
        .unwrap()
    })
}

fn prose_reflections(
    document: &Html,
    spec: &str,
    base_url: &str,
    idl: &[ParsedIdlDefinition],
    elements: &Elements,
    content: &ContentAttributes,
) -> Vec<Reflection> {
    static SEL: OnceLock<Selector> = OnceLock::new();
    let sel = SEL.get_or_init(|| Selector::parse(r##"a[href$="#reflect"]"##).unwrap());
    let reflect = local("HTML", "reflect");
    let mut blocks = HashSet::new();
    let mut out = Vec::new();
    for link in document.select(sel) {
        let resolved = link
            .value()
            .attr("href")
            .and_then(|href| resolve_href(href, spec, base_url));
        if resolved.as_ref() != Some(&reflect) {
            continue;
        }
        let Some(block) = innermost_block(&link) else {
            continue;
        };
        if !blocks.insert(block.id()) {
            continue;
        }
        let tokens = flatten(&block, spec, base_url);
        let pattern = pattern(&tokens);
        let Some(captures) = prose_re().captures(&pattern.text) else {
            continue;
        };
        let slot = |group: usize| {
            captures
                .get(group)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .and_then(|i| pattern.slots.get(i))
                .map(|&index| &tokens[index])
        };
        let Some(attribute) = slot(1) else {
            continue;
        };
        let attribute_name = token_text(attribute).to_string();
        let anchor = match attribute {
            BlockToken::Link { target, .. } => target.clone(),
            BlockToken::Dfn { id, .. } if !id.is_empty() => Some(local(spec, id)),
            _ => None,
        };
        let (idl_attribute, owner) = match anchor {
            Some(anchor) => {
                let owner = idl
                    .iter()
                    .find(|row| row.kind == "attribute" && row.anchor == anchor.anchor)
                    .and_then(|row| row.owner.clone());
                (anchor, owner)
            }
            None => {
                let Some(row) = block_interface(&block, elements.table).and_then(|owner| {
                    idl.iter().find(|row| {
                        row.kind == "attribute"
                            && row.name == attribute_name
                            && row.owner.as_deref() == Some(owner.as_str())
                    })
                }) else {
                    continue;
                };
                (local(spec, &row.anchor), row.owner.clone())
            }
        };
        let (name, linked) = match slot(2) {
            Some(token) => (
                token_text(token).to_string(),
                match token {
                    BlockToken::Link { target, .. } => target.clone(),
                    _ => None,
                },
            ),
            None => (captures[3].to_string(), None),
        };
        let content_attribute = linked.or_else(|| {
            let elements = owner
                .as_deref()
                .map(|owner| elements.names(owner))
                .unwrap_or_default();
            content.find(&name, &elements, spec)
        });
        out.push(Reflection {
            idl_attribute,
            idl_attribute_name: attribute_name,
            content_attribute,
            content_attribute_name: name,
            basis: ReflectionBasis::ProseReflect,
        });
    }
    out
}

/// The IDL interface of the element whose definition section holds `block`.
fn block_interface(block: &ElementRef<'_>, table: &TypeTable) -> Option<String> {
    std::iter::once(*block)
        .chain(block.ancestors().filter_map(ElementRef::wrap))
        .find_map(|el| preceding_heading_id(&el))
        .and_then(|heading| match table.by_anchor.get(&heading) {
            Some(TypeKey::Idl(name)) => Some(name.clone()),
            _ => None,
        })
}

/// `HTMLAnchorElement` with `[Reflect]` and `[Reflect=download]` attributes and
/// the `target` content attribute dfn.
#[cfg(test)]
pub(crate) const REFLECT_HTML: &str = r##"<h4 id="the-a-element">The a element</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlanchorelement">HTMLAnchorElement</dfn> : HTMLElement {
      [CEReactions, <a href="#xattr-reflect">Reflect</a>] attribute DOMString <dfn data-dfn-for="HTMLAnchorElement" data-dfn-type="attribute" id="dom-a-target">target</dfn>;
      [CEReactions, <a href="#xattr-reflect">Reflect</a>=<a>download</a>] attribute DOMString <dfn data-dfn-for="HTMLAnchorElement" data-dfn-type="attribute" id="dom-a-dl">dl</dfn>;
    };</code></pre></dd></dl>
    <dfn data-dfn-for="a" data-dfn-type="element-attr" id="attr-hyperlink-target">target</dfn>"##;

#[cfg(test)]
mod tests {
    use crate::state::model::{ReflectionBasis, SiteClass};

    #[test]
    fn reflect_extended_attribute_and_prose_reflection() {
        let state = crate::state::testing::extract_html(super::REFLECT_HTML, "HTML");
        let r: Vec<_> = state
            .model
            .reflections
            .iter()
            .map(|r| {
                (
                    r.idl_attribute.anchor.as_str(),
                    r.content_attribute_name.as_str(),
                    r.content_attribute.as_ref().map(|c| c.anchor.as_str()),
                )
            })
            .collect();
        assert_eq!(
            r,
            [
                ("dom-a-target", "target", Some("attr-hyperlink-target")),
                ("dom-a-dl", "download", None)
            ]
        );
        let sites = crate::state::extract::derive_sites(&state);
        assert!(sites.iter().any(|s| s.class == SiteClass::Declared
            && s.op == "reflect"
            && s.target.as_ref().unwrap().anchor == "attr-hyperlink-target"));
    }

    #[test]
    fn prose_reflection_links_the_idl_and_content_attributes() {
        let html = r##"<h4 id="the-img-element">The img element</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlimageelement">HTMLImageElement</dfn> : HTMLElement {
      attribute DOMString <dfn data-dfn-for="HTMLImageElement" data-dfn-type="attribute" id="dom-img-decoding">decoding</dfn>;
    };</code></pre></dd></dl>
    <dl><dt><dfn data-dfn-for="img" data-dfn-type="element-attr" id="attr-img-decoding">decoding</dfn></dt><dd>x</dd></dl>
    <p>The <code><a href="#dom-img-decoding">decoding</a></code> IDL attribute must <a href="#reflect">reflect</a> the <code><a href="#attr-img-decoding">decoding</a></code> content attribute, <a href="#limited-to-only-known-values">limited to only known values</a>.</p>
    <p>The <a href="#reflect">reflect</a> algorithm is elsewhere.</p>"##;
        let state = crate::state::testing::extract_html(html, "HTML");
        let [reflection] = state.model.reflections.as_slice() else {
            panic!("{:?}", state.model.reflections);
        };
        assert_eq!(reflection.idl_attribute.anchor, "dom-img-decoding");
        assert_eq!(reflection.idl_attribute_name, "decoding");
        assert_eq!(reflection.content_attribute_name, "decoding");
        assert_eq!(
            reflection.content_attribute.as_ref().unwrap().anchor,
            "attr-img-decoding"
        );
        assert_eq!(reflection.basis, ReflectionBasis::ProseReflect);
        let site = crate::state::extract::derive_sites(&state)
            .into_iter()
            .find(|s| s.op == "reflect")
            .unwrap();
        assert_eq!(
            site.text,
            "The decoding IDL attribute reflects the decoding content attribute."
        );
        assert_eq!(site.subject.anchor, "dom-img-decoding");
        assert_eq!(site.context, "reflection");
        assert_eq!(site.receiver, "this");
        assert_eq!(site.target_text, "decoding");
        assert_eq!(site.basis, "reflect");
    }

    #[test]
    fn prose_dfn_attribute_on_an_inherited_interface() {
        let html = r##"<h4 id="the-audio-element">The audio element</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlaudioelement">HTMLAudioElement</dfn> : <a href="#htmlmediaelement">HTMLMediaElement</a> {};</code></pre></dd></dl>
    <pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlmediaelement">HTMLMediaElement</dfn> : HTMLElement {
      attribute DOMString <a href="#dom-media-preload">preload</a>;
    };</code></pre>
    <p>The <dfn data-dfn-for="audio,video" data-dfn-type="element-attr" id="attr-media-preload"><code>preload</code></dfn> attribute is an enumerated attribute.</p>
    <p>The <dfn data-dfn-for="HTMLMediaElement" data-dfn-type="attribute" id="dom-media-preload"><code>preload</code></dfn> IDL attribute must <a href="#reflect">reflect</a> the <code>preload</code> content attribute.</p>"##;
        let state = crate::state::testing::extract_html(html, "HTML");
        let r: Vec<_> = state
            .model
            .reflections
            .iter()
            .map(|r| {
                (
                    r.idl_attribute.anchor.as_str(),
                    r.content_attribute.as_ref().map(|c| c.anchor.as_str()),
                    r.basis.clone(),
                )
            })
            .collect();
        assert_eq!(
            r,
            [(
                "dom-media-preload",
                Some("attr-media-preload"),
                ReflectionBasis::ProseReflect
            )]
        );
    }

    #[test]
    fn global_attribute_reflection_uses_html_global_dfns() {
        let html = r##"<pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlelement">HTMLElement</dfn> : Element {
      [CEReactions, <a href="#xattr-reflect">Reflect</a>] attribute DOMString <dfn data-dfn-for="HTMLElement" data-dfn-type="attribute" id="dom-title">title</dfn>;
      [<a href="#xattr-reflect">ReflectSetter</a>="access-key"] readonly attribute DOMString <dfn data-dfn-for="HTMLElement" data-dfn-type="attribute" id="dom-accesskey">accessKey</dfn>;
    };</code></pre>
    <dfn data-dfn-for="html-global" data-dfn-type="element-attr" id="attr-title">title</dfn>"##;
        let state = crate::state::testing::extract_html(html, "HTML");
        let r: Vec<_> = state
            .model
            .reflections
            .iter()
            .map(|r| {
                (
                    r.content_attribute_name.as_str(),
                    r.content_attribute.as_ref().map(|c| c.anchor.as_str()),
                    r.basis.clone(),
                )
            })
            .collect();
        assert_eq!(
            r,
            [
                (
                    "title",
                    Some("attr-title"),
                    ReflectionBasis::IdlExtendedAttribute("Reflect".into())
                ),
                (
                    "access-key",
                    None,
                    ReflectionBasis::IdlExtendedAttribute("ReflectSetter".into())
                ),
            ]
        );
    }
}
