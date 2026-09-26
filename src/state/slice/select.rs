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

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    Feeds,
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

/// What a selector keeps, before `depth` cuts it and omitted steps are grouped into runs.
struct Selection {
    kept: Vec<Option<StepRole>>,
    /// Definition edge kinds per step, in `DefKind` order; empty unless the selector records them.
    edges: Vec<Vec<DefKind>>,
    /// Reason of the runs the selector omits.
    reason: String,
}

/// The view of one algorithm that `view` selects.
pub fn slice(index: &SliceIndex, view: &ViewRequest) -> Result<Slice, SliceError> {
    let view = validate_shape(view)?;
    let size = subtree_sizes(index);
    let (mut slice, selection) = if let Some(feeding) = &view.feeding {
        backward(index, feeding)?
    } else if !view.steps.is_empty() {
        select_steps(index, &view.steps, &size)?
    } else if !view.involving.is_empty() {
        forward(index, &view.involving)?
    } else {
        let selection = Selection {
            kept: vec![Some(StepRole::Selected); index.steps.len()],
            edges: Vec::new(),
            reason: String::new(),
        };
        (Slice::default(), selection)
    };
    slice.view = view;
    Ok(finish(index, slice, selection, &size))
}

/// Applies `depth`, then fills in the kept steps and the omitted runs.
fn finish(index: &SliceIndex, mut slice: Slice, mut sel: Selection, size: &[u32]) -> Slice {
    slice.omitted = match slice.view.depth {
        None => omitted_runs(index, &sel.kept, size, &|_| (sel.reason.clone(), 0)),
        Some(depth) => {
            let depth = depth as usize;
            // in_slice_before[i]: steps before step i whose role puts them in the slice.
            let mut in_slice_before = vec![0u32; index.steps.len() + 1];
            for (i, role) in sel.kept.iter().enumerate() {
                let in_slice = matches!(
                    role,
                    Some(
                        StepRole::Match
                            | StepRole::Inherited
                            | StepRole::Target
                            | StepRole::Definition
                    )
                );
                in_slice_before[i + 1] = in_slice_before[i] + u32::from(in_slice);
            }
            for (i, role) in sel.kept.iter_mut().enumerate() {
                if index.depth(i) > depth {
                    *role = None;
                }
            }
            let reason = |group: &[usize]| {
                if index.depth(group[0]) <= depth {
                    return (sel.reason.clone(), 0);
                }
                let hidden: u32 = group
                    .iter()
                    .map(|&g| in_slice_before[g + size[g] as usize] - in_slice_before[g])
                    .sum();
                let text = if hidden > 0 {
                    format!("below depth {depth}, {hidden} in slice")
                } else {
                    format!("below depth {depth}")
                };
                (text, hidden)
            };
            omitted_runs(index, &sel.kept, size, &reason)
        }
    };
    slice.steps = kept_steps(index, &sel);
    slice
}

/// The step at `path`, or `UnknownStep` listing the top-level paths and the children of the
/// deepest existing proper prefix of `path`.
fn step_at(index: &SliceIndex, path: &str) -> Result<usize, SliceError> {
    index.step_by_path(path).ok_or_else(|| {
        let prefixes = std::iter::successors(Some(path), |p| p.rsplit_once('.').map(|(p, _)| p));
        let parent = prefixes.skip(1).find_map(|p| index.step_by_path(p));
        let top = index.children(None);
        let below = parent.map(|p| index.children(Some(p))).unwrap_or_default();
        SliceError {
            code: SliceErrorCode::UnknownStep,
            message: format!("step {path} does not exist in {}", index.anchor),
            candidates: dedupe(
                top.iter()
                    .chain(&below)
                    .map(|&i| index.steps[i].path.clone()),
            ),
        }
    })
}

/// Step selection (spec §6.7): each named step with its subtree, and its enclosing steps.
fn select_steps(
    index: &SliceIndex,
    paths: &[String],
    size: &[u32],
) -> Result<(Slice, Selection), SliceError> {
    let mut kept = vec![None; index.steps.len()];
    for path in paths {
        let i = step_at(index, path)?;
        kept[i..i + size[i] as usize].fill(Some(StepRole::Selected));
    }
    keep_context(index, &mut kept);
    let selection = Selection {
        kept,
        edges: Vec::new(),
        reason: format!("outside {}", paths.join(", ")),
    };
    Ok((Slice::default(), selection))
}

fn is_definition(kind: DefKind) -> bool {
    matches!(
        kind,
        DefKind::Let | DefKind::Set | DefKind::Mutate | DefKind::Store
    )
}

/// Backward slice (spec §6.6).
fn backward(
    index: &SliceIndex,
    feeding: &FeedingSelector,
) -> Result<(Slice, Selection), SliceError> {
    let target = step_at(index, &feeding.step)?;
    let path = &index.steps[target].path;
    let mentions = &index.steps[target].mentions;
    let seeds: Vec<u32> = if feeding.variables.is_empty() {
        mentions.clone()
    } else {
        feeding
            .variables
            .iter()
            .map(|name| {
                index
                    .var(name)
                    .filter(|v| mentions.contains(v))
                    .ok_or_else(|| SliceError {
                        code: SliceErrorCode::InvalidSelector,
                        message: format!("*{name}* is not mentioned by step {path}"),
                        candidates: mentions.iter().map(|&v| index.name(v).to_owned()).collect(),
                    })
            })
            .collect::<Result<_, _>>()?
    };

    let n = index.steps.len();
    let mut definitions_of: Vec<Vec<usize>> = vec![Vec::new(); index.vars.len()];
    for (e, edge) in index.edges.iter().enumerate() {
        if let Some(v) = edge.var.filter(|_| is_definition(edge.kind)) {
            definitions_of[v as usize].push(e);
        }
    }

    let mut in_set = vec![false; index.vars.len()];
    let mut members: Vec<u32> = Vec::new();
    let mut variables: Vec<SliceVariable> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    for &v in &seeds {
        in_set[v as usize] = true;
        members.push(v);
        queue.push_back(v);
        variables.push(SliceVariable {
            name: index.name(v).to_owned(),
            basis: VarBasis::Seed,
            step: None,
            from: Vec::new(),
        });
    }
    let mut kept: Vec<Option<StepRole>> = vec![None; n];
    let mut edges: Vec<Vec<DefKind>> = vec![Vec::new(); n];
    kept[target] = Some(StepRole::Target);
    while let Some(v) = queue.pop_front() {
        for &e in &definitions_of[v as usize] {
            let edge = &index.edges[e];
            let step = edge.step as usize;
            if step >= target {
                continue;
            }
            kept[step] = Some(StepRole::Definition);
            if !edges[step].contains(&edge.kind) {
                edges[step].push(edge.kind);
            }
            for &u in &edge.uses {
                if in_set[u as usize] {
                    continue;
                }
                in_set[u as usize] = true;
                members.push(u);
                queue.push_back(u);
                variables.push(SliceVariable {
                    name: index.name(u).to_owned(),
                    basis: VarBasis::Feeds,
                    step: Some(index.steps[step].path.clone()),
                    from: vec![index.name(v).to_owned()],
                });
            }
        }
    }
    for kinds in &mut edges {
        kinds.sort_by_key(|&k| k as u8);
    }
    keep_context(index, &mut kept);

    let mut bound = vec![false; index.vars.len()];
    for edge in &index.edges {
        if matches!(edge.kind, DefKind::Let | DefKind::Set | DefKind::Mutate) {
            if let Some(v) = edge.var {
                bound[v as usize] = true;
            }
        }
    }
    let mut inputs: Vec<String> = members
        .iter()
        .filter(|&&v| !bound[v as usize])
        .map(|&v| index.name(v).to_owned())
        .collect();
    inputs.sort();

    let later_definitions = index
        .edges
        .iter()
        .filter(|e| e.step as usize >= target && is_definition(e.kind))
        .filter_map(|e| {
            let v = e.var.filter(|&v| in_set[v as usize])?;
            Some(LaterDefinition {
                step: index.steps[e.step as usize].path.clone(),
                variable: index.name(v).to_owned(),
                kind: e.kind,
            })
        })
        .collect();

    let unfollowed = index
        .edges
        .iter()
        .filter(|e| e.kind == DefKind::Opaque && (e.step as usize) < target)
        .filter_map(|e| {
            let variables = names_in(index, &e.uses, &in_set);
            (!variables.is_empty()).then(|| Unfollowed {
                step: index.steps[e.step as usize].path.clone(),
                reason: UnfollowedReason::OpaqueStatement,
                variables,
            })
        })
        .collect();

    let reason = if feeding.variables.is_empty() {
        format!("does not feed step {path}")
    } else {
        format!(
            "does not feed {} in step {path}",
            or_list(&feeding.variables)
        )
    };
    let slice = Slice {
        variables,
        unfollowed,
        rebound: rebound(index, &members),
        inputs: Some(inputs),
        later_definitions: Some(later_definitions),
        ..Default::default()
    };
    Ok((
        slice,
        Selection {
            kept,
            edges,
            reason,
        },
    ))
}

/// Slice variables `Let`-bound at more than one step, in `members` order.
fn rebound(index: &SliceIndex, members: &[u32]) -> Vec<Rebound> {
    members
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
        .collect()
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
fn forward(index: &SliceIndex, involving: &[String]) -> Result<(Slice, Selection), SliceError> {
    let mut in_set = vec![false; index.vars.len()];
    let mut variables: Vec<SliceVariable> = Vec::new();
    let mut members: Vec<u32> = Vec::new();
    for name in involving {
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

    let slice = Slice {
        variables,
        stores,
        unfollowed,
        rebound: rebound(index, &members),
        ..Default::default()
    };
    let selection = Selection {
        kept,
        edges: Vec::new(),
        reason: format!("no use of {}", or_list(involving)),
    };
    Ok((slice, selection))
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

fn kept_steps(index: &SliceIndex, sel: &Selection) -> Vec<KeptStep> {
    index
        .steps
        .iter()
        .zip(&sel.kept)
        .enumerate()
        .filter_map(|(i, (step, role))| {
            role.map(|role| KeptStep {
                path: step.path.clone(),
                role,
                edges: sel.edges.get(i).cloned().unwrap_or_default(),
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
    size: &[u32],
    reason: &dyn Fn(&[usize]) -> (String, u32),
) -> Vec<OmittedRun> {
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
    use crate::state::testing::{slice_fixture_insert, slice_fixture_navigate, slice_index};

    fn insertish() -> SliceIndex {
        slice_fixture_insert()
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

    fn navish() -> SliceIndex {
        slice_fixture_navigate()
    }

    fn feeding(text: &str) -> ViewRequest {
        ViewRequest {
            feeding: Some(FeedingSelector::parse(text).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn backward_slice_follows_definitions_and_stores_before_the_target() {
        let s = slice(&navish(), &feeding("7")).unwrap();
        let vars: Vec<(&str, VarBasis, Option<&str>, Vec<String>)> = s
            .variables
            .iter()
            .map(|v| (v.name.as_str(), v.basis, v.step.as_deref(), v.from.clone()))
            .collect();
        assert_eq!(
            vars,
            [
                ("navigable", VarBasis::Seed, None, vec![]),
                ("entry", VarBasis::Seed, None, vec![]),
                ("historyHandling", VarBasis::Seed, None, vec![]),
                (
                    "state",
                    VarBasis::Feeds,
                    Some("5"),
                    vec!["entry".to_string()]
                ),
                ("url", VarBasis::Feeds, Some("5"), vec!["entry".to_string()]),
                (
                    "initiator",
                    VarBasis::Feeds,
                    Some("4"),
                    vec!["state".to_string()]
                ),
                (
                    "referrerPolicy",
                    VarBasis::Feeds,
                    Some("4"),
                    vec!["state".to_string()]
                ),
                (
                    "sourceDocument",
                    VarBasis::Feeds,
                    Some("2"),
                    vec!["initiator".to_string()]
                ),
            ]
        );
        let kept: Vec<(String, StepRole, Vec<DefKind>)> = s
            .steps
            .iter()
            .map(|k| (k.path.clone(), k.role, k.edges.clone()))
            .collect();
        assert_eq!(
            kept,
            [
                ("2".into(), StepRole::Definition, vec![DefKind::Let]),
                ("3".into(), StepRole::Context, vec![]),
                ("3.1".into(), StepRole::Definition, vec![DefKind::Set]),
                ("4".into(), StepRole::Definition, vec![DefKind::Let]),
                ("4.1".into(), StepRole::Definition, vec![DefKind::Store]),
                ("5".into(), StepRole::Definition, vec![DefKind::Let]),
                ("6".into(), StepRole::Definition, vec![DefKind::Store]),
                ("7".into(), StepRole::Target, vec![]),
            ]
        );
        assert_eq!(runs(&s), [(None, "1", "1", 1), (None, "8", "9", 2)]);
        assert_eq!(s.omitted[0].reason, "does not feed step 7");
        assert_eq!(
            s.inputs.as_deref().unwrap(),
            [
                "historyHandling",
                "navigable",
                "referrerPolicy",
                "sourceDocument",
                "url"
            ]
        );
        assert_eq!(
            s.later_definitions.as_deref().unwrap(),
            [LaterDefinition {
                step: "8".into(),
                variable: "entry".into(),
                kind: DefKind::Set
            }]
        );
        assert_eq!(
            s.unfollowed,
            [Unfollowed {
                step: "4".into(),
                reason: UnfollowedReason::OpaqueStatement,
                variables: vec!["initiator".into()]
            }]
        );
        assert!(s.stores.is_empty());
    }

    #[test]
    fn backward_slice_with_named_variables_and_errors() {
        let index = navish();
        let s = slice(&index, &feeding("7:historyHandling")).unwrap();
        assert_eq!(roles(&s), [("7".to_string(), StepRole::Target)]);
        assert_eq!(s.inputs.as_deref().unwrap(), ["historyHandling"]);
        assert_eq!(runs(&s), [(None, "1", "6", 8), (None, "8", "9", 2)]);
        assert_eq!(
            s.omitted[0].reason,
            "does not feed *historyHandling* in step 7"
        );
        let s = slice(&index, &feeding("7:entry,*navigable*")).unwrap();
        assert_eq!(
            s.omitted[0].reason,
            "does not feed *entry* or *navigable* in step 7"
        );
        assert!(!s.variables.iter().any(|v| v.name == "historyHandling"));
        for unmentioned in ["7:other", "7:url", "7:nope"] {
            let e = slice(&index, &feeding(unmentioned)).unwrap_err();
            assert_eq!(
                (e.code, e.candidates.clone()),
                (
                    SliceErrorCode::InvalidSelector,
                    vec!["navigable".into(), "entry".into(), "historyHandling".into()]
                ),
                "{unmentioned}"
            );
        }
        let e = slice(&index, &feeding("3.4")).unwrap_err();
        assert_eq!(e.code, SliceErrorCode::UnknownStep);
        assert_eq!(
            e.candidates,
            ["1", "2", "3", "4", "5", "6", "7", "8", "9", "3.1"]
        );
    }

    #[test]
    fn steps_selector_keeps_subtrees_and_context() {
        let index = navish();
        let view = ViewRequest {
            steps: vec!["3".into(), "4.1.".into()],
            ..Default::default()
        };
        let s = slice(&index, &view).unwrap();
        assert_eq!(s.view.steps, ["3", "4.1"]);
        assert_eq!(
            roles(&s),
            [
                ("3".to_string(), StepRole::Selected),
                ("3.1".to_string(), StepRole::Selected),
                ("4".to_string(), StepRole::Context),
                ("4.1".to_string(), StepRole::Selected),
            ]
        );
        assert_eq!(runs(&s), [(None, "1", "2", 2), (None, "5", "9", 5)]);
        assert_eq!(s.omitted[0].reason, "outside 3, 4.1");
        let e = slice(
            &index,
            &ViewRequest {
                steps: vec!["12".into()],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.code, SliceErrorCode::UnknownStep);
    }

    #[test]
    fn depth_alone_and_under_a_slice() {
        let index = navish();
        let s = slice(
            &index,
            &ViewRequest {
                depth: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.steps.len(), 9);
        assert!(s.steps.iter().all(|k| k.role == StepRole::Selected));
        assert_eq!(
            runs(&s),
            [(Some("3"), "3.1", "3.1", 1), (Some("4"), "4.1", "4.1", 1)]
        );
        assert!(s
            .omitted
            .iter()
            .all(|r| r.reason == "below depth 1" && r.in_slice == 0));
        let s = slice(
            &index,
            &ViewRequest {
                depth: Some(1),
                ..involving(&["initiator"])
            },
        )
        .unwrap();
        assert_eq!(
            roles(&s)
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>(),
            ["2", "3", "4", "5", "7", "8"]
        );
        let reasons: Vec<(&str, &str, u32)> = s
            .omitted
            .iter()
            .map(|r| (r.first.as_str(), r.reason.as_str(), r.in_slice))
            .collect();
        assert_eq!(
            reasons,
            [
                ("1", "no use of *initiator*", 0),
                ("3.1", "below depth 1, 1 in slice", 1),
                ("4.1", "below depth 1, 1 in slice", 1),
                ("6", "no use of *initiator*", 0),
                ("9", "no use of *initiator*", 0),
            ]
        );
    }

    #[test]
    fn degenerate_views() {
        let index = navish();
        let s = slice(&index, &involving(&["documentResource"])).unwrap();
        assert_eq!(roles(&s), [("1".to_string(), StepRole::Match)]);
        assert_eq!(runs(&s), [(None, "2", "9", 10)]);
        let s = slice(
            &index,
            &ViewRequest {
                depth: Some(99),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((s.steps.len(), s.omitted.len()), (11, 0));
        let all = slice_index("go", &[("1", &["x"]), ("2", &["x"])], &[], &[]);
        let s = slice(&all, &involving(&["x"])).unwrap();
        assert!(s.omitted.is_empty());
        let one = slice_index("go", &[("1", &["x"])], &[], &[]);
        let s = slice(
            &one,
            &ViewRequest {
                steps: vec!["1".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((s.steps.len(), s.omitted.len()), (1, 0));
        let bare = slice_index("go", &[("1", &[]), ("2", &["x"])], &[], &[]);
        let s = slice(&bare, &feeding("1")).unwrap();
        assert_eq!(roles(&s), [("1".to_string(), StepRole::Target)]);
    }

    #[test]
    fn conservation_holds_for_every_view_kind() {
        let index = navish();
        let mut views = vec![
            ViewRequest {
                depth: Some(1),
                ..Default::default()
            },
            ViewRequest {
                depth: Some(2),
                ..Default::default()
            },
        ];
        for path in index.steps.iter().map(|s| s.path.clone()) {
            views.push(ViewRequest {
                feeding: Some(FeedingSelector {
                    step: path.clone(),
                    variables: vec![],
                }),
                ..Default::default()
            });
            views.push(ViewRequest {
                steps: vec![path.clone()],
                ..Default::default()
            });
        }
        for name in &index.vars {
            views.push(ViewRequest {
                depth: Some(1),
                ..involving(&[name.as_str()])
            });
        }
        for view in views {
            let s = slice(&index, &view).unwrap();
            let omitted: u32 = s.omitted.iter().map(|r| r.steps).sum();
            assert_eq!(s.steps.len() as u32 + omitted, 11, "{view:?}");
            for k in &s.steps {
                let i = index.step_by_path(&k.path).unwrap();
                if let Some(p) = index.steps[i].parent {
                    assert!(
                        s.steps
                            .iter()
                            .any(|q| q.path == index.steps[p as usize].path),
                        "context closure: {view:?} {}",
                        k.path
                    );
                }
            }
        }
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
