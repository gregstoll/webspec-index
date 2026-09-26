use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceIndex {
    pub anchor: String,
    pub vars: Vec<String>,
    pub steps: Vec<SliceStep>,
    pub edges: Vec<DefEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceStep {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mentions: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub loop_binds: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefEdge {
    pub step: u32,
    pub kind: DefKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub var: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub uses: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefKind {
    Let,
    Set,
    Mutate,
    Store,
    Opaque,
}

impl SliceIndex {
    pub fn step_by_path(&self, path: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.path == path)
    }

    pub fn children(&self, parent: Option<usize>) -> Vec<usize> {
        let parent = parent.map(|p| p as u32);
        (0..self.steps.len())
            .filter(|&i| self.steps[i].parent == parent)
            .collect()
    }

    pub fn depth(&self, step: usize) -> usize {
        self.steps[step].path.split('.').count()
    }

    pub fn var(&self, name: &str) -> Option<u32> {
        self.vars.iter().position(|v| v == name).map(|i| i as u32)
    }

    pub fn name(&self, var: u32) -> &str {
        &self.vars[var as usize]
    }
}

/// Parent of each step: the nearest preceding step whose path is this path minus its last
/// component. Paths, not `parent_step_id`: steps of nested bodies have no structural parent.
pub(crate) fn derive_parents(paths: &[String]) -> Vec<Option<u32>> {
    let mut last_at: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut parents = Vec::with_capacity(paths.len());
    for (i, path) in paths.iter().enumerate() {
        parents.push(
            path.rsplit_once('.')
                .and_then(|(prefix, _)| last_at.get(prefix).copied()),
        );
        last_at.insert(path.as_str(), i as u32);
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::slice_index;

    #[test]
    fn slice_index_json_is_compact_and_round_trips() {
        let index = SliceIndex {
            anchor: "go".into(),
            vars: vec!["foo".into(), "a".into()],
            steps: vec![
                SliceStep {
                    path: "1".into(),
                    parent: None,
                    mentions: vec![0, 1],
                    loop_binds: vec![],
                },
                SliceStep {
                    path: "1.1".into(),
                    parent: Some(0),
                    mentions: vec![],
                    loop_binds: vec![1],
                },
            ],
            edges: vec![DefEdge {
                step: 0,
                kind: DefKind::Let,
                var: Some(1),
                uses: vec![0],
                target: None,
            }],
        };
        let json = serde_json::to_string(&index).unwrap();
        assert_eq!(
            json,
            r#"{"anchor":"go","vars":["foo","a"],"steps":[{"path":"1","mentions":[0,1]},{"path":"1.1","parent":0,"loop_binds":[1]}],"edges":[{"step":0,"kind":"let","var":1,"uses":[0]}]}"#
        );
        assert_eq!(serde_json::from_str::<SliceIndex>(&json).unwrap(), index);
    }

    #[test]
    fn builder_derives_parents_from_paths_and_interns_in_mention_order() {
        let index = slice_index(
            "go",
            &[
                ("1", &["a", "foo"]),
                ("2", &[]),
                ("2.1", &["b"]),
                ("2.1.1", &["a"]),
                ("3", &["foo"]),
            ],
            &[("2.1", DefKind::Let, Some("b"), &["a"])],
            &[("3", "item")],
        );
        assert_eq!(index.vars, ["a", "foo", "b", "item"]);
        let parents: Vec<Option<u32>> = index.steps.iter().map(|s| s.parent).collect();
        assert_eq!(parents, [None, None, Some(1), Some(2), None]);
        assert_eq!(index.step_by_path("2.1"), Some(2));
        assert_eq!(index.step_by_path("9"), None);
        assert_eq!(index.children(None), [0, 1, 4]);
        assert_eq!(index.children(Some(1)), [2]);
        assert_eq!(index.steps[4].loop_binds, [3]);
        assert_eq!(index.depth(3), 3);
        assert_eq!(index.var("b"), Some(2));
        assert_eq!(index.name(3), "item");
        assert_eq!(
            index.edges[0],
            DefEdge {
                step: 2,
                kind: DefKind::Let,
                var: Some(2),
                uses: vec![0],
                target: None
            }
        );
    }

    #[test]
    fn duplicate_paths_take_the_nearest_preceding_prefix_as_parent() {
        let paths: Vec<String> = ["1", "1.1", "1", "1.1", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(derive_parents(&paths), [None, Some(0), None, Some(2), None]);
    }

    #[test]
    fn store_edges_from_the_builder_carry_a_target() {
        let index = slice_index(
            "go",
            &[("1", &["d", "foo"])],
            &[("1", DefKind::Store, Some("d"), &["foo"])],
            &[],
        );
        assert_eq!(index.edges[0].target.as_deref(), Some("*d*'s field"));
    }
}
