//! Field and owner inference (§6.3): R1 `data-dfn-for` and R2 declaration sentences.
use crate::parse::idl_defs::normalize_owner;
use crate::parse::sections::is_inside_algorithm_content;
use crate::parse::steps::StructuralSpec;
use crate::state::block::{
    flatten, innermost_block, norm, pattern, plain_text, sentences, BlockToken,
};
use crate::state::model::{
    AnchorTarget, DeclarationSite, FieldBasis, FieldDef, ModelIssue, Owner, OwnerBasis, OwnerRef,
    OwnerVia, SetMember, StateIssueCode, TypeExpr, TypeKey, TypeKind,
};
use crate::state::types::{ConceptDfn, TypeTable};
use regex::Regex;
use scraper::{ElementRef, Html, Node, Selector};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

static R2: &str = r"^(?:(?:In addition(?: to [^,]*)?|Additionally|Also), )?(?:(?:Each|Every|All|A|An|The) )?(?P<owners>(?:⟦[LDC]\d+⟧|[A-Za-z][\w-]*(?: [A-Za-z][\w-]*){0,3}?)(?:(?:, and |, | and )(?:⟦[LDC]\d+⟧|[A-Za-z][\w-]*(?: [A-Za-z][\w-]*){0,3}?))*) (?:(?:objects?|interfaces?|elements?|instances?) )?(?:also )?(?:has|have)(?: |:)";
static R2_ALT: &str =
    r"^Objects (?:that implement|implementing) the (?P<owners>⟦L\d+⟧) interface (?:also )?have ";

const CLASSIFIERS: [&str; 7] = [
    "object",
    "objects",
    "interface",
    "element",
    "elements",
    "instance",
    "instances",
];

/// "An HTML element can have …" states an ability, not a declaration: a plain-word owner phrase
/// ending in one of these is the subject plus an auxiliary verb.
const AUXILIARIES: [&str; 13] = [
    "can", "cannot", "could", "may", "might", "must", "shall", "should", "will", "would", "does",
    "do", "did",
];

fn regex(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).unwrap())
}

fn is_concept_dfn(dfn: &ElementRef<'_>) -> bool {
    let v = dfn.value();
    v.name() == "dfn"
        && v.attr("id").is_some()
        && matches!(v.attr("data-dfn-type"), None | Some("dfn"))
        && !dfn
            .children()
            .filter_map(ElementRef::wrap)
            .any(|c| c.value().name() == "var")
        && !dfn
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|a| a.value().name() == "pre")
}

/// The dfn text, then its `data-lt` alternatives, deduplicated.
fn dfn_names(dfn: &ElementRef<'_>) -> (String, Vec<String>) {
    let name = norm(&dfn.text().collect::<String>());
    let mut names = vec![name.clone()];
    for alt in dfn.value().attr("data-lt").unwrap_or_default().split('|') {
        let alt = norm(alt);
        if !alt.is_empty() && !names.contains(&alt) {
            names.push(alt);
        }
    }
    (name, names)
}

/// Concept dfns: `dfn[id]` without `data-dfn-type` or with `"dfn"`, not parameters, not in `<pre>`.
#[allow(dead_code)]
pub(crate) fn concept_dfns(document: &Html) -> Vec<ConceptDfn> {
    static SEL: OnceLock<Selector> = OnceLock::new();
    let sel = SEL.get_or_init(|| Selector::parse("dfn[id]").unwrap());
    document
        .select(sel)
        .filter(is_concept_dfn)
        .map(|dfn| {
            let (name, names) = dfn_names(&dfn);
            ConceptDfn {
                id: dfn.value().attr("id").unwrap_or_default().to_string(),
                name,
                names,
            }
        })
        .collect()
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct DeclareOutput {
    pub fields: Vec<FieldDef>,
    pub members: Vec<SetMember>,
    pub issues: Vec<ModelIssue>,
    pub counters: DeclareCounters,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct DeclareCounters {
    pub concept_dfns: u32,
    pub owner_by_rule: BTreeMap<String, u32>,
    pub owner_candidates: u32,
    pub owner_resolved: u32,
    pub set_members: u32,
}

/// Names bound by reviewed type declarations (§8.2 `name`).
#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct NameBindings {
    pub names: HashMap<String, TypeKey>,
}

/// Concept dfns in document order, each with the id of the heading it sits under.
fn located_concept_dfns(document: &Html) -> Vec<(ElementRef<'_>, String)> {
    let mut section = String::new();
    let mut out = Vec::new();
    for el in document
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        match el.value().name() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if let Some(id) = el.value().attr("id") {
                    section = id.to_string();
                }
            }
            "dfn" if is_concept_dfn(&el) => out.push((el, section.clone())),
            _ => {}
        }
    }
    out
}

fn fold(name: &str) -> String {
    norm(name).to_lowercase()
}

struct ConceptIndex<'a> {
    name_by_id: HashMap<&'a str, String>,
    id_by_name: HashMap<String, &'a str>,
}

impl<'a> ConceptIndex<'a> {
    fn new(dfns: &[(ElementRef<'a>, String)]) -> Self {
        let mut index = Self {
            name_by_id: HashMap::new(),
            id_by_name: HashMap::new(),
        };
        for (dfn, _) in dfns {
            let id = dfn.value().attr("id").unwrap_or_default();
            let (name, names) = dfn_names(dfn);
            for n in &names {
                index.id_by_name.entry(fold(n)).or_insert(id);
            }
            index.name_by_id.entry(id).or_insert(name);
        }
        index
    }

    /// Case-folded, whitespace-collapsed lookup that tolerates a trailing plural `s`.
    fn by_name(&self, phrase: &str) -> Option<&'a str> {
        let query = fold(phrase);
        self.id_by_name
            .get(&query)
            .or_else(|| {
                query
                    .strip_suffix('s')
                    .and_then(|singular| self.id_by_name.get(singular))
            })
            .copied()
    }
}

enum OwnerPhrase {
    /// Token index of a link, dfn or code placeholder.
    Slot(usize),
    Words(String),
}

struct Resolver<'r, 'a> {
    spec: &'r str,
    table: &'r mut TypeTable,
    concepts: &'r ConceptIndex<'a>,
    bindings: &'r NameBindings,
}

impl Resolver<'_, '_> {
    fn concept(&mut self, id: &str, name: &str) -> TypeKey {
        match self.table.by_anchor.get(id) {
            Some(key) => key.clone(),
            None => self
                .table
                .add_concept(self.spec, id, name, TypeKind::Concept),
        }
    }

    fn link(&mut self, target: &AnchorTarget) -> TypeKey {
        if target.spec == self.spec {
            if let Some(key) = self.table.by_anchor.get(&target.anchor) {
                return key.clone();
            }
            if let Some(name) = self.concepts.name_by_id.get(target.anchor.as_str()) {
                return self
                    .table
                    .add_concept(self.spec, &target.anchor, name, TypeKind::Concept);
            }
        }
        TypeKey::Anchor(target.clone())
    }

    /// Name owner resolution. `idl_identifier` admits any WebIDL identifier as a provisional
    /// `idl:` key; only R1 names and `<code>` owners take that step.
    fn name(&mut self, name: &str, idl_identifier: bool) -> Option<OwnerRef> {
        static IDENT: OnceLock<Regex> = OnceLock::new();
        if self.table.idl_names.contains(name) {
            return Some(OwnerRef {
                key: TypeKey::Idl(name.to_string()),
                via: OwnerVia::IdlName,
            });
        }
        if let Some(id) = self.concepts.by_name(name) {
            let concept_name = self.concepts.name_by_id[id].clone();
            return Some(OwnerRef {
                key: self.concept(id, &concept_name),
                via: OwnerVia::DfnName,
            });
        }
        if let Some(key) = self.bindings.names.get(name) {
            return Some(OwnerRef {
                key: key.clone(),
                via: OwnerVia::Declaration,
            });
        }
        (idl_identifier && regex(&IDENT, r"^[A-Z_][A-Za-z0-9_]*$").is_match(name)).then(|| {
            OwnerRef {
                key: TypeKey::Idl(name.to_string()),
                via: OwnerVia::IdlName,
            }
        })
    }

    /// Resolves one owner phrase, or returns its lowercase hint.
    fn phrase(&mut self, phrase: &OwnerPhrase, tokens: &[BlockToken]) -> Result<OwnerRef, String> {
        let named = |r: &mut Self, text: &str, code: bool| {
            r.name(text, code).ok_or_else(|| text.to_lowercase())
        };
        match phrase {
            OwnerPhrase::Words(words) => named(self, &strip_classifiers(words), false),
            OwnerPhrase::Slot(index) => match &tokens[*index] {
                BlockToken::Link {
                    target: Some(target),
                    ..
                } => Ok(OwnerRef {
                    key: self.link(target),
                    via: OwnerVia::Link,
                }),
                BlockToken::Link {
                    text,
                    target: None,
                    code,
                } => named(self, text, *code),
                BlockToken::Dfn { id, text, .. } => Ok(OwnerRef {
                    key: self.concept(id, text),
                    via: OwnerVia::DfnName,
                }),
                BlockToken::Code(text) => named(self, text, true),
                BlockToken::Var(text) | BlockToken::Text(text) => Err(text.to_lowercase()),
            },
        }
    }

    /// The same-spec concept dfn an owner phrase names, if any.
    fn concept_anchor(&self, phrase: &OwnerPhrase, tokens: &[BlockToken]) -> Option<String> {
        let local = |id: &str| {
            self.concepts
                .name_by_id
                .contains_key(id)
                .then(|| id.to_string())
        };
        match phrase {
            OwnerPhrase::Words(words) => self
                .concepts
                .by_name(&strip_classifiers(words))
                .map(str::to_string),
            OwnerPhrase::Slot(index) => match &tokens[*index] {
                BlockToken::Link {
                    target: Some(target),
                    ..
                } if target.spec == self.spec => local(&target.anchor),
                BlockToken::Dfn { id, .. } => local(id),
                _ => None,
            },
        }
    }

    /// A `data-dfn-for` naming a local IDL type, on a dfn whose declaration sentence names one
    /// concept dfn, is evidence that the concept is that type (§6.2).
    fn alias_by_dfn_for(&mut self, key: &TypeKey, phrase: &OwnerPhrase, tokens: &[BlockToken]) {
        let TypeKey::Idl(name) = key else {
            return;
        };
        if !self.table.idl_names.contains(name) {
            return;
        }
        if let Some(anchor) = self.concept_anchor(phrase, tokens) {
            self.table.alias_concept(key, self.spec, &anchor);
        }
    }

    fn owner(
        &mut self,
        phrases: &[OwnerPhrase],
        tokens: &[BlockToken],
        basis: OwnerBasis,
    ) -> Owner {
        let mut types: Vec<OwnerRef> = Vec::new();
        let mut hints = Vec::new();
        for phrase in phrases {
            match self.phrase(phrase, tokens) {
                Ok(owner) => {
                    if !types.iter().any(|t| t.key == owner.key) {
                        types.push(owner);
                    }
                }
                Err(hint) => hints.push(hint),
            }
        }
        if types.is_empty() {
            Owner::Unknown {
                hint: Some(hints.join(", ")),
            }
        } else {
            Owner::Known { types, basis }
        }
    }
}

fn strip_classifiers(words: &str) -> String {
    let mut words: Vec<&str> = words.split(' ').collect();
    while words.len() > 1 && CLASSIFIERS.contains(words.last().unwrap()) {
        words.pop();
    }
    words.join(" ")
}

/// R2: the owners of a declaration sentence ("Each OWNER has …") that precedes the dfn.
fn declaration_owners(tokens: &[BlockToken], dfn_id: &str) -> Option<Vec<OwnerPhrase>> {
    static R2_RE: OnceLock<Regex> = OnceLock::new();
    static R2_ALT_RE: OnceLock<Regex> = OnceLock::new();
    static SEPARATOR: OnceLock<Regex> = OnceLock::new();
    static PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
    let pat = pattern(tokens);
    let slot = pat
        .slots
        .iter()
        .position(|&i| matches!(&tokens[i], BlockToken::Dfn { id, .. } if id == dfn_id))?;
    let dfn_at = pat.text.find(&format!("⟦D{slot}⟧"))?;
    let sentence = sentences(&pat)
        .into_iter()
        .find(|range| range.contains(&dfn_at))?;
    let text = &pat.text[sentence.start..];
    let caps = regex(&R2_RE, R2)
        .captures(text)
        .or_else(|| regex(&R2_ALT_RE, R2_ALT).captures(text))?;
    if sentence.start + caps.get(0)?.end() > dfn_at {
        return None;
    }
    let placeholder = regex(&PLACEHOLDER, r"^⟦[LDC](\d+)⟧$");
    let mut owners = Vec::new();
    for part in regex(&SEPARATOR, r", and |, | and ").split(&caps["owners"]) {
        if let Some(n) = placeholder.captures(part) {
            owners.push(OwnerPhrase::Slot(pat.slots[n[1].parse::<usize>().ok()?]));
        } else if part.split(' ').any(|w| AUXILIARIES.contains(&w)) {
            return None;
        } else {
            owners.push(OwnerPhrase::Words(part.to_string()));
        }
    }
    Some(owners)
}

fn is_algorithm_container(element: &ElementRef<'_>) -> bool {
    element.value().name() == "div"
        && (element.value().classes().any(|c| c == "algorithm")
            || element.value().attr("data-algorithm").is_some())
}

fn followed_by_ol(block: &ElementRef<'_>) -> bool {
    for sibling in block.next_siblings() {
        match sibling.value() {
            Node::Text(t) if t.trim().is_empty() => continue,
            Node::Comment(_) => continue,
            Node::Element(e) => return e.name() == "ol",
            _ => return false,
        }
    }
    false
}

/// Bikeshed puts `data-dfn-for` on algorithms and predicates too; those are never fields.
fn is_algorithm_section(
    dfn: &ElementRef<'_>,
    block: Option<&ElementRef<'_>>,
    tokens: &[BlockToken],
    algorithm_anchors: &HashSet<&str>,
) -> bool {
    let id = dfn.value().attr("id").unwrap_or_default();
    if algorithm_anchors.contains(id)
        || dfn
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|a| is_algorithm_container(&a))
    {
        return true;
    }
    let text = plain_text(tokens);
    text.starts_with("To ")
        || text.contains("run these steps")
        || text.contains("the following steps")
        || block.is_some_and(followed_by_ol)
}

fn owner_keys(owner: &Owner) -> Option<BTreeSet<&TypeKey>> {
    match owner {
        Owner::Known { types, .. } => Some(types.iter().map(|t| &t.key).collect()),
        Owner::Unknown { .. } => None,
    }
}

/// Cross-spec anchor keys are canonicalized only at query time, so they can't be compared here.
fn is_cross_spec(key: &TypeKey, spec: &str) -> bool {
    matches!(key, TypeKey::Anchor(target) if target.spec != spec)
}

fn rule_key(basis: &OwnerBasis) -> &'static str {
    match basis {
        OwnerBasis::DfnFor => "dfn_for",
        OwnerBasis::DeclarationSentence => "declaration_sentence",
        OwnerBasis::PropertyList => "property_list",
        OwnerBasis::StructItems => "struct_items",
        OwnerBasis::Override { .. } => "override",
    }
}

/// Infers fields and their owners from concept dfns outside algorithm steps (§6.3).
#[allow(dead_code)]
pub(crate) fn declare_fields(
    document: &Html,
    spec: &str,
    base_url: &str,
    structure: &StructuralSpec,
    table: &mut TypeTable,
    bindings: &NameBindings,
) -> DeclareOutput {
    let mut out = DeclareOutput::default();
    let dfns = located_concept_dfns(document);
    let concepts = ConceptIndex::new(&dfns);
    let algorithm_anchors: HashSet<&str> = structure
        .algorithms
        .iter()
        .map(|a| a.source.section_anchor.as_str())
        .collect();
    let mut resolver = Resolver {
        spec,
        table,
        concepts: &concepts,
        bindings,
    };
    out.counters.concept_dfns = dfns.len() as u32;

    for (dfn, section_anchor) in &dfns {
        if is_inside_algorithm_content(dfn) {
            continue;
        }
        let id = dfn.value().attr("id").unwrap_or_default();
        let block = innermost_block(dfn).or_else(|| dfn.parent().and_then(ElementRef::wrap));
        let tokens = block
            .as_ref()
            .map(|b| flatten(b, spec, base_url))
            .unwrap_or_default();
        let dfn_for = dfn
            .value()
            .attr("data-dfn-for")
            .map(normalize_owner)
            .filter(|owner| !owner.is_empty());
        let phrases = declaration_owners(&tokens, id);
        let r1 = dfn_for.map(|name| {
            let owner = resolver.name(&name, true);
            if let (Some(owner), Some([phrase])) = (&owner, phrases.as_deref()) {
                resolver.alias_by_dfn_for(&owner.key, phrase, &tokens);
            }
            (name, owner)
        });
        let declared = phrases
            .map(|phrases| resolver.owner(&phrases, &tokens, OwnerBasis::DeclarationSentence));

        let (owner, rule) = match (r1, &declared) {
            (Some((name, r1)), _) => {
                let owner = match r1 {
                    Some(owner) => Owner::Known {
                        types: vec![owner],
                        basis: OwnerBasis::DfnFor,
                    },
                    None => Owner::Unknown {
                        hint: Some(name.to_lowercase()),
                    },
                };
                let conflict = owner_keys(&owner)
                    .zip(declared.as_ref().and_then(owner_keys))
                    .filter(|(r1, r2)| r1 != r2 && !r2.iter().any(|k| is_cross_spec(k, spec)));
                if let Some((r1, r2)) = conflict {
                    let list = |keys: BTreeSet<&TypeKey>| {
                        keys.iter()
                            .map(|k| k.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    out.issues.push(ModelIssue {
                        code: StateIssueCode::OwnerConflict,
                        anchor: Some(id.to_string()),
                        message: format!(
                            "data-dfn-for owner {} of {id} disagrees with declaration sentence owner {}; data-dfn-for kept",
                            list(r1),
                            list(r2)
                        ),
                    });
                }
                (owner, OwnerBasis::DfnFor)
            }
            (None, Some(owner)) => (owner.clone(), OwnerBasis::DeclarationSentence),
            (None, None) => continue,
        };

        out.counters.owner_candidates += 1;
        *out.counters
            .owner_by_rule
            .entry(rule_key(&rule).to_string())
            .or_default() += 1;
        if matches!(owner, Owner::Known { .. }) {
            out.counters.owner_resolved += 1;
        }

        let field_basis = if declared.is_some() {
            FieldBasis::Declared
        } else if is_algorithm_section(dfn, block.as_ref(), &tokens, &algorithm_anchors) {
            continue;
        } else {
            FieldBasis::DfnForOnly
        };

        if let Owner::Unknown { hint } = &owner {
            out.issues.push(ModelIssue {
                code: StateIssueCode::OwnerUnknown,
                anchor: Some(id.to_string()),
                message: format!(
                    "owner of {id} not inferred (hint: {})",
                    hint.as_deref().unwrap_or_default()
                ),
            });
        }

        let (name, names) = dfn_names(dfn);
        out.fields.push(FieldDef {
            anchor: AnchorTarget {
                spec: spec.to_string(),
                anchor: id.to_string(),
            },
            name,
            names,
            owner,
            field_basis,
            declared_type: TypeExpr::Unknown,
            initial: None,
            declaration: Some(DeclarationSite {
                section_anchor: section_anchor.clone(),
                text: plain_text(&tokens),
            }),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::model::{AnchorRole, ConceptAliasBasis};
    use scraper::Html;

    fn run(html: &str, spec: &str) -> DeclareOutput {
        run_with_table(html, spec).0
    }

    fn run_with_table(html: &str, spec: &str) -> (DeclareOutput, TypeTable) {
        let doc = Html::parse_document(html);
        let base = if spec == "DOM" {
            "https://dom.spec.whatwg.org/"
        } else {
            "https://html.spec.whatwg.org/"
        };
        let idl = crate::parse::idl_defs::extract_idl_definitions(html);
        let structure = extract_step_structure_from_document(&doc, spec, base, "hash:t");
        let concepts = concept_dfns(&doc);
        let mut table = crate::state::types::collect_types(&doc, spec, base, &idl, &concepts);
        let out = declare_fields(
            &doc,
            spec,
            base,
            &structure,
            &mut table,
            &NameBindings::default(),
        );
        (out, table)
    }

    fn alias_role(table: &TypeTable, idl: &str, anchor: &str) -> Option<AnchorRole> {
        table.types[&TypeKey::Idl(idl.into())]
            .anchors
            .iter()
            .find(|a| a.target.anchor == anchor)
            .map(|a| a.role.clone())
    }

    const DOM_IDL: &str = r#"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> {};
      interface <dfn data-dfn-type="interface" id="interface-documenttype">DocumentType</dfn> : Node {};
      interface <dfn data-dfn-type="interface" id="interface-attr">Attr</dfn> : Node {};</pre>"#;

    #[test]
    fn plural_and_data_lt_concepts_become_evidence_aliases_through_dfn_for() {
        let html = format!(
            r##"{DOM_IDL}
      <p><dfn data-dfn-type="dfn" id="concept-node">Nodes</dfn> are objects.</p>
      <p>There are <dfn data-dfn-type="dfn" data-lt="doctype" id="concept-doctype">doctypes</dfn> and <dfn data-dfn-type="dfn" data-lt="attribute" id="concept-attribute">attributes</dfn>.</p>
      <p>Each <a href="#concept-node">node</a> has an associated <dfn data-dfn-for="Node" data-dfn-type="dfn" id="concept-node-document">node document</dfn>.</p>
      <p><a href="#concept-doctype">Doctypes</a> have an associated <dfn data-dfn-for="DocumentType" data-dfn-type="dfn" id="concept-doctype-name">name</dfn>.</p>
      <p><a href="#concept-attribute">Attributes</a> have a <dfn data-dfn-for="Attr" data-dfn-type="dfn" id="concept-attribute-value">value</dfn>.</p>"##
        );
        let (out, table) = run_with_table(&html, "DOM");
        assert!(
            !out.issues
                .iter()
                .any(|i| i.code == StateIssueCode::OwnerConflict),
            "{:?}",
            out.issues
        );
        for (idl, concept) in [
            ("Node", "concept-node"),
            ("DocumentType", "concept-doctype"),
            ("Attr", "concept-attribute"),
        ] {
            assert_eq!(table.by_anchor[concept], TypeKey::Idl(idl.into()));
            assert_eq!(
                alias_role(&table, idl, concept),
                Some(AnchorRole::ConceptAlias(ConceptAliasBasis::Evidence))
            );
            assert!(!table.types.contains_key(&TypeKey::Anchor(AnchorTarget {
                spec: "DOM".into(),
                anchor: concept.into()
            })));
        }
        assert_eq!(
            owner_keys(field(&out, "concept-doctype-name")),
            ["idl:DocumentType"]
        );
    }

    #[test]
    fn cross_spec_declaration_owner_is_not_a_conflict() {
        let out = run(
            r##"<p>Each <a href="https://html.spec.whatwg.org/#window">Window</a> object has an associated <dfn data-dfn-for="Window" data-dfn-type="dfn" id="w">current event</dfn>.</p>
      <p>Each <a href="#concept-thing">thing</a> has a <dfn data-dfn-for="Window" data-dfn-type="dfn" id="v">value</dfn>.</p>"##,
            "DOM",
        );
        let conflicts: Vec<_> = out
            .issues
            .iter()
            .filter(|i| i.code == StateIssueCode::OwnerConflict)
            .filter_map(|i| i.anchor.as_deref())
            .collect();
        assert_eq!(conflicts, ["v"]);
    }

    fn field<'a>(out: &'a DeclareOutput, anchor: &str) -> &'a FieldDef {
        out.fields
            .iter()
            .find(|f| f.anchor.anchor == anchor)
            .unwrap_or_else(|| panic!("no field {anchor}"))
    }

    fn owner_keys(f: &FieldDef) -> Vec<String> {
        match &f.owner {
            Owner::Known { types, .. } => types.iter().map(|t| t.key.to_string()).collect(),
            Owner::Unknown { .. } => vec![],
        }
    }

    #[test]
    fn wattsi_declaration_sentence_with_partial_interface() {
        let out = run(
            r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
      <h3 id="the-document-object">The Document object</h3>
      <p>Each <code><a href="#document">Document</a></code> has an <dfn id="f">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p>"##,
            "HTML",
        );
        let f = field(&out, "f");
        assert_eq!(owner_keys(f), ["idl:Document"]);
        assert!(
            matches!(&f.owner, Owner::Known { basis: OwnerBasis::DeclarationSentence, types } if types[0].via == OwnerVia::Link)
        );
        assert_eq!(f.field_basis, FieldBasis::Declared);
        assert_eq!(
            f.declaration.as_ref().unwrap().section_anchor,
            "the-document-object"
        );
    }

    #[test]
    fn multiple_owners_share_one_declaration() {
        let out = run(
            r##"<h4 id="the-img-element">img</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlimageelement">HTMLImageElement</dfn> {};</code></pre></dd></dl>
      <p>Each <a href="#the-img-element">img</a>, <a href="#the-audio-element">audio</a>, <a href="#the-video-element">video</a>, and <a href="#the-iframe-element">iframe</a> element has associated <dfn id="llrs">lazy load resumption steps</dfn>, initially null.</p>"##,
            "HTML",
        );
        assert_eq!(owner_keys(field(&out, "llrs")).len(), 4);
    }

    #[test]
    fn negative_sentences_give_no_owner_candidate() {
        let out = run(
            r##"<p>If a <a href="#reflect">reflected IDL attribute</a> has the type long, <dfn id="a">x</dfn>.</p>
      <p>A <a href="#media-element">media element</a> is said to have <dfn id="b">ended playback</dfn> when:</p>
      <p>An HTML element can have specific <dfn id="c">focus</dfn>.</p>"##,
            "HTML",
        );
        assert!(out.fields.is_empty(), "{:?}", out.fields);
    }

    #[test]
    fn unresolvable_names_keep_a_hint() {
        let out = run(
            r#"<p>Such objects have associated <dfn id="x">thing</dfn>.</p>
      <p>Some video files also have an explicit date and time corresponding to the media timeline, known as the <dfn id="timeline-offset">timeline offset</dfn>.</p>"#,
            "HTML",
        );
        assert!(matches!(&field(&out, "x").owner, Owner::Unknown { hint: Some(h) } if h == "such"));
        assert!(
            matches!(&field(&out, "timeline-offset").owner, Owner::Unknown { hint: Some(h) } if h == "some video files")
        );
    }

    #[test]
    fn parameter_dfns_are_skipped() {
        let out = run(
            r#"<p>To <dfn id="navigate">navigate</dfn> with <dfn data-dfn-for="navigate" id="p"><var>documentResource</var></dfn>:</p><ol><li>Return.</li></ol>"#,
            "HTML",
        );
        assert!(out.fields.is_empty());
    }

    #[test]
    fn bikeshed_dfn_for_owner_and_concept_alias() {
        let out = run(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> {};
      interface <dfn data-dfn-type="interface" id="interface-element">Element</dfn> : Node {};</pre>
      <p>A <dfn data-dfn-type="dfn" id="concept-node">node</dfn> is …</p>
      <p>Each <a href="#concept-node">node</a> has an associated <dfn data-dfn-for="Node" data-dfn-type="dfn" id="concept-node-document">node document</dfn>, set upon creation, that is a <a href="#concept-document">document</a>.</p>"##,
            "DOM",
        );
        let f = field(&out, "concept-node-document");
        assert_eq!(owner_keys(f), ["idl:Node"]);
        assert!(matches!(
            &f.owner,
            Owner::Known {
                basis: OwnerBasis::DfnFor,
                ..
            }
        ));
        assert!(!out
            .issues
            .iter()
            .any(|i| i.code == StateIssueCode::OwnerConflict));
    }

    #[test]
    fn algorithm_dfns_with_dfn_for_are_not_fields() {
        let out = run(
            r##"<div class="algorithm" data-algorithm="length"><p>To determine the <dfn data-dfn-for="Node" data-dfn-type="dfn" id="concept-node-length">length</dfn> of a <a href="#concept-node">node</a> <var>node</var>, run these steps:</p><ol><li>Return 0.</li></ol></div>
      <div class="algorithm" data-algorithm="equals"><p>A <a href="#concept-node">node</a> <var>A</var> <dfn data-dfn-for="Node" data-dfn-type="dfn" id="concept-node-equals">equals</dfn> a node <var>B</var> if all of the following conditions are true:</p><ul><li>x</li></ul></div>
      <p>The <dfn data-dfn-for="Node" data-dfn-type="dfn" id="orphan">orphan thing</dfn> is described elsewhere.</p>"##,
            "DOM",
        );
        assert!(out
            .fields
            .iter()
            .all(|f| f.anchor.anchor != "concept-node-length"
                && f.anchor.anchor != "concept-node-equals"));
        assert_eq!(field(&out, "orphan").field_basis, FieldBasis::DfnForOnly);
    }

    #[test]
    fn dom_window_dfn_for_resolves_to_a_provisional_idl_key() {
        let out = run(
            r#"<p>Each <code>Window</code> object has an associated <dfn data-dfn-for="Window" data-dfn-type="dfn" id="w">current event</dfn>.</p>"#,
            "DOM",
        );
        assert!(
            matches!(&field(&out, "w").owner, Owner::Known { types, .. } if types[0].key == TypeKey::Idl("Window".into()) && types[0].via == OwnerVia::IdlName)
        );
    }
}
