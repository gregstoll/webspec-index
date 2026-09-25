//! Type index and inherited field lookup (§6.5).
use crate::state::model::{AnchorTarget, SuperBasis, SuperEdge, TypeKey};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// A node in the type hierarchy: a single type with its supertype edges.
#[allow(dead_code)]
pub struct TypeNode {
    pub key: TypeKey,
    pub name: String,
    pub supertypes: Vec<SuperEdge>,
}

/// A field in the flat field table, suitable for `lookup_field`.
#[allow(dead_code)]
pub struct FieldEntry {
    pub anchor: AnchorTarget,
    pub names: Vec<String>,
    pub owners: Vec<TypeKey>,
}

/// Result of a `lookup_field` query.
#[derive(Debug)]
pub enum Lookup {
    /// Exactly one match: the field anchor and the BFS path from the query
    /// type to the type that owns the field.
    Found {
        field: AnchorTarget,
        path: Vec<TypeKey>,
    },
    /// Multiple matches at the same BFS depth; each entry is
    /// (field anchor, path from query type to owning type).
    Ambiguous(Vec<(AnchorTarget, Vec<TypeKey>)>),
    NotFound,
}

/// Index over a set of `TypeNode`s that supports BFS supertype traversal and
/// field lookup by name or anchor.
pub struct TypeIndex {
    types: BTreeMap<TypeKey, TypeNode>,
    /// Maps an anchor key to its canonical type key.
    alias: HashMap<TypeKey, TypeKey>,
}

impl TypeIndex {
    /// Build the index.
    ///
    /// `anchors` is an iterator of `(anchor_key, canonical_type_key)` pairs.
    pub fn new(
        types: impl IntoIterator<Item = TypeNode>,
        anchors: impl IntoIterator<Item = (TypeKey, TypeKey)>,
    ) -> Self {
        Self {
            types: types.into_iter().map(|n| (n.key.clone(), n)).collect(),
            alias: anchors.into_iter().collect(),
        }
    }

    /// Resolve an anchor key to its canonical type key, or return `key` unchanged.
    pub fn canonical(&self, key: &TypeKey) -> TypeKey {
        self.alias.get(key).cloned().unwrap_or_else(|| key.clone())
    }

    /// Breadth-first traversal of the supertype graph starting from `key`.
    ///
    /// Returns `(type_key, path_from_start)` in BFS order; the start type
    /// appears first with a one-element path.  Edge ordering within each node:
    /// `IdlInheritance` before `IdlIncludes` before `Override`, declaration
    /// order within a basis.  Cycle-safe via a visited set.
    pub fn supertypes_bfs(&self, key: &TypeKey) -> Vec<(TypeKey, Vec<TypeKey>)> {
        let start = self.canonical(key);
        let mut result = Vec::new();
        let mut visited: BTreeSet<TypeKey> = BTreeSet::new();
        let mut queue: VecDeque<(TypeKey, Vec<TypeKey>)> = VecDeque::new();

        visited.insert(start.clone());
        queue.push_back((start.clone(), vec![start]));

        while let Some((ty, path)) = queue.pop_front() {
            result.push((ty.clone(), path.clone()));

            if let Some(node) = self.types.get(&ty) {
                let mut edges: Vec<(usize, &SuperEdge)> =
                    node.supertypes.iter().enumerate().collect();
                edges.sort_by_key(|(i, e)| (basis_rank(&e.basis), *i));

                for (_, edge) in edges {
                    let target = self.canonical(&edge.target);
                    if visited.insert(target.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(target.clone());
                        queue.push_back((target, new_path));
                    }
                }
            }
        }

        result
    }

    /// Search for `name_or_anchor` in the field table, walking supertypes BFS.
    ///
    /// At each BFS depth, every field whose canonical owner set contains the
    /// current type and whose `names` (case-folded) contain
    /// `fold_name(name_or_anchor)`, or whose `anchor.anchor ==
    /// name_or_anchor`, is collected.  The first depth that yields any matches
    /// is decisive: one match → `Found`, more than one → `Ambiguous`.
    pub fn lookup_field(
        &self,
        key: &TypeKey,
        name_or_anchor: &str,
        fields: &[FieldEntry],
    ) -> Lookup {
        let folded = fold_name(name_or_anchor);
        let bfs = self.supertypes_bfs(key);

        let mut i = 0;
        while i < bfs.len() {
            let current_depth = bfs[i].1.len() - 1;

            // Collect all BFS entries at this depth.
            let mut j = i;
            let mut matches: Vec<(AnchorTarget, Vec<TypeKey>)> = Vec::new();

            while j < bfs.len() && bfs[j].1.len() - 1 == current_depth {
                let (ty, path) = &bfs[j];
                for f in fields {
                    let canonical_owners: Vec<TypeKey> =
                        f.owners.iter().map(|o| self.canonical(o)).collect();
                    if canonical_owners.contains(ty) {
                        let name_match = f.names.iter().any(|n| fold_name(n) == folded);
                        let anchor_match = f.anchor.anchor == name_or_anchor;
                        if name_match || anchor_match {
                            matches.push((f.anchor.clone(), path.clone()));
                        }
                    }
                }
                j += 1;
            }

            if !matches.is_empty() {
                if matches.len() == 1 {
                    let (field, path) = matches.remove(0);
                    return Lookup::Found { field, path };
                } else {
                    return Lookup::Ambiguous(matches);
                }
            }

            i = j;
        }

        Lookup::NotFound
    }
}

/// Normalize a field name for case-insensitive matching: lowercase, collapse
/// whitespace to single spaces, strip backticks.
pub fn fold_name(name: &str) -> String {
    let s = name.replace('`', "");
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn basis_rank(basis: &SuperBasis) -> u8 {
    match basis {
        SuperBasis::IdlInheritance => 0,
        SuperBasis::IdlIncludes => 1,
        SuperBasis::Override { .. } => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> TypeKey {
        TypeKey::parse(s).unwrap()
    }

    fn node(key: &str, supers: &[(&str, SuperBasis)]) -> TypeNode {
        TypeNode {
            key: k(key),
            name: key.into(),
            supertypes: supers
                .iter()
                .map(|(t, b)| SuperEdge {
                    target: k(t),
                    basis: b.clone(),
                })
                .collect(),
        }
    }

    fn field(spec_anchor: &str, name: &str, owner: &str) -> FieldEntry {
        let (spec, anchor) = spec_anchor.split_once('#').unwrap();
        FieldEntry {
            anchor: AnchorTarget {
                spec: spec.into(),
                anchor: anchor.into(),
            },
            names: vec![name.into()],
            owners: vec![k(owner)],
        }
    }

    #[test]
    fn inherited_field_is_found_with_path() {
        let index = TypeIndex::new(
            vec![
                node("idl:Element", &[("idl:Node", SuperBasis::IdlInheritance)]),
                node("idl:Node", &[]),
            ],
            vec![(k("DOM#concept-node"), k("idl:Node"))],
        );
        let fields = vec![field(
            "DOM#concept-node-document",
            "node document",
            "DOM#concept-node",
        )];
        match index.lookup_field(&k("idl:Element"), "Node Document", &fields) {
            Lookup::Found { field, path } => {
                assert_eq!(field.anchor, "concept-node-document");
                assert_eq!(path, vec![k("idl:Element"), k("idl:Node")]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn same_depth_duplicates_are_ambiguous_and_cycles_terminate() {
        let index = TypeIndex::new(
            vec![
                node("idl:A", &[("idl:B", SuperBasis::IdlInheritance)]),
                node("idl:B", &[("idl:A", SuperBasis::IdlInheritance)]),
            ],
            vec![],
        );
        let fields = vec![
            field("X#one", "state", "idl:A"),
            field("Y#two", "state", "idl:A"),
        ];
        assert!(
            matches!(index.lookup_field(&k("idl:A"), "state", &fields), Lookup::Ambiguous(v) if v.len() == 2)
        );
        assert_eq!(index.supertypes_bfs(&k("idl:A")).len(), 2);
    }

    #[test]
    fn fold_name_lowercases_and_collapses_whitespace_and_strips_backticks() {
        assert_eq!(fold_name("Node Document"), "node document");
        assert_eq!(fold_name("  foo   bar  "), "foo bar");
        assert_eq!(fold_name("`active document`"), "active document");
    }

    #[test]
    fn not_found_when_no_fields_match() {
        let index = TypeIndex::new(vec![node("idl:A", &[])], vec![]);
        let fields = vec![field("X#one", "state", "idl:A")];
        assert!(matches!(
            index.lookup_field(&k("idl:A"), "nonexistent", &fields),
            Lookup::NotFound
        ));
    }

    #[test]
    fn anchor_match_takes_precedence_over_fold_name() {
        let index = TypeIndex::new(vec![node("idl:A", &[])], vec![]);
        let fields = vec![field("X#my-anchor", "some name", "idl:A")];
        match index.lookup_field(&k("idl:A"), "my-anchor", &fields) {
            Lookup::Found { field, .. } => assert_eq!(field.anchor, "my-anchor"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn canonical_returns_key_itself_when_not_an_alias() {
        let index = TypeIndex::new(vec![], vec![(k("X#anchor"), k("idl:T"))]);
        assert_eq!(index.canonical(&k("idl:T")), k("idl:T"));
        assert_eq!(index.canonical(&k("X#anchor")), k("idl:T"));
    }
}
