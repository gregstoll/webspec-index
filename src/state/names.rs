//! Type names in algorithm intros resolved against the finished object model (§8.1.4).
//!
//! The same rules as SP1's owner-name resolution (exact IDL name, then concept dfn name,
//! case-folded with a trailing plural `s` tolerated and canonicalized through concept
//! aliases), plus a case-insensitive IDL-name match, the rule SP1's `NameMatch` alias applies.

use crate::parse::steps::{AnchorTarget, LinkSpan};
use crate::state::model::{ObjectModel, TypeKey, TypeRef};
use crate::state::types::ConceptDfn;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub(crate) struct NameResolver {
    spec: String,
    idl_names: HashSet<String>,
    /// Lowercased IDL name → the first IDL name (in model order) with that folding.
    idl_by_lowercase: HashMap<String, String>,
    /// Same-spec anchor of a model type → that type's canonical key.
    key_by_anchor: HashMap<String, TypeKey>,
    /// Folded concept dfn name → the key it resolves to; the first dfn with a name wins.
    concept_by_name: HashMap<String, TypeKey>,
}

fn fold(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl NameResolver {
    pub(crate) fn new(spec: &str, model: &ObjectModel, concepts: &[ConceptDfn]) -> Self {
        let mut idl_names = HashSet::new();
        let mut idl_by_lowercase = HashMap::new();
        let mut key_by_anchor = HashMap::new();
        for type_def in &model.types {
            if let TypeKey::Idl(name) = &type_def.key {
                idl_names.insert(name.clone());
                idl_by_lowercase
                    .entry(name.to_lowercase())
                    .or_insert_with(|| name.clone());
            }
            for anchor in &type_def.anchors {
                if anchor.target.spec == spec {
                    key_by_anchor
                        .entry(anchor.target.anchor.clone())
                        .or_insert_with(|| type_def.key.clone());
                }
            }
        }
        let mut concept_by_name = HashMap::new();
        for dfn in concepts {
            let key = key_by_anchor.get(&dfn.id).cloned().unwrap_or_else(|| {
                TypeKey::Anchor(AnchorTarget {
                    spec: spec.to_string(),
                    anchor: dfn.id.clone(),
                })
            });
            for name in &dfn.names {
                concept_by_name
                    .entry(fold(name))
                    .or_insert_with(|| key.clone());
            }
        }
        Self {
            spec: spec.to_string(),
            idl_names,
            idl_by_lowercase,
            key_by_anchor,
            concept_by_name,
        }
    }

    /// The type an unlinked type name denotes, or `None` when no rule applies.
    pub(crate) fn resolve(&self, name: &str) -> Option<TypeKey> {
        if self.idl_names.contains(name) {
            return Some(TypeKey::Idl(name.to_string()));
        }
        let folded = fold(name);
        let concept = self.concept_by_name.get(&folded).or_else(|| {
            folded
                .strip_suffix('s')
                .and_then(|singular| self.concept_by_name.get(singular))
        });
        if let Some(key) = concept {
            return Some(key.clone());
        }
        self.idl_by_lowercase
            .get(&name.to_lowercase())
            .map(|idl| TypeKey::Idl(idl.clone()))
    }

    /// The type a link in a type position denotes.
    pub(crate) fn link_type(&self, link: &LinkSpan) -> TypeRef {
        static IDENT: OnceLock<Regex> = OnceLock::new();
        if let Some(target) = &link.target {
            if target.spec == self.spec {
                if let Some(key) = self.key_by_anchor.get(&target.anchor) {
                    return TypeRef::Known(key.clone());
                }
            }
        }
        if link.visible_text.starts_with('`') {
            let name = link.visible_text.replace('`', "");
            let ident = IDENT.get_or_init(|| Regex::new(r"^[A-Z_][A-Za-z0-9_]*$").unwrap());
            if ident.is_match(&name) {
                return TypeRef::Known(TypeKey::Idl(name));
            }
        }
        TypeRef::Unresolved(link.target.clone().unwrap_or_else(|| AnchorTarget {
            spec: String::new(),
            anchor: link.visible_text.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::TextSpan;
    use crate::state::testing::{extract_html, AUTODIR_HTML, INSERT_DOM};

    fn resolver(html: &str, spec: &str) -> NameResolver {
        let document = scraper::Html::parse_document(html);
        let state = extract_html(html, spec);
        NameResolver::new(
            spec,
            &state.model,
            &crate::state::declare::concept_dfns(&document),
        )
    }

    #[test]
    fn resolves_idl_names_concepts_and_case_insensitive_idl() {
        let dom = resolver(INSERT_DOM, "DOM");
        assert_eq!(dom.resolve("Node"), Some(TypeKey::Idl("Node".into())));
        assert_eq!(dom.resolve("node"), Some(TypeKey::Idl("Node".into())));
        assert_eq!(dom.resolve("nodes"), Some(TypeKey::Idl("Node".into())));
        let html = resolver(AUTODIR_HTML, "HTML");
        assert_eq!(
            html.resolve("element"),
            Some(TypeKey::Idl("Element".into()))
        );
        assert_eq!(html.resolve("time"), None);
    }

    #[test]
    fn concept_dfns_without_a_model_type_resolve_to_their_anchor() {
        let html = r##"<p>A <dfn id="text-track-cue">text track cue</dfn> is a unit of time-sensitive data.</p>"##;
        let resolver = resolver(html, "HTML");
        let anchor = TypeKey::Anchor(AnchorTarget {
            spec: "HTML".into(),
            anchor: "text-track-cue".into(),
        });
        assert_eq!(resolver.resolve("text  track\ncue"), Some(anchor.clone()));
        assert_eq!(resolver.resolve("Text track cues"), Some(anchor));
    }

    #[test]
    fn link_types_prefer_model_anchors_then_code_wrapped_idl_names() {
        let dom = resolver(INSERT_DOM, "DOM");
        let link = |href_anchor: &str, text: &str| LinkSpan {
            id: "l".into(),
            span: TextSpan { start: 0, end: 1 },
            visible_text: text.into(),
            href: format!("#{href_anchor}"),
            target: Some(AnchorTarget {
                spec: "DOM".into(),
                anchor: href_anchor.into(),
            }),
            generator_id: None,
            link_type: None,
        };
        assert_eq!(
            dom.link_type(&link("concept-node", "node")),
            TypeRef::Known(TypeKey::Idl("Node".into()))
        );
        assert_eq!(
            dom.link_type(&link("nhb", "`NavigationHistoryBehavior`")),
            TypeRef::Known(TypeKey::Idl("NavigationHistoryBehavior".into()))
        );
        assert_eq!(
            dom.link_type(&link("x", "tree order")),
            TypeRef::Unresolved(AnchorTarget {
                spec: "DOM".into(),
                anchor: "x".into()
            })
        );
        assert_eq!(
            dom.link_type(&link("x", "NavigationHistoryBehavior")),
            TypeRef::Unresolved(AnchorTarget {
                spec: "DOM".into(),
                anchor: "x".into()
            }),
            "an IDL-shaped name counts only when code-wrapped"
        );
        let untargeted = LinkSpan {
            target: None,
            ..link("x", "tree order")
        };
        assert_eq!(
            dom.link_type(&untargeted),
            TypeRef::Unresolved(AnchorTarget {
                spec: String::new(),
                anchor: "tree order".into()
            })
        );
    }
}
