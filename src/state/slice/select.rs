use super::index::{DefKind, SliceIndex};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub involving: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feeding: Option<FeedingSelector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    #[serde(default)]
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedingSelector {
    pub step: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<String>,
}

impl FeedingSelector {
    /// Parses `STEP[:V1[,V2…]]`, normalized and validated like [`validate_shape`].
    pub fn parse(text: &str) -> Result<FeedingSelector, SliceError> {
        let (step, variables) = text.split_once(':').unwrap_or((text, ""));
        let selector = FeedingSelector {
            step: normalize_path(step),
            variables: normalize_names(variables.split(',')),
        };
        check_names(&selector.variables)?;
        check_path(&selector.step)?;
        Ok(selector)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slice {
    pub view: ViewRequest,
    /// Seeds first, then derived variables in fixed-point order.
    pub variables: Vec<SliceVariable>,
    /// Document order.
    pub steps: Vec<KeptStep>,
    pub omitted: Vec<OmittedRun>,
    pub stores: Vec<StoreNote>,
    pub unfollowed: Vec<Unfollowed>,
    pub rebound: Vec<Rebound>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub later_definitions: Option<Vec<LaterDefinition>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SliceVariable {
    pub name: String,
    pub basis: VarBasis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VarBasis {
    Seed,
    Let,
    Set,
    Mutate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeptStep {
    pub path: String,
    pub role: StepRole,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<DefKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRole {
    Match,
    Inherited,
    Context,
    Target,
    Definition,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OmittedRun {
    pub parent: Option<String>,
    pub first: String,
    pub last: String,
    pub steps: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub in_slice: u32,
    pub reason: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoreNote {
    pub step: String,
    pub target: String,
    pub from: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unfollowed {
    pub step: String,
    pub reason: UnfollowedReason,
    pub variables: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnfollowedReason {
    OpaqueStatement,
    LoopBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rebound {
    pub name: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaterDefinition {
    pub step: String,
    pub variable: String,
    pub kind: DefKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SliceError {
    pub code: SliceErrorCode,
    pub message: String,
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceErrorCode {
    InvalidSelector,
    NotAnAlgorithm,
    UnknownVariable,
    UnknownStep,
    Unavailable,
}

impl SliceErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            SliceErrorCode::InvalidSelector => "invalid_selector",
            SliceErrorCode::NotAnAlgorithm => "not_an_algorithm",
            SliceErrorCode::UnknownVariable => "unknown_variable",
            SliceErrorCode::UnknownStep => "unknown_step",
            SliceErrorCode::Unavailable => "unavailable",
        }
    }
}

impl SliceError {
    fn invalid(message: impl Into<String>) -> SliceError {
        SliceError {
            code: SliceErrorCode::InvalidSelector,
            message: message.into(),
            candidates: Vec::new(),
        }
    }
}

impl fmt::Display for SliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slice_{}: {}", self.code.as_str(), self.message)?;
        if !self.candidates.is_empty() {
            write!(f, "\nCandidates: {}", self.candidates.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for SliceError {}

fn normalize_name(name: &str) -> String {
    let name = name.trim();
    let stripped = name
        .strip_prefix('*')
        .and_then(|n| n.strip_suffix('*'))
        .filter(|_| name.len() >= 2);
    stripped.unwrap_or(name).to_owned()
}

fn normalize_path(path: &str) -> String {
    let path = path.trim();
    path.strip_suffix('.').unwrap_or(path).to_owned()
}

/// Drops empty entries and keeps the first of duplicates.
fn dedupe(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !item.is_empty() && !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn normalize_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    dedupe(names.map(normalize_name))
}

fn check_names(names: &[String]) -> Result<(), SliceError> {
    match names.iter().find(|n| n.contains('*')) {
        Some(name) => Err(SliceError::invalid(format!("invalid variable name {name}"))),
        None => Ok(()),
    }
}

fn check_path(path: &str) -> Result<(), SliceError> {
    let valid = path
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if valid {
        Ok(())
    } else {
        Err(SliceError::invalid(format!("invalid step path {path}")))
    }
}

/// Normalizes `view` and runs every check that needs no slice index.
pub fn validate_shape(view: &ViewRequest) -> Result<ViewRequest, SliceError> {
    let view = ViewRequest {
        involving: normalize_names(view.involving.iter().map(String::as_str)),
        feeding: view.feeding.as_ref().map(|f| FeedingSelector {
            step: normalize_path(&f.step),
            variables: normalize_names(f.variables.iter().map(String::as_str)),
        }),
        steps: dedupe(view.steps.iter().map(|p| normalize_path(p))),
        depth: view.depth,
    };
    check_names(&view.involving)?;
    if let Some(feeding) = &view.feeding {
        check_names(&feeding.variables)?;
    }
    let selectors = [
        !view.involving.is_empty(),
        view.feeding.is_some(),
        !view.steps.is_empty(),
    ];
    match selectors.iter().filter(|&&s| s).count() {
        0 if view.depth.is_none() => {
            return Err(SliceError::invalid(
                "empty view: give involving, feeding, steps or depth",
            ))
        }
        0 | 1 => {}
        _ => {
            return Err(SliceError::invalid(
                "choose one of involving, feeding, steps",
            ))
        }
    }
    if view.depth == Some(0) {
        return Err(SliceError::invalid("depth must be at least 1"));
    }
    for path in view.feeding.iter().map(|f| &f.step).chain(&view.steps) {
        check_path(path)?;
    }
    Ok(view)
}

/// The view of one algorithm that `view` selects.
pub fn slice(index: &SliceIndex, view: &ViewRequest) -> Result<Slice, SliceError> {
    let view = validate_shape(view)?;
    if view.feeding.is_some() || !view.steps.is_empty() || view.depth.is_some() {
        return Err(SliceError::invalid("not implemented"));
    }
    forward(index, view)
}

/// `*a*`, `*a* or *b*`, `*a*, *b* or *c*`.
fn or_list(names: &[String]) -> String {
    let emph: Vec<String> = names.iter().map(|n| format!("*{n}*")).collect();
    match emph.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

fn names_in(index: &SliceIndex, vars: &[u32], in_set: &[bool]) -> Vec<String> {
    vars.iter()
        .filter(|&&v| in_set[v as usize])
        .map(|&v| index.name(v).to_owned())
        .collect()
}

/// Forward slice (spec §6.3–6.5).
fn forward(index: &SliceIndex, view: ViewRequest) -> Result<Slice, SliceError> {
    let mut in_set = vec![false; index.vars.len()];
    let mut variables: Vec<SliceVariable> = Vec::new();
    let mut members: Vec<u32> = Vec::new();
    for name in &view.involving {
        let Some(v) = index.var(name) else {
            return Err(SliceError {
                code: SliceErrorCode::UnknownVariable,
                message: format!(
                    "variable *{name}* is not mentioned in the steps of {}",
                    index.anchor
                ),
                candidates: index.vars.clone(),
            });
        };
        in_set[v as usize] = true;
        members.push(v);
        variables.push(SliceVariable {
            name: name.clone(),
            basis: VarBasis::Seed,
            step: None,
            from: Vec::new(),
        });
    }

    let mut changed = true;
    while changed {
        changed = false;
        for edge in &index.edges {
            let basis = match edge.kind {
                DefKind::Let => VarBasis::Let,
                DefKind::Set => VarBasis::Set,
                DefKind::Mutate => VarBasis::Mutate,
                DefKind::Store | DefKind::Opaque => continue,
            };
            let Some(v) = edge.var.filter(|&v| !in_set[v as usize]) else {
                continue;
            };
            let from = names_in(index, &edge.uses, &in_set);
            if from.is_empty() {
                continue;
            }
            in_set[v as usize] = true;
            members.push(v);
            variables.push(SliceVariable {
                name: index.name(v).to_owned(),
                basis,
                step: Some(index.steps[edge.step as usize].path.clone()),
                from,
            });
            changed = true;
        }
    }

    let n = index.steps.len();
    let mut kept: Vec<Option<StepRole>> = vec![None; n];
    for (i, step) in index.steps.iter().enumerate() {
        if step.mentions.iter().any(|&v| in_set[v as usize]) {
            kept[i] = Some(StepRole::Match);
        }
    }
    for (i, step) in index.steps.iter().enumerate() {
        if step.mentions.is_empty()
            && step
                .parent
                .is_some_and(|p| kept[p as usize] == Some(StepRole::Match))
        {
            kept[i] = Some(StepRole::Inherited);
        }
    }
    keep_context(index, &mut kept);

    let stores = index
        .edges
        .iter()
        .filter(|edge| edge.kind == DefKind::Store)
        .filter_map(|edge| {
            let from = names_in(index, &edge.uses, &in_set);
            (!from.is_empty()).then(|| StoreNote {
                step: index.steps[edge.step as usize].path.clone(),
                target: edge.target.clone().unwrap_or_default(),
                from,
            })
        })
        .collect();

    let mut unfollowed = Vec::new();
    for (i, step) in index.steps.iter().enumerate() {
        if kept[i] != Some(StepRole::Match) {
            continue;
        }
        for edge in index
            .edges
            .iter()
            .filter(|e| e.kind == DefKind::Opaque && e.step as usize == i)
        {
            let variables = names_in(index, &edge.uses, &in_set);
            if !variables.is_empty() {
                unfollowed.push(Unfollowed {
                    step: step.path.clone(),
                    reason: UnfollowedReason::OpaqueStatement,
                    variables,
                });
            }
        }
        for &v in step.loop_binds.iter().filter(|&&v| !in_set[v as usize]) {
            unfollowed.push(Unfollowed {
                step: step.path.clone(),
                reason: UnfollowedReason::LoopBinding,
                variables: vec![index.name(v).to_owned()],
            });
        }
    }

    let rebound = members
        .iter()
        .filter_map(|&v| {
            let mut steps: Vec<u32> = index
                .edges
                .iter()
                .filter(|e| e.kind == DefKind::Let && e.var == Some(v))
                .map(|e| e.step)
                .collect();
            steps.sort_unstable();
            steps.dedup();
            (steps.len() > 1).then(|| Rebound {
                name: index.name(v).to_owned(),
                steps: steps
                    .iter()
                    .map(|&s| index.steps[s as usize].path.clone())
                    .collect(),
            })
        })
        .collect();

    let reason = format!("no use of {}", or_list(&view.involving));
    let omitted = omitted_runs(index, &kept, &|_| (reason.clone(), 0));
    Ok(Slice {
        view,
        variables,
        steps: kept_steps(index, &kept),
        omitted,
        stores,
        unfollowed,
        rebound,
        inputs: None,
        later_definitions: None,
    })
}

/// Keeps every not yet kept ancestor of a kept step as `Context`.
fn keep_context(index: &SliceIndex, kept: &mut [Option<StepRole>]) {
    for i in (0..index.steps.len()).rev() {
        if kept[i].is_none() {
            continue;
        }
        let mut parent = index.steps[i].parent;
        while let Some(p) = parent.map(|p| p as usize) {
            if kept[p].is_some() {
                break;
            }
            kept[p] = Some(StepRole::Context);
            parent = index.steps[p].parent;
        }
    }
}

fn kept_steps(index: &SliceIndex, kept: &[Option<StepRole>]) -> Vec<KeptStep> {
    index
        .steps
        .iter()
        .zip(kept)
        .filter_map(|(step, role)| {
            role.map(|role| KeptStep {
                path: step.path.clone(),
                role,
                edges: Vec::new(),
            })
        })
        .collect()
}

fn subtree_sizes(index: &SliceIndex) -> Vec<u32> {
    let mut size = vec![1u32; index.steps.len()];
    for i in (0..index.steps.len()).rev() {
        if let Some(p) = index.steps[i].parent {
            size[p as usize] += size[i];
        }
    }
    size
}

/// Runs of consecutive non-kept siblings under the root and under every kept step, in document
/// order of their first step. `reason` gives a run's text and its count of hidden slice steps.
fn omitted_runs(
    index: &SliceIndex,
    kept: &[Option<StepRole>],
    reason: &dyn Fn(&[usize]) -> (String, u32),
) -> Vec<OmittedRun> {
    let size = subtree_sizes(index);
    let n = index.steps.len();
    // Slot 0 holds the root's children, slot i + 1 those of step i.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    for (i, step) in index.steps.iter().enumerate() {
        children[step.parent.map_or(0, |p| p as usize + 1)].push(i);
    }
    let parents = std::iter::once(None).chain((0..n).filter(|&i| kept[i].is_some()).map(Some));
    let mut runs: Vec<(usize, OmittedRun)> = Vec::new();
    for parent in parents {
        let siblings = &children[parent.map_or(0, |p| p + 1)];
        for group in siblings.split(|&child| kept[child].is_some()) {
            let (Some(&first), Some(&last)) = (group.first(), group.last()) else {
                continue;
            };
            let (text, in_slice) = reason(group);
            runs.push((
                first,
                OmittedRun {
                    parent: parent.map(|p| index.steps[p].path.clone()),
                    first: index.steps[first].path.clone(),
                    last: index.steps[last].path.clone(),
                    steps: group.iter().map(|&g| size[g]).sum(),
                    in_slice,
                    reason: text,
                },
            ));
        }
    }
    runs.sort_by_key(|(first, _)| *first);
    runs.into_iter().map(|(_, run)| run).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::slice::DefKind;
    use crate::state::testing::slice_index;

    /// A reduced DOM "insert": 18 steps, Let chains, a store and a loop.
    fn insertish() -> SliceIndex {
        slice_index(
            "insert",
            &[
                ("1", &["nodes", "node"]),
                ("2", &["count", "nodes"]),
                ("3", &["count"]),
                ("4", &["node"]),
                ("4.1", &[]),
                ("4.2", &["node", "nodes"]),
                ("5", &["child"]),
                ("5.1", &["parent", "child", "count"]),
                ("5.2", &["parent", "child", "count"]),
                ("6", &["previousSibling", "child", "parent"]),
                ("7", &["node", "nodes"]),
                ("7.1", &["node", "parent"]),
                ("7.2", &["node", "parent"]),
                ("7.3", &["node", "inclusiveDescendant"]),
                ("7.3.1", &["inclusiveDescendant"]),
                (
                    "8",
                    &[
                        "suppressObservers",
                        "parent",
                        "nodes",
                        "previousSibling",
                        "child",
                    ],
                ),
                ("9", &["parent"]),
                ("10", &["staticNodeList"]),
            ],
            &[
                ("1", DefKind::Let, Some("nodes"), &["node"]),
                ("2", DefKind::Let, Some("count"), &["nodes"]),
                (
                    "6",
                    DefKind::Let,
                    Some("previousSibling"),
                    &["child", "parent"],
                ),
                ("7.2", DefKind::Store, Some("parent"), &["node"]),
            ],
            &[("7.3", "inclusiveDescendant")],
        )
    }

    fn involving(names: &[&str]) -> ViewRequest {
        ViewRequest {
            involving: names.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn roles(s: &Slice) -> Vec<(String, StepRole)> {
        s.steps.iter().map(|k| (k.path.clone(), k.role)).collect()
    }

    fn runs(s: &Slice) -> Vec<(Option<&str>, &str, &str, u32)> {
        s.omitted
            .iter()
            .map(|r| {
                (
                    r.parent.as_deref(),
                    r.first.as_str(),
                    r.last.as_str(),
                    r.steps,
                )
            })
            .collect()
    }

    #[test]
    fn forward_slice_from_parent_follows_let_and_keeps_enclosing_steps() {
        let s = slice(&insertish(), &involving(&["parent"])).unwrap();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "view": {"involving": ["parent"], "depth": null},
                "variables": [
                    {"name": "parent", "basis": "seed"},
                    {"name": "previousSibling", "basis": "let", "step": "6", "from": ["parent"]}
                ],
                "steps": [
                    {"path": "5", "role": "context"}, {"path": "5.1", "role": "match"}, {"path": "5.2", "role": "match"},
                    {"path": "6", "role": "match"}, {"path": "7", "role": "context"}, {"path": "7.1", "role": "match"},
                    {"path": "7.2", "role": "match"}, {"path": "8", "role": "match"}, {"path": "9", "role": "match"}
                ],
                "omitted": [
                    {"parent": null, "first": "1", "last": "4", "steps": 6, "reason": "no use of *parent*"},
                    {"parent": "7", "first": "7.3", "last": "7.3", "steps": 2, "reason": "no use of *parent*"},
                    {"parent": null, "first": "10", "last": "10", "steps": 1, "reason": "no use of *parent*"}
                ],
                "stores": [], "unfollowed": [], "rebound": []
            })
        );
    }

    #[test]
    fn forward_slice_from_node_reports_inherited_stores_and_loops() {
        let s = slice(&insertish(), &involving(&["node"])).unwrap();
        let vars: Vec<(&str, VarBasis, Option<&str>)> = s
            .variables
            .iter()
            .map(|v| (v.name.as_str(), v.basis, v.step.as_deref()))
            .collect();
        assert_eq!(
            vars,
            [
                ("node", VarBasis::Seed, None),
                ("nodes", VarBasis::Let, Some("1")),
                ("count", VarBasis::Let, Some("2"))
            ]
        );
        assert!(roles(&s).contains(&("4.1".into(), StepRole::Inherited)));
        assert!(roles(&s).contains(&("5".into(), StepRole::Context)));
        assert!(
            !roles(&s).iter().any(|(p, _)| p == "7.3.1"),
            "a child with other variables is not inherited"
        );
        assert_eq!(s.steps.len(), 14);
        assert_eq!(
            runs(&s),
            [
                (None, "6", "6", 1),
                (Some("7.3"), "7.3.1", "7.3.1", 1),
                (None, "9", "10", 2)
            ]
        );
        assert_eq!(
            s.stores,
            [StoreNote {
                step: "7.2".into(),
                target: "*parent*'s field".into(),
                from: vec!["node".into()]
            }]
        );
        assert_eq!(
            s.unfollowed,
            [Unfollowed {
                step: "7.3".into(),
                reason: UnfollowedReason::LoopBinding,
                variables: vec!["inclusiveDescendant".into()]
            }]
        );
        assert_eq!(
            (s.inputs.clone(), s.later_definitions.clone()),
            (None, None)
        );
    }

    #[test]
    fn opaque_statements_and_rebound_names_are_listed() {
        let index = slice_index(
            "go",
            &[
                ("1", &["x", "foo"]),
                ("2", &["x", "foo"]),
                ("3", &["foo", "q"]),
                ("4", &["x"]),
            ],
            &[
                ("1", DefKind::Let, Some("x"), &[]),
                ("2", DefKind::Let, Some("x"), &["foo"]),
                ("3", DefKind::Opaque, None, &["foo", "q"]),
            ],
            &[],
        );
        let s = slice(&index, &involving(&["foo"])).unwrap();
        assert_eq!(
            s.unfollowed,
            [Unfollowed {
                step: "3".into(),
                reason: UnfollowedReason::OpaqueStatement,
                variables: vec!["foo".into()]
            }]
        );
        assert_eq!(
            s.rebound,
            [Rebound {
                name: "x".into(),
                steps: vec!["1".into(), "2".into()]
            }]
        );
        assert_eq!(
            s.steps.len(),
            4,
            "flow-insensitive: x joins, so step 1 and 4 match"
        );
    }

    #[test]
    fn selector_input_forms() {
        let index = insertish();
        let s = slice(&index, &involving(&["*node*", " node ", "node", ""])).unwrap();
        assert_eq!(s.view.involving, ["node"]);
        let e = slice(&index, &involving(&["nope"])).unwrap_err();
        assert_eq!(e.code, SliceErrorCode::UnknownVariable);
        assert_eq!(
            e.candidates,
            [
                "nodes",
                "node",
                "count",
                "child",
                "parent",
                "previousSibling",
                "inclusiveDescendant",
                "suppressObservers",
                "staticNodeList"
            ]
        );
        assert_eq!(
            e.to_string().lines().next().unwrap(),
            "slice_unknown_variable: variable *nope* is not mentioned in the steps of insert"
        );
        let unavailable = SliceError {
            code: SliceErrorCode::Unavailable,
            message: "m".into(),
            candidates: vec![],
        };
        assert!(
            unavailable.to_string().starts_with("slice_unavailable:"),
            "{unavailable}"
        );
        assert_eq!(
            serde_json::to_value(SliceErrorCode::Unavailable).unwrap(),
            "unavailable"
        );
        assert_eq!(SliceErrorCode::InvalidSelector.as_str(), "invalid_selector");
        let both = ViewRequest {
            involving: vec!["node".into()],
            steps: vec!["1".into()],
            ..Default::default()
        };
        assert_eq!(
            slice(&index, &both).unwrap_err().code,
            SliceErrorCode::InvalidSelector
        );
        assert_eq!(
            slice(&index, &ViewRequest::default()).unwrap_err().code,
            SliceErrorCode::InvalidSelector
        );
        let zero = ViewRequest {
            depth: Some(0),
            ..involving(&["node"])
        };
        assert_eq!(
            slice(&index, &zero).unwrap_err().code,
            SliceErrorCode::InvalidSelector
        );
        assert_eq!(
            validate_shape(&involving(&["*"])).unwrap_err().code,
            SliceErrorCode::InvalidSelector
        );
        assert_eq!(
            validate_shape(&involving(&[""])).unwrap_err().code,
            SliceErrorCode::InvalidSelector
        );
        assert_eq!(
            FeedingSelector::parse("24.9.1").unwrap(),
            FeedingSelector {
                step: "24.9.1".into(),
                variables: vec![]
            }
        );
        assert_eq!(
            FeedingSelector::parse("24.9.1:").unwrap(),
            FeedingSelector {
                step: "24.9.1".into(),
                variables: vec![]
            }
        );
        assert_eq!(
            FeedingSelector::parse(" 13. : *historyEntry* , url ").unwrap(),
            FeedingSelector {
                step: "13".into(),
                variables: vec!["historyEntry".into(), "url".into()]
            }
        );
        for bad in ["abc", "1..2", ":x", ""] {
            assert_eq!(
                FeedingSelector::parse(bad).unwrap_err().code,
                SliceErrorCode::InvalidSelector,
                "{bad:?}"
            );
        }
        let spaced = slice_index("go", &[("1", &["raw input"]), ("2", &["größe"])], &[], &[]);
        assert_eq!(
            slice(&spaced, &involving(&["*raw input*"]))
                .unwrap()
                .steps
                .len(),
            1
        );
        assert_eq!(
            slice(&spaced, &involving(&["größe"])).unwrap().steps[0].path,
            "2"
        );
    }

    #[test]
    fn conservation_holds_for_every_variable() {
        let index = insertish();
        for name in &index.vars {
            let s = slice(&index, &involving(&[name])).unwrap();
            let omitted: u32 = s.omitted.iter().map(|r| r.steps).sum();
            assert_eq!(
                s.steps.len() as u32 + omitted,
                index.steps.len() as u32,
                "{name}"
            );
        }
    }
}
