//! Field and owner inference (§6.3): R1 `data-dfn-for` and R2 declaration sentences.
use crate::parse::idl_defs::normalize_owner;
use crate::parse::sections::{is_inside_algorithm_content, is_var_only};
use crate::parse::steps::StructuralSpec;
use crate::state::block::{
    flatten, innermost_block, list_item, norm, pattern, plain_text, sentences, BlockToken,
};
use crate::state::catalog::{StateCatalog, TypeDeclaration};
use crate::state::model::{
    AnchorTarget, DeclarationSite, FieldBasis, FieldDef, ModelIssue, Owner, OwnerBasis, OwnerRef,
    OwnerVia, SetMember, StateIssueCode, TypeExpr, TypeKey, TypeKind, TypeRef,
};
use crate::state::typeexpr::{
    declared_type, initial_value, locate_dfn_in_pat, sibling_dd_text, substitute_text_tokens,
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
/// containing any of these words is the subject of an auxiliary construction, not a declaration.
const AUXILIARIES: [&str; 13] = [
    "can", "cannot", "could", "may", "might", "must", "shall", "should", "will", "would", "does",
    "do", "did",
];

fn regex(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).unwrap())
}

/// Result of applying R3/R4/R5 to the list the dfn lives in (§6.3).
enum ListCandidate<'a> {
    /// R3: the intro paragraph matches `R2` — the dfn is a property of the named owner(s).
    Property {
        owners: Vec<OwnerPhrase>,
        intro_initial: Option<String>,
    },
    /// R4: the intro links to `INFRA#struct` or says "following items" and contains a dfn.
    Struct {
        owner_dfn: ElementRef<'a>,
        intro_initial: Option<String>,
    },
    /// R5: "A/An D is a set of … the following" — the dfn is a set member, never a field.
    SetOf { set_dfn: ElementRef<'a> },
}

/// True if an element is a spec callout (note/example/warning/advisement) to skip when
/// searching for an intro paragraph.
fn is_list_callout(e: &ElementRef<'_>) -> bool {
    e.value()
        .classes()
        .any(|c| matches!(c, "note" | "example" | "warning" | "advisement"))
}

/// Extract `initially <word>` or `all initially <word>` from an intro plain-text string.
fn extract_intro_initial(intro_text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(&RE, r"(?:all )?initially (\w+)")
        .captures(intro_text)
        .map(|caps| caps[1].to_string())
}

/// Apply the R2 pattern to `text` (a single sentence from a pattern string) using `slots`
/// as the token-index table for placeholder resolution. Returns the matched owner phrases.
fn parse_r2_owners(text: &str, slots: &[usize]) -> Option<Vec<OwnerPhrase>> {
    static R2_RE: OnceLock<Regex> = OnceLock::new();
    static R2_ALT_RE: OnceLock<Regex> = OnceLock::new();
    static SEPARATOR: OnceLock<Regex> = OnceLock::new();
    static PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
    let caps = regex(&R2_RE, R2)
        .captures(text)
        .or_else(|| regex(&R2_ALT_RE, R2_ALT).captures(text))?;
    let placeholder = regex(&PLACEHOLDER, r"^⟦[LDC](\d+)⟧$");
    let mut owners = Vec::new();
    for part in regex(&SEPARATOR, r", and |, | and ").split(&caps["owners"]) {
        if let Some(n) = placeholder.captures(part) {
            owners.push(OwnerPhrase::Slot(slots[n[1].parse::<usize>().ok()?]));
        } else if part.split(' ').any(|w| AUXILIARIES.contains(&w)) {
            return None;
        } else {
            owners.push(OwnerPhrase::Words(part.to_string()));
        }
    }
    Some(owners)
}

/// Try to identify the intro paragraph before a list, apply R5/R4/R3, and return the candidate.
fn list_candidate<'a>(
    dfn: &ElementRef<'a>,
    spec: &str,
    base_url: &str,
) -> Option<(ListCandidate<'a>, Vec<BlockToken>)> {
    let (_item, list) = list_item(dfn)?;

    // Walk backwards from the list, skipping whitespace text nodes, comments and callouts.
    let intro_el = {
        let mut found: Option<ElementRef<'a>> = None;
        let mut cursor = list.prev_sibling();
        while let Some(node) = cursor {
            match node.value() {
                Node::Text(t) if t.trim().is_empty() => {}
                Node::Comment(_) => {}
                Node::Element(_) => {
                    let el = ElementRef::wrap(node).expect("element node wraps");
                    if is_list_callout(&el) {
                        // skip callouts, keep looking
                    } else {
                        found = Some(el);
                        break;
                    }
                }
                _ => break,
            }
            cursor = node.prev_sibling();
        }
        found?
    };

    if intro_el.value().name() != "p" {
        return None;
    }

    let intro_tokens = flatten(&intro_el, spec, base_url);
    let intro_pat = pattern(&intro_tokens);

    if !intro_pat.text.ends_with(':') {
        return None;
    }

    let intro_sents = sentences(&intro_pat);
    let last_sent = intro_sents.last()?;
    let last_sent_text = &intro_pat.text[last_sent.start..];

    // Build a map from dfn id → ElementRef for dfns inside the intro paragraph.
    let intro_dfn_map: HashMap<&str, ElementRef<'a>> = {
        static SEL: OnceLock<Selector> = OnceLock::new();
        let sel = SEL.get_or_init(|| Selector::parse("dfn[id]").unwrap());
        intro_el
            .select(sel)
            .filter_map(|d| d.value().attr("id").map(|id| (id, d)))
            .collect()
    };

    let intro_text = plain_text(&intro_tokens);

    // R5: "A/An ⟦D(N)⟧ is a set of (zero or more of )the following"
    {
        static R5_RE: OnceLock<Regex> = OnceLock::new();
        let r5 = regex(
            &R5_RE,
            r"^(?:A|An) ⟦D(\d+)⟧ is a set of (?:zero or more of )?the following ",
        );
        if let Some(caps) = r5.captures(last_sent_text) {
            let slot_idx: usize = caps[1].parse().ok()?;
            let token_idx = *intro_pat.slots.get(slot_idx)?;
            if let BlockToken::Dfn { id, .. } = &intro_tokens[token_idx] {
                if let Some(&set_dfn) = intro_dfn_map.get(id.as_str()) {
                    return Some((ListCandidate::SetOf { set_dfn }, intro_tokens));
                }
            }
            return None;
        }
    }

    // R4: intro links to INFRA#struct, or contains "following items", and has a dfn.
    {
        let has_struct_link = intro_tokens.iter().any(|t| {
            matches!(t, BlockToken::Link { target: Some(target), .. }
                if target.spec == "INFRA" && target.anchor == "struct")
        });
        let has_following_items = intro_text.contains("following items");
        if has_struct_link || has_following_items {
            // Find the first dfn in document order from the intro tokens.
            let owner_dfn = intro_tokens.iter().find_map(|t| {
                if let BlockToken::Dfn { id, .. } = t {
                    intro_dfn_map.get(id.as_str()).copied()
                } else {
                    None
                }
            });
            if let Some(owner_dfn) = owner_dfn {
                let intro_initial = extract_intro_initial(&intro_text);
                return Some((
                    ListCandidate::Struct {
                        owner_dfn,
                        intro_initial,
                    },
                    intro_tokens,
                ));
            }
        }
    }

    // R3: last sentence matches R2.
    if let Some(owners) = parse_r2_owners(last_sent_text, &intro_pat.slots) {
        let intro_initial = extract_intro_initial(&intro_text);
        return Some((
            ListCandidate::Property {
                owners,
                intro_initial,
            },
            intro_tokens,
        ));
    }

    None
}

fn is_concept_dfn(dfn: &ElementRef<'_>) -> bool {
    let v = dfn.value();
    v.name() == "dfn"
        && v.attr("id").is_some()
        && matches!(v.attr("data-dfn-type"), None | Some("dfn"))
        && !is_var_only(dfn)
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

/// The dfn text with `<code>` runs in backticks, as markdown renders it.
fn dfn_display_name(dfn: &ElementRef<'_>) -> String {
    fn walk(node: ego_tree::NodeRef<'_, Node>, out: &mut String) {
        match node.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) if element.name() == "code" => {
                let code = ElementRef::wrap(node).map(|e| e.text().collect::<String>());
                out.push('`');
                out.push_str(code.as_deref().unwrap_or_default().trim());
                out.push('`');
            }
            Node::Element(_) => node.children().for_each(|child| walk(child, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    dfn.children().for_each(|child| walk(child, &mut out));
    norm(&out)
}

/// Concept dfns: `dfn[id]` without `data-dfn-type` or with `"dfn"`, not parameters, not in `<pre>`.
pub(crate) fn concept_dfns(document: &Html) -> Vec<ConceptDfn> {
    static SEL: OnceLock<Selector> = OnceLock::new();
    let sel = SEL.get_or_init(|| Selector::parse("dfn[id]").unwrap());
    document
        .select(sel)
        .filter(is_concept_dfn)
        .map(|dfn| {
            let (_, names) = dfn_names(&dfn);
            ConceptDfn {
                id: dfn.value().attr("id").unwrap_or_default().to_string(),
                names,
            }
        })
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct DeclareOutput {
    pub fields: Vec<FieldDef>,
    pub members: Vec<SetMember>,
    pub issues: Vec<ModelIssue>,
    pub counters: DeclareCounters,
    /// Plain text of the innermost block of every concept dfn outside algorithm steps, by dfn id:
    /// what a reviewed declaration's `expect_text` is checked against.
    pub blocks: HashMap<String, String>,
    /// The `owner_by_rule` key each entry of `fields` was counted under.
    field_rules: Vec<&'static str>,
}

#[derive(Debug, Default)]
pub(crate) struct DeclareCounters {
    pub concept_dfns: u32,
    pub owner_by_rule: BTreeMap<String, u32>,
    pub owner_candidates: u32,
    pub owner_resolved: u32,
    pub set_members: u32,
}

/// Names bound by reviewed type declarations (§8.2 `name`).
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
///
/// Delegates to [`parse_r2_owners`] after locating the sentence and slicing the text to
/// the dfn position, so there is exactly one owner-phrase parser in this module.
fn declaration_owners(tokens: &[BlockToken], dfn_id: &str) -> Option<Vec<OwnerPhrase>> {
    let pat = pattern(tokens);
    let slot = pat
        .slots
        .iter()
        .position(|&i| matches!(&tokens[i], BlockToken::Dfn { id, .. } if id == dfn_id))?;
    let dfn_at = pat.text.find(&format!("⟦D{slot}⟧"))?;
    let sentence = sentences(&pat)
        .into_iter()
        .find(|range| range.contains(&dfn_at))?;
    // Pass only the text from the sentence start up to the dfn so that R2 must end
    // before the dfn — equivalent to the old explicit end-position check.
    let text_before_dfn = &pat.text[sentence.start..dfn_at];
    parse_r2_owners(text_before_dfn, &pat.slots)
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

pub(crate) fn rule_key(basis: &OwnerBasis) -> &'static str {
    match basis {
        OwnerBasis::DfnFor => "dfn_for",
        OwnerBasis::DeclarationSentence => "declaration_sentence",
        OwnerBasis::PropertyList => "property_list",
        OwnerBasis::StructItems => "struct_items",
        OwnerBasis::Override { .. } => "override",
    }
}

/// Infers fields and their owners from concept dfns outside algorithm steps (§6.3).
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
        let block_text = plain_text(&tokens);
        out.blocks.insert(id.to_string(), block_text.clone());
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

        // R5 check (set member) → always wins; R3/R4 pass through for field handling.
        let list_r34 = match list_candidate(dfn, spec, base_url) {
            Some((ListCandidate::SetOf { set_dfn }, _)) => {
                let set_id = set_dfn.value().attr("id").unwrap_or_default();
                let set_name = norm(&set_dfn.text().collect::<String>());
                let set_key =
                    resolver
                        .table
                        .add_concept(spec, set_id, &set_name, TypeKind::Concept);
                let (name, names) = dfn_names(dfn);
                out.members.push(SetMember {
                    anchor: AnchorTarget {
                        spec: spec.to_string(),
                        anchor: id.to_string(),
                    },
                    name,
                    names,
                    set: set_key,
                    declaration: DeclarationSite {
                        section_anchor: section_anchor.clone(),
                        text: block_text,
                    },
                });
                out.counters.set_members += 1;
                continue;
            }
            other => other,
        };

        // R4: always register the InfraStruct type even when R1 wins the field's owner.
        if let Some((ListCandidate::Struct { owner_dfn, .. }, _)) = &list_r34 {
            let struct_id = owner_dfn.value().attr("id").unwrap_or_default();
            let struct_name = norm(&owner_dfn.text().collect::<String>());
            resolver
                .table
                .add_concept(spec, struct_id, &struct_name, TypeKind::InfraStruct);
        }

        // Capture intro_initial before list_r34 is consumed by the owner match.
        let intro_initial: Option<String> = match &list_r34 {
            Some((ListCandidate::Property { intro_initial, .. }, _)) => intro_initial.clone(),
            Some((ListCandidate::Struct { intro_initial, .. }, _)) => intro_initial.clone(),
            _ => None,
        };

        let has_list = list_r34.is_some();

        let (owner, rule) = match (r1, &declared, list_r34) {
            // R1 wins over R2/R3/R4 for the field owner.
            (Some((name, r1)), _, _) => {
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
            // No R1: R4 (struct items) owns the field.
            (None, _, Some((ListCandidate::Struct { owner_dfn, .. }, _))) => {
                let struct_id = owner_dfn.value().attr("id").unwrap_or_default();
                let key = resolver
                    .table
                    .by_anchor
                    .get(struct_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        TypeKey::Anchor(AnchorTarget {
                            spec: spec.to_string(),
                            anchor: struct_id.to_string(),
                        })
                    });
                let owner = Owner::Known {
                    types: vec![OwnerRef {
                        key,
                        via: OwnerVia::DfnName,
                    }],
                    basis: OwnerBasis::StructItems,
                };
                (owner, OwnerBasis::StructItems)
            }
            // No R1: R3 (property list) owns the field.
            (None, _, Some((ListCandidate::Property { owners, .. }, intro_tokens))) => {
                let owner = resolver.owner(&owners, &intro_tokens, OwnerBasis::PropertyList);
                (owner, OwnerBasis::PropertyList)
            }
            // No R1, no list: R2 (declaration sentence).
            (None, Some(owner), None) => (owner.clone(), OwnerBasis::DeclarationSentence),
            // No owner source at all.
            (None, None, None) => continue,
            // SetOf was already handled above with `continue`; this arm is unreachable.
            (None, _, Some((ListCandidate::SetOf { .. }, _))) => unreachable!(),
        };

        out.counters.owner_candidates += 1;
        *out.counters
            .owner_by_rule
            .entry(rule_key(&rule).to_string())
            .or_default() += 1;
        if matches!(owner, Owner::Known { .. }) {
            out.counters.owner_resolved += 1;
        }

        let field_basis = if declared.is_some() || has_list {
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

        // §6.4: compute declared type and initial value.
        let dd_text = sibling_dd_text(dfn, spec, base_url);
        let pat = pattern(&tokens);
        let (declared_type_val, initial_val) = {
            // Immutable snapshot of the type table for the resolve closure.
            let by_anchor = &resolver.table.by_anchor;
            let spec_str: &str = resolver.spec;
            let resolve_fn = |target: &AnchorTarget| -> TypeRef {
                if target.spec == spec_str {
                    if let Some(key) = by_anchor.get(&target.anchor) {
                        return TypeRef::Known(key.clone());
                    }
                }
                TypeRef::Unresolved(target.clone())
            };
            if let Some((dfn_slot, clause, next_sent)) = locate_dfn_in_pat(&tokens, &pat, id) {
                let ty = declared_type(
                    &tokens,
                    &pat,
                    dfn_slot,
                    clause.clone(),
                    dd_text.as_deref(),
                    &resolve_fn,
                );
                let clause_text = substitute_text_tokens(&pat.text[clause], &pat, &tokens);
                let next_sent_text = next_sent.map(|s| substitute_text_tokens(&s, &pat, &tokens));
                let init = initial_value(
                    &clause_text,
                    next_sent_text.as_deref(),
                    intro_initial.as_deref(),
                );
                (ty, init)
            } else {
                // Dfn not found in block tokens; still honour intro_initial.
                let init = initial_value("", None, intro_initial.as_deref());
                (TypeExpr::Unknown, init)
            }
        };

        let (_, names) = dfn_names(dfn);
        let name = dfn_display_name(dfn);
        out.fields.push(FieldDef {
            anchor: AnchorTarget {
                spec: spec.to_string(),
                anchor: id.to_string(),
            },
            name,
            names,
            owner,
            field_basis,
            declared_type: declared_type_val,
            initial: initial_val,
            declaration: Some(DeclarationSite {
                section_anchor: section_anchor.clone(),
                text: block_text,
            }),
        });
        out.field_rules.push(rule_key(&rule));
    }

    rewrite_owner_keys(&mut out, resolver.table, spec);
    out
}

/// Rewrite same-spec `TypeKey::Anchor` owner keys through `table.by_anchor`: an alias can
/// remove a Concept entry and reroute its anchor to another key after fields named it.
fn rewrite_owner_keys(out: &mut DeclareOutput, table: &TypeTable, spec: &str) {
    for field in &mut out.fields {
        if let Owner::Known { types, .. } = &mut field.owner {
            let mut seen = HashSet::new();
            types.retain_mut(|owner_ref| {
                if let TypeKey::Anchor(ref target) = owner_ref.key {
                    if target.spec == spec {
                        if let Some(new_key) = table.by_anchor.get(&target.anchor).cloned() {
                            owner_ref.key = new_key;
                        }
                    }
                }
                seen.insert(owner_ref.key.clone())
            });
        }
    }
    for member in &mut out.members {
        if let TypeKey::Anchor(ref target) = member.set {
            if target.spec == spec {
                if let Some(new_key) = table.by_anchor.get(&target.anchor).cloned() {
                    member.set = new_key;
                }
            }
        }
    }
}

fn is_heading(element: &ElementRef<'_>) -> bool {
    matches!(
        element.value().name(),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
    )
}

/// What a type declaration's `expect_text` is checked against: the anchor's block text, or
/// (for a heading) the blocks of the fields owned through the declared name.
enum TypeAnchorText {
    Block(String),
    Heading,
}

/// One walk over the document for the anchors of this spec's type declarations.
fn type_anchor_texts(
    document: &Html,
    spec: &str,
    base_url: &str,
    anchors: &HashSet<&str>,
) -> HashMap<String, TypeAnchorText> {
    let mut out = HashMap::new();
    for el in document
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        let Some(id) = el.value().attr("id").filter(|id| anchors.contains(id)) else {
            continue;
        };
        if out.contains_key(id) {
            continue;
        }
        let text = if is_heading(&el) {
            TypeAnchorText::Heading
        } else {
            let block = innermost_block(&el).unwrap_or(el);
            TypeAnchorText::Block(plain_text(&flatten(&block, spec, base_url)))
        };
        out.insert(id.to_string(), text);
    }
    out
}

fn mismatch(anchor: &str, public_id: &str, what: &str) -> ModelIssue {
    ModelIssue {
        code: StateIssueCode::DeclarationMismatch,
        anchor: Some(anchor.to_string()),
        message: format!(
            "declaration {public_id}: expect_text does not match {what}; declaration ignored"
        ),
    }
}

fn is_owned_via_declaration(field: &FieldDef, key: &TypeKey) -> bool {
    matches!(&field.owner, Owner::Known { types, .. }
        if types.iter().any(|t| t.via == OwnerVia::Declaration && t.key == *key))
}

fn resolve_key(table: &TypeTable, spec: &str, key: &TypeKey) -> TypeKey {
    match key {
        TypeKey::Anchor(target) if target.spec == spec => table
            .by_anchor
            .get(&target.anchor)
            .cloned()
            .unwrap_or_else(|| key.clone()),
        _ => key.clone(),
    }
}

/// Resolves same-spec `{nominal: SPEC#anchor}` references the catalog loader leaves unresolved.
fn resolve_type(ty: &TypeExpr, table: &TypeTable, spec: &str) -> TypeExpr {
    match ty {
        TypeExpr::Nominal {
            ty: TypeRef::Unresolved(target),
            text,
        } if target.spec == spec => match table.by_anchor.get(&target.anchor) {
            Some(key) => TypeExpr::Nominal {
                ty: TypeRef::Known(key.clone()),
                text: text.clone(),
            },
            None => ty.clone(),
        },
        TypeExpr::Infra { kind, args } => TypeExpr::Infra {
            kind: *kind,
            args: args.iter().map(|a| resolve_type(a, table, spec)).collect(),
        },
        TypeExpr::Union(members) => TypeExpr::Union(
            members
                .iter()
                .map(|m| resolve_type(m, table, spec))
                .collect(),
        ),
        _ => ty.clone(),
    }
}

/// `declare_fields` with the reviewed declarations of `catalog` that belong to `spec` (§8.2).
/// A type declaration whose text matches binds its name before owner resolution; a heading
/// anchor is checked against the fields owned through that name afterwards, and its binding is
/// withdrawn on mismatch. Field declarations then override owner, type and initial value.
pub(crate) fn declare_with_catalog(
    document: &Html,
    spec: &str,
    base_url: &str,
    structure: &StructuralSpec,
    table: &mut TypeTable,
    catalog: &StateCatalog,
) -> DeclareOutput {
    let type_decls: Vec<&TypeDeclaration> =
        catalog.types.iter().filter(|d| d.ty.spec == spec).collect();
    let mut issues = Vec::new();
    let mut bindings = NameBindings::default();
    let mut deferred = Vec::new();
    if !type_decls.is_empty() {
        let anchors: HashSet<&str> = type_decls.iter().map(|d| d.ty.anchor.as_str()).collect();
        let texts = type_anchor_texts(document, spec, base_url, &anchors);
        for decl in type_decls {
            let anchor = decl.ty.anchor.as_str();
            let key = match texts.get(anchor) {
                None => {
                    issues.push(mismatch(anchor, &decl.public_id, "a missing anchor"));
                    continue;
                }
                Some(TypeAnchorText::Heading) => {
                    let key = table.declared_key(decl);
                    deferred.push((decl, key.clone()));
                    key
                }
                Some(TypeAnchorText::Block(text)) if decl.expect_text.regex().is_match(text) => {
                    table.apply_type_declaration(spec, decl)
                }
                Some(TypeAnchorText::Block(_)) => {
                    issues.push(mismatch(anchor, &decl.public_id, "the anchor's block"));
                    continue;
                }
            };
            if let Some(name) = &decl.name {
                bindings.names.insert(name.clone(), key);
            }
        }
    }

    let mut out = declare_fields(document, spec, base_url, structure, table, &bindings);

    let mut aliased = false;
    for (decl, key) in deferred {
        let matched = decl.name.is_some()
            && out.fields.iter().any(|f| {
                is_owned_via_declaration(f, &key)
                    && out
                        .blocks
                        .get(&f.anchor.anchor)
                        .is_some_and(|text| decl.expect_text.regex().is_match(text))
            });
        if matched {
            table.apply_type_declaration(spec, decl);
            aliased |= decl.alias_of.is_some();
            continue;
        }
        issues.push(mismatch(
            &decl.ty.anchor,
            &decl.public_id,
            "any field owned through its name",
        ));
        withdraw_binding(&mut out, &key);
    }
    if aliased {
        rewrite_owner_keys(&mut out, table, spec);
    }

    apply_field_declarations(document, spec, table, catalog, &mut out, &mut issues);
    out.issues.extend(issues);
    out
}

/// Drop owner refs a withdrawn name binding produced; a field left without owners is unknown.
fn withdraw_binding(out: &mut DeclareOutput, key: &TypeKey) {
    for field in &mut out.fields {
        let Owner::Known { types, .. } = &mut field.owner else {
            continue;
        };
        types.retain(|t| !(t.via == OwnerVia::Declaration && t.key == *key));
        if types.is_empty() {
            field.owner = Owner::Unknown { hint: None };
            out.counters.owner_resolved -= 1;
            out.issues.push(ModelIssue {
                code: StateIssueCode::OwnerUnknown,
                anchor: Some(field.anchor.anchor.clone()),
                message: format!("owner of {} not inferred", field.anchor.anchor),
            });
        }
    }
}

fn apply_field_declarations(
    document: &Html,
    spec: &str,
    table: &TypeTable,
    catalog: &StateCatalog,
    out: &mut DeclareOutput,
    issues: &mut Vec<ModelIssue>,
) {
    let mut located: Option<Vec<(ElementRef<'_>, String)>> = None;
    for decl in catalog.fields.iter().filter(|d| d.field.spec == spec) {
        let anchor = decl.field.anchor.as_str();
        let Some(block) = out
            .blocks
            .get(anchor)
            .filter(|text| decl.expect_text.regex().is_match(text))
        else {
            issues.push(mismatch(anchor, &decl.public_id, "the field's block"));
            continue;
        };
        let index = match out.fields.iter().position(|f| f.anchor.anchor == anchor) {
            Some(index) => index,
            None => {
                let dfns = located.get_or_insert_with(|| located_concept_dfns(document));
                let Some((dfn, section_anchor)) = dfns
                    .iter()
                    .find(|(dfn, _)| dfn.value().attr("id") == Some(anchor))
                else {
                    continue;
                };
                let (_, names) = dfn_names(dfn);
                out.fields.push(FieldDef {
                    anchor: decl.field.clone(),
                    name: dfn_display_name(dfn),
                    names,
                    owner: Owner::Unknown { hint: None },
                    field_basis: FieldBasis::Declared,
                    declared_type: TypeExpr::Unknown,
                    initial: None,
                    declaration: Some(DeclarationSite {
                        section_anchor: section_anchor.clone(),
                        text: block.clone(),
                    }),
                });
                out.field_rules.push("");
                out.fields.len() - 1
            }
        };
        let field = &mut out.fields[index];
        if let Some(owners) = &decl.owner {
            let counters = &mut out.counters;
            match out.field_rules[index] {
                "" => counters.owner_candidates += 1,
                previous => {
                    if let Some(count) = counters.owner_by_rule.get_mut(previous) {
                        *count -= 1;
                    }
                }
            }
            if !matches!(field.owner, Owner::Known { .. }) {
                counters.owner_resolved += 1;
            }
            let rule = OwnerBasis::Override {
                rule_id: decl.public_id.clone(),
            };
            *counters
                .owner_by_rule
                .entry(rule_key(&rule).to_string())
                .or_default() += 1;
            out.field_rules[index] = rule_key(&rule);
            let mut types: Vec<OwnerRef> = Vec::new();
            for owner in owners {
                let key = resolve_key(table, spec, owner);
                if !types.iter().any(|t| t.key == key) {
                    types.push(OwnerRef {
                        key,
                        via: OwnerVia::Declaration,
                    });
                }
            }
            field.owner = Owner::Known { types, basis: rule };
            out.issues.retain(|i| {
                !(i.code == StateIssueCode::OwnerUnknown && i.anchor.as_deref() == Some(anchor))
            });
        }
        if let Some(ty) = &decl.ty {
            field.declared_type = resolve_type(ty, table, spec);
        }
        if let Some(initial) = &decl.initial {
            field.initial = Some(initial.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::model::{
        AnchorRole, ConceptAliasBasis, InitialValue, Literal, Primitive, TypeRef,
    };
    use scraper::Html;

    fn run(html: &str, spec: &str) -> DeclareOutput {
        run_with_table(html, spec).0
    }

    fn run_declare(html: &str, spec: &str) -> DeclareOutput {
        run(html, spec)
    }

    fn run_with_table(html: &str, spec: &str) -> (DeclareOutput, TypeTable) {
        let doc = Html::parse_document(html);
        let base = if spec == "DOM" {
            "https://dom.spec.whatwg.org/"
        } else {
            "https://html.spec.whatwg.org/"
        };
        let idl = crate::parse::idl_defs::extract_idl_definitions(&doc);
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
        assert_eq!(f.name, "is initial `about:blank`");
        assert_eq!(f.names, ["is initial about:blank"]);
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
    fn initially_undefined_is_not_null() {
        let out = run(
            r##"<h4 id="the-img-element">img</h4><dl class="element"><dt>DOM interface:</dt><dd><pre><code class="idl">interface <dfn data-dfn-type="interface" id="htmlimageelement">HTMLImageElement</dfn> {};</code></pre></dd></dl>
      <p>Each <a href="#the-img-element">img</a> element has associated <dfn id="u">pending thing</dfn>, initially undefined.</p>"##,
            "HTML",
        );
        assert!(matches!(
            &field(&out, "u").initial,
            Some(InitialValue::Literal {
                value: Literal::Undefined,
                ..
            })
        ));
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

    // A6: R3 property lists, R4 struct items, R5 set members.

    #[test]
    fn property_list_items_are_owned_by_the_intro_owner() {
        let out = run(
            r##"<p>A <dfn id="navigable">navigable</dfn> presents a document. Each navigable has:</p><ul>
      <li><p>An <dfn id="nav-id">id</dfn>, a new unique internal value.</p></li>
      <li><p>A <dfn data-dfn-for="navigable" id="nav-parent">parent</dfn>, a <a href="#navigable">navigable</a> or null.</p></li></ul>"##,
            "HTML",
        );
        assert_eq!(owner_keys(field(&out, "nav-id")), ["HTML#navigable"]);
        assert!(matches!(
            &field(&out, "nav-id").owner,
            Owner::Known {
                basis: OwnerBasis::PropertyList,
                ..
            }
        ));
        assert!(matches!(
            &field(&out, "nav-parent").owner,
            Owner::Known {
                basis: OwnerBasis::DfnFor,
                ..
            }
        ));
        assert_eq!(field(&out, "nav-parent").field_basis, FieldBasis::Declared);
    }

    #[test]
    fn struct_items_make_an_infra_struct_owner() {
        let doc_html = r##"<p>The <dfn id="dlti">document load timing info</dfn> <a href="https://infra.spec.whatwg.org/#struct">struct</a> has the following items:</p><dl>
      <dt><dfn data-dfn-for="document load timing info" id="nst">navigation start time</dfn> (default 0)</dt><dd>A number</dd></dl>"##;
        let (out, table) = run_with_table(doc_html, "HTML");
        assert_eq!(owner_keys(field(&out, "nst")), ["HTML#dlti"]);
        // R1 creates dlti as Concept first; R4 must upgrade it to InfraStruct.
        let dlti_key = TypeKey::Anchor(AnchorTarget {
            spec: "HTML".into(),
            anchor: "dlti".into(),
        });
        assert_eq!(table.types[&dlti_key].kind, TypeKind::InfraStruct);
    }

    #[test]
    fn event_flags_list_is_owned_by_event() {
        let out = run(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="event">Event</dfn> {};</pre>
      <p>An <dfn data-dfn-type="dfn" id="concept-event">event</dfn> is …</p>
      <p>Each <a href="#concept-event">event</a> has the following associated flags that are all initially unset:</p>
      <ul><li><dfn data-dfn-for="Event" data-dfn-type="dfn" id="stop-propagation-flag">stop propagation flag</dfn></li>
      <li><dfn data-dfn-for="Event" data-dfn-type="dfn" id="canceled-flag">canceled flag</dfn></li></ul>"##,
            "DOM",
        );
        assert_eq!(
            owner_keys(field(&out, "stop-propagation-flag")),
            ["idl:Event"]
        );
        assert_eq!(owner_keys(field(&out, "canceled-flag")), ["idl:Event"]);
    }

    #[test]
    fn sandboxing_flags_are_set_members_not_fields() {
        let out = run(
            r##"<p>A <dfn id="sfs">sandboxing flag set</dfn> is a set of zero or more of the following flags, which are used to restrict abilities:</p>
      <dl><dt>The <dfn id="snf">sandboxed navigation browsing context flag</dfn></dt><dd><p>This flag prevents content from navigating.</p></dd></dl>"##,
            "HTML",
        );
        assert!(out.fields.iter().all(|f| f.anchor.anchor != "snf"));
        let m = out
            .members
            .iter()
            .find(|m| m.anchor.anchor == "snf")
            .unwrap();
        assert_eq!(m.set.to_string(), "HTML#sfs");
    }

    #[test]
    fn owner_key_rewritten_after_concept_aliases_to_idl() {
        // tid's R2 owner resolves to concept-thing (TypeKey::Anchor(HTML#concept-thing)).
        // ttype comes later and aliases concept-thing to idl:FancyType via data-dfn-for.
        // Post-processing must rewrite tid's owner to idl:FancyType, and every owner key
        // must have a TypeDef in the table.
        let out = run(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="fancytype-iface">FancyType</dfn> {};</pre>
      <p>A <dfn data-dfn-type="dfn" id="concept-thing">document thing</dfn> is …</p>
      <p>Each <a href="#concept-thing">document thing</a> has a <dfn id="tid">thing id</dfn>.</p>
      <p>A <a href="#concept-thing">document thing</a> has an associated <dfn data-dfn-for="FancyType" data-dfn-type="dfn" id="ttype">type</dfn>.</p>"##,
            "HTML",
        );
        assert_eq!(owner_keys(field(&out, "tid")), ["idl:FancyType"]);
        let (_, table) = run_with_table(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="fancytype-iface">FancyType</dfn> {};</pre>
      <p>A <dfn data-dfn-type="dfn" id="concept-thing">document thing</dfn> is …</p>
      <p>Each <a href="#concept-thing">document thing</a> has a <dfn id="tid">thing id</dfn>.</p>
      <p>A <a href="#concept-thing">document thing</a> has an associated <dfn data-dfn-for="FancyType" data-dfn-type="dfn" id="ttype">type</dfn>.</p>"##,
            "HTML",
        );
        for f in &out.fields {
            if let Owner::Known { types, .. } = &f.owner {
                for t in types {
                    assert!(
                        table.types.contains_key(&t.key),
                        "no TypeDef for {:?} (field {})",
                        t.key,
                        f.anchor.anchor
                    );
                }
            }
        }
    }

    // A7: declared type and initial value.

    #[test]
    fn post_dfn_boolean_and_initial_false() {
        let out = run_declare(
            r##"<p>Each <code><a href="#document">Document</a></code> has an <dfn id="f">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p><pre><code class="idl">partial interface <dfn id="document">Document</dfn> {};</code></pre>"##,
            "HTML",
        );
        let f = field(&out, "f");
        assert_eq!(f.declared_type, TypeExpr::Primitive(Primitive::Boolean));
        assert!(matches!(
            &f.initial,
            Some(InitialValue::Literal {
                value: Literal::Bool(false),
                ..
            })
        ));
    }

    #[test]
    fn union_with_null_and_nominal() {
        use crate::state::typeexpr::parse_type_phrase;
        let links = vec![(
            "navigable".to_string(),
            TypeRef::Known(TypeKey::parse("HTML#navigable").unwrap()),
        )];
        let ty = parse_type_phrase("⟦L0⟧ or null", &links);
        assert!(matches!(ty, TypeExpr::Union(ref v) if v.len() == 2 && v[1] == TypeExpr::Null));
    }

    #[test]
    fn struct_item_default_and_dd_type() {
        let out = run_declare(
            r##"<p>The <dfn id="dlti">document load timing info</dfn> <a href="https://infra.spec.whatwg.org/#struct">struct</a> has the following items:</p><dl><dt><dfn data-dfn-for="document load timing info" id="nst">navigation start time</dfn> (default 0)</dt><dd>A number</dd></dl>"##,
            "HTML",
        );
        let f = field(&out, "nst");
        assert_eq!(f.declared_type, TypeExpr::Primitive(Primitive::Number));
        assert!(
            matches!(&f.initial, Some(InitialValue::Literal { value: Literal::Number(n), .. }) if n == "0")
        );
    }

    #[test]
    fn intro_initial_unset_applies_to_every_item_and_next_sentence_initial() {
        let out = run_declare(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="event">Event</dfn> {};</pre><p>An <dfn data-dfn-type="dfn" id="concept-event">event</dfn> is …</p>
      <p>Each <a href="#concept-event">event</a> has the following associated flags that are all initially unset:</p><ul><li><dfn data-dfn-for="Event" data-dfn-type="dfn" id="spf">stop propagation flag</dfn></li></ul>
      <p>Each <code><a href="#nodeiterator">NodeIterator</a></code> object has an associated boolean <dfn data-dfn-for="NodeIterator" data-dfn-type="dfn" id="active">is active</dfn> to avoid recursive invocations. It is initially false.</p>"##,
            "DOM",
        );
        assert!(matches!(
            field(&out, "spf").initial,
            Some(InitialValue::Unset { .. })
        ));
        assert_eq!(
            field(&out, "active").declared_type,
            TypeExpr::Primitive(Primitive::Boolean)
        );
        assert!(matches!(
            &field(&out, "active").initial,
            Some(InitialValue::Literal {
                value: Literal::Bool(false),
                ..
            })
        ));
    }

    #[test]
    fn set_upon_creation_is_opaque_and_nominal_keeps_text() {
        let out = run_declare(
            r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> {}; interface <dfn data-dfn-type="interface" id="interface-document">Document</dfn> : Node {};</pre>
      <p>A <dfn data-dfn-type="dfn" id="concept-node">node</dfn>. A <dfn data-dfn-type="dfn" id="concept-document">document</dfn>.</p>
      <p>Each <a href="#concept-node">node</a> has an associated <dfn data-dfn-for="Node" data-dfn-type="dfn" id="nd">node document</dfn>, set upon creation, that is a <a href="#concept-document">document</a>.</p>"##,
            "DOM",
        );
        let f = field(&out, "nd");
        assert!(
            matches!(&f.declared_type, TypeExpr::Nominal { ty: TypeRef::Known(TypeKey::Idl(n)), text } if n == "Document" && text == "document")
        );
        assert!(
            matches!(&f.initial, Some(InitialValue::Opaque { text }) if text == "set upon creation")
        );
    }

    #[test]
    fn which_is_an_article_does_not_bleed_into_type() {
        // Regression for R1 regex: "a|an" with zero-width \s* caused "an foo"
        // to be parsed as article "a" + type "n foo".
        let out = run_declare(
            r##"<pre class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</pre>
<p>Each <code><a href="#document">Document</a></code> has an associated <dfn id="concept-document-coop" data-dfn-for="Document">opener policy</dfn>, which is an <a href="#opener-policy">opener policy</a>, initially a new opener policy.</p>"##,
            "HTML",
        );
        let f = field(&out, "concept-document-coop");
        // The type text must be "opener policy", not "n opener policy".
        let type_text = match &f.declared_type {
            TypeExpr::Nominal { text, .. } => Some(text.as_str()),
            _ => None,
        };
        assert_eq!(
            type_text,
            Some("opener policy"),
            "got {:?}",
            f.declared_type
        );
    }

    #[test]
    fn only_var_only_dfns_are_parameters() {
        let doc = Html::parse_document(
            r#"<p>To <dfn id="alg">frob</dfn> <dfn id="param" data-dfn-for="frob"><var>thing</var></dfn>:</p>
<p>Each thing has a <dfn id="mixed"><var>x</var>'s ancestor list</dfn>.</p>"#,
        );
        let ids: Vec<_> = concept_dfns(&doc).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["alg", "mixed"]);
    }

    #[test]
    fn which_is_serial_list_type_spans_its_commas() {
        let out = run_declare(
            r##"<pre class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</pre>
<p>A <dfn id="navigation-id">navigation ID</dfn> is a string.</p>
<p>Each <code><a href="#document">Document</a></code> has an <dfn id="ongoing-navigation" data-dfn-for="Document">ongoing navigation</dfn>, which is a <a href="#navigation-id">navigation ID</a>, "<code>traversal</code>", or null, initially null.</p>"##,
            "HTML",
        );
        let f = field(&out, "ongoing-navigation");
        let TypeExpr::Union(alts) = &f.declared_type else {
            panic!("got {:?}", f.declared_type);
        };
        assert_eq!(alts.len(), 3, "got {alts:?}");
        assert!(
            matches!(&alts[0], TypeExpr::Nominal { text, .. } if text == "navigation ID"),
            "got {alts:?}"
        );
        assert!(
            matches!(&alts[1], TypeExpr::Enumerated(values) if values == &["traversal"]),
            "got {alts:?}"
        );
        assert!(matches!(&alts[2], TypeExpr::Null), "got {alts:?}");
        assert!(
            matches!(
                &f.initial,
                Some(InitialValue::Literal {
                    value: Literal::Null,
                    ..
                })
            ),
            "got {:?}",
            f.initial
        );
    }

    #[test]
    fn initial_value_quoted_code_element() {
        // Regression: `initially "<code>complete</code>"` was parsed as
        // InitialValue::Opaque with text `"` because the regex stopped at the
        // ⟦C0⟧ placeholder.  substitute_text_tokens pre-expands it so the full
        // quoted string is captured.
        let out = run_declare(
            r##"<pre class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</pre>
<p>Each <code><a href="#document">Document</a></code> has a <dfn id="current-document-readiness" data-dfn-for="Document">current document readiness</dfn>, a string, initially "<code>complete</code>".</p>"##,
            "HTML",
        );
        let f = field(&out, "current-document-readiness");
        assert!(
            matches!(
                &f.initial,
                Some(InitialValue::Literal { value: Literal::String(s), .. }) if s == "complete"
            ),
            "got {:?}",
            f.initial
        );
    }

    #[test]
    fn initial_value_reads_through_links() {
        let out = run_declare(
            r##"<p>A <dfn id="session-history-entry">session history entry</dfn> is a struct with the following items:</p>
<ul><li><p><dfn id="she-scroll-restoration-mode">scroll restoration mode</dfn>, a <a href="#scroll-restoration-mode">scroll restoration mode</a>, initially "<code><a href="#dom-scrollrestoration-auto">auto</a></code>".</p></li>
<li><p><dfn id="she-navigation-api-key">navigation API key</dfn>, which is a string, initially set to the result of <a href="#generating-a-random-uuid">generating a random UUID</a>.</p></li></ul>"##,
            "HTML",
        );
        let f = field(&out, "she-scroll-restoration-mode");
        assert!(
            matches!(
                &f.initial,
                Some(InitialValue::Literal { value: Literal::String(s), .. }) if s == "auto"
            ),
            "got {:?}",
            f.initial
        );
        let f = field(&out, "she-navigation-api-key");
        assert!(
            matches!(
                &f.initial,
                Some(InitialValue::Opaque { text }) if text == "the result of generating a random UUID"
            ),
            "got {:?}",
            f.initial
        );
    }
}
