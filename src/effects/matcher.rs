//! Local catalog matching over the parser's canonical inline representation.

use crate::effects::catalog::{
    Anchor, CaptureSource, Catalog, Emit, EmitValue, ParameterType, Rule,
};
use crate::effects::model::{EffectParams, EffectValue, EvidenceBasis, IssueCode, Span};
use crate::parse::steps::{
    AnchorTarget, InlineTokenKind, OperationSite, StructuralAlgorithm, StructuralSegment,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct MatchedEffect {
    pub kind: String,
    pub params: EffectParams,
    pub rule_ids: BTreeSet<String>,
    pub basis: EvidenceBasis,
    pub captures: BTreeMap<String, Option<String>>,
    pub arguments: BTreeMap<String, String>,
    pub span: Option<Span>,
    pub issues: BTreeSet<IssueCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationMatch {
    pub effects: Vec<MatchedEffect>,
    pub issues: BTreeSet<IssueCode>,
}

pub(crate) fn match_operation_with_issues(
    catalog: &Catalog,
    algorithm: &StructuralAlgorithm,
    segment: &StructuralSegment,
    operation: &OperationSite,
) -> OperationMatch {
    let Some(target) = &operation.target else {
        return OperationMatch {
            effects: Vec::new(),
            issues: BTreeSet::new(),
        };
    };
    let subject = Anchor {
        spec: algorithm.source.spec.clone(),
        anchor: algorithm.source.section_anchor.clone(),
    };
    let mut found = Vec::new();
    let mut ambiguous_kinds = BTreeSet::new();
    for rule in catalog.rules() {
        // Rules without an endpoint anchor are segment rules. Applying them
        // once per link would duplicate effects and would miss link-free text.
        if rule.match_spec.anchor.is_none()
            || !anchor_matches(rule.match_spec.anchor.as_ref(), target)
            || rule
                .match_spec
                .subject
                .as_ref()
                .is_some_and(|expected| expected != &subject)
            || rule
                .match_spec
                .exclude_text
                .iter()
                .any(|pattern| pattern.regex().is_match(&segment.text))
        {
            continue;
        }
        if let Some(pattern) = &rule.match_spec.text {
            for captures in pattern.regex().captures_iter(&segment.text) {
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                // A text specialization belongs to this operation only when the
                // complete match contains this operation's linked name. This is
                // what prevents one broad segment match from specializing two calls.
                if whole.start() > operation.span.start || whole.end() < operation.span.end {
                    continue;
                }
                let matching_references = algorithm
                    .operation_sites
                    .iter()
                    .filter(|candidate| {
                        candidate.role == crate::parse::steps::ReferenceRole::Operation
                            && candidate.segment_id == operation.segment_id
                            && candidate.target.as_ref().is_some_and(|target| {
                                anchor_matches(rule.match_spec.anchor.as_ref(), target)
                            })
                            && whole.start() <= candidate.span.start
                            && whole.end() >= candidate.span.end
                    })
                    .count();
                if matching_references != 1 {
                    ambiguous_kinds.insert(rule.emit.kind.clone());
                    continue;
                }
                found.push(emit_match(
                    catalog,
                    rule,
                    &rule.emit,
                    segment,
                    Some(&captures),
                    Some((whole.start(), whole.end())),
                ));
            }
        } else {
            found.push(emit_match(catalog, rule, &rule.emit, segment, None, None));
        }
    }
    let mut found = specialize(found);
    for matched in &mut found {
        if ambiguous_kinds.contains(&matched.kind) {
            matched.issues.insert(IssueCode::AmbiguousMatch);
            matched.issues.insert(IssueCode::UnresolvedArgument);
        }
    }
    let issues = if ambiguous_kinds.is_empty() {
        BTreeSet::new()
    } else {
        BTreeSet::from([IssueCode::AmbiguousMatch, IssueCode::UnresolvedArgument])
    };
    OperationMatch {
        effects: found,
        issues,
    }
}

/// Match anchorless rules once against an executable segment.
///
/// Specialization is restricted to emissions from the same regex occurrence;
/// equal text appearing twice remains two distinct source origins.
pub(crate) fn match_segment(
    catalog: &Catalog,
    algorithm: &StructuralAlgorithm,
    segment: &StructuralSegment,
) -> Vec<MatchedEffect> {
    let subject = Anchor {
        spec: algorithm.source.spec.clone(),
        anchor: algorithm.source.section_anchor.clone(),
    };
    let mut by_span: BTreeMap<(usize, usize), Vec<MatchedEffect>> = BTreeMap::new();
    for rule in catalog
        .rules()
        .filter(|rule| rule.match_spec.anchor.is_none())
    {
        if rule
            .match_spec
            .subject
            .as_ref()
            .is_some_and(|expected| expected != &subject)
            || rule
                .match_spec
                .exclude_text
                .iter()
                .any(|pattern| pattern.regex().is_match(&segment.text))
        {
            continue;
        }
        let Some(pattern) = &rule.match_spec.text else {
            continue;
        };
        for captures in pattern.regex().captures_iter(&segment.text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            by_span
                .entry((whole.start(), whole.end()))
                .or_default()
                .push(emit_match(
                    catalog,
                    rule,
                    &rule.emit,
                    segment,
                    Some(&captures),
                    Some((whole.start(), whole.end())),
                ));
        }
    }
    by_span.into_values().flat_map(specialize).collect()
}

pub(crate) fn intrinsic_rules(catalog: &Catalog) -> Vec<(&Rule, MatchedEffect)> {
    catalog
        .rules()
        .filter(|rule| {
            rule.match_spec.anchor.is_some()
                && rule.match_spec.subject.is_none()
                && rule.match_spec.text.is_none()
                && rule.match_spec.exclude_text.is_empty()
        })
        .map(|rule| {
            let params = rule
                .emit
                .params
                .iter()
                .map(|(name, value)| {
                    let value = match value {
                        EmitValue::Unknown => None,
                        EmitValue::Constant(value) => Some(value.clone()),
                        EmitValue::Capture { .. } => {
                            unreachable!("catalog validation rejects this")
                        }
                    };
                    (name.clone(), value)
                })
                .collect();
            (
                rule,
                MatchedEffect {
                    kind: rule.emit.kind.clone(),
                    params,
                    rule_ids: BTreeSet::from([rule.public_id.clone()]),
                    basis: EvidenceBasis::Matched,
                    captures: BTreeMap::new(),
                    arguments: BTreeMap::new(),
                    span: None,
                    issues: BTreeSet::new(),
                },
            )
        })
        .collect()
}

pub(crate) fn continuation_modes(
    catalog: &Catalog,
    algorithm: &StructuralAlgorithm,
    segment: &StructuralSegment,
    operation: &OperationSite,
) -> BTreeSet<crate::effects::model::Execution> {
    let Some(target) = &operation.target else {
        return BTreeSet::new();
    };
    let subject = Anchor {
        spec: algorithm.source.spec.clone(),
        anchor: algorithm.source.section_anchor.clone(),
    };
    catalog
        .rules()
        .filter(|rule| {
            rule.match_spec.anchor.is_some()
                && anchor_matches(rule.match_spec.anchor.as_ref(), target)
                && rule
                    .match_spec
                    .subject
                    .as_ref()
                    .is_none_or(|expected| expected == &subject)
                && !rule
                    .match_spec
                    .exclude_text
                    .iter()
                    .any(|pattern| pattern.regex().is_match(&segment.text))
                && rule.match_spec.text.as_ref().is_none_or(|pattern| {
                    pattern.regex().find_iter(&segment.text).any(|whole| {
                        whole.start() <= operation.span.start && whole.end() >= operation.span.end
                    })
                })
        })
        .filter_map(|rule| rule.continuations)
        .collect()
}

fn anchor_matches(expected: Option<&Anchor>, actual: &AnchorTarget) -> bool {
    expected.is_none_or(|expected| expected.spec == actual.spec && expected.anchor == actual.anchor)
}

fn emit_match(
    catalog: &Catalog,
    rule: &Rule,
    emit: &Emit,
    segment: &StructuralSegment,
    captures: Option<&regex::Captures<'_>>,
    span: Option<(usize, usize)>,
) -> MatchedEffect {
    let definition = &catalog.effects[&emit.kind];
    let mut params = BTreeMap::new();
    let mut captured = BTreeMap::new();
    let mut arguments = BTreeMap::new();
    let mut issues = BTreeSet::new();
    for (name, parameter) in &definition.parameters {
        let emit_value = emit.params.get(name).unwrap_or(&EmitValue::Unknown);
        let value = match emit_value {
            EmitValue::Unknown => None,
            EmitValue::Constant(value) => Some(value.clone()),
            EmitValue::Capture { capture, from } => {
                let capture_match = captures.and_then(|values| values.name(capture));
                let source = from.unwrap_or(match parameter.parameter_type {
                    ParameterType::Anchor => CaptureSource::Anchor,
                    _ => CaptureSource::Literal,
                });
                let extracted = capture_match.and_then(|value| {
                    arguments.insert(name.clone(), value.as_str().to_owned());
                    extract_capture(segment, value.start(), value.end(), source)
                });
                captured.insert(capture.clone(), extracted.clone());
                if extracted.is_none() {
                    issues.insert(IssueCode::UnresolvedArgument);
                }
                extracted.map(EffectValue::String)
            }
        };
        params.insert(name.clone(), value);
    }
    MatchedEffect {
        kind: emit.kind.clone(),
        params,
        rule_ids: BTreeSet::from([rule.public_id.clone()]),
        basis: EvidenceBasis::Matched,
        captures: captured,
        arguments,
        span: span.map(|(start, end)| Span {
            start: start as u64,
            end: end as u64,
        }),
        issues,
    }
}

fn extract_capture(
    segment: &StructuralSegment,
    start: usize,
    end: usize,
    source: CaptureSource,
) -> Option<String> {
    match source {
        CaptureSource::Text => segment.text.get(start..end).map(str::to_owned),
        CaptureSource::Literal => {
            let token = segment.tokens.iter().find(|token| {
                token.span.start == start
                    && token.span.end == end
                    && token.kind == InlineTokenKind::Literal
            })?;
            let raw = segment.text.get(start..end)?;
            Some(raw.trim_matches(['`', '"', '\'']).to_owned())
                .filter(|v| !v.is_empty())
                .or_else(|| Some(token.source_text.clone()))
        }
        CaptureSource::Anchor => segment
            .links
            .iter()
            .find(|link| link.span.start == start && link.span.end == end)
            .and_then(|link| link.target.as_ref())
            .map(|target| format!("{}#{}", target.spec, target.anchor)),
    }
}

fn specialize(matches: Vec<MatchedEffect>) -> Vec<MatchedEffect> {
    let mut result: Vec<MatchedEffect> = Vec::new();
    for candidate in matches {
        if let Some(equal) = result
            .iter_mut()
            .find(|item| item.kind == candidate.kind && item.params == candidate.params)
        {
            equal.rule_ids.extend(candidate.rule_ids);
            equal.captures.extend(candidate.captures);
            equal.arguments.extend(candidate.arguments);
            equal.issues.extend(candidate.issues);
        } else {
            result.push(candidate);
        }
    }
    let mut keep = vec![true; result.len()];
    for less in 0..result.len() {
        let dominators: Vec<usize> = (0..result.len())
            .filter(|&more| more != less && dominates(&result[more], &result[less]))
            .collect();
        if !dominators.is_empty() {
            keep[less] = false;
            let provenance = result[less].rule_ids.clone();
            for more in dominators {
                result[more].rule_ids.extend(provenance.clone());
            }
        }
    }
    for left in 0..result.len() {
        for right in (left + 1)..result.len() {
            if result[left].kind == result[right].kind
                && conflicting_known_values(&result[left].params, &result[right].params)
            {
                result[left].issues.insert(IssueCode::AmbiguousMatch);
                result[right].issues.insert(IssueCode::AmbiguousMatch);
            }
        }
    }
    result
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect()
}

fn conflicting_known_values(left: &EffectParams, right: &EffectParams) -> bool {
    left.iter().any(|(name, left)| {
        matches!((left, right.get(name)), (Some(left), Some(Some(right))) if left != right)
    })
}

fn dominates(more: &MatchedEffect, less: &MatchedEffect) -> bool {
    if more.kind != less.kind || more.params.len() != less.params.len() {
        return false;
    }
    let mut added = false;
    for (name, less_value) in &less.params {
        let Some(more_value) = more.params.get(name) else {
            return false;
        };
        match (less_value, more_value) {
            (Some(left), Some(right)) if left != right => return false,
            (Some(_), None) => return false,
            (None, Some(_)) => added = true,
            _ => {}
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::catalog::{load_catalog, load_package_files};
    use crate::parse::steps::extract_step_structure;

    fn matches(html: &str, yaml: &str) -> Vec<Vec<MatchedEffect>> {
        matches_with_issues(html, yaml)
            .into_iter()
            .map(|matched| matched.effects)
            .collect()
    }

    fn matches_with_issues(html: &str, yaml: &str) -> Vec<OperationMatch> {
        let package = load_package_files(&[("catalog.yaml", yaml)]).unwrap();
        let catalog = load_catalog([package]).unwrap();
        let structure =
            extract_step_structure(html, "TEST", "https://example.test/spec", &"a".repeat(64));
        let algorithm = &structure.algorithms[0];
        algorithm
            .operation_sites
            .iter()
            .map(|operation| {
                let segment = algorithm
                    .segments
                    .iter()
                    .find(|segment| segment.source.node_id == operation.segment_id)
                    .unwrap();
                match_operation_with_issues(&catalog, algorithm, segment, operation)
            })
            .collect()
    }

    #[test]
    fn broad_anchor_text_match_does_not_specialize_multiple_operations() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="direct">run direct</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>;
          then <a href="#fire">fire an event</a> named <code>"b"</code>.</p></li></ol>
        </div>"##;
        let yaml = r#"schema: 1
package: test
effects:
  event.fire:
    category: events
    label: fire an event
    parameters:
      name: {type: string}
rules:
  - id: generic
    match: {anchor: TEST#fire}
    emit: {kind: event.fire, params: {name: null}}
  - id: broad
    match:
      anchor: TEST#fire
      text: '(?i)fire an event named `(?P<name>[^`]+)`.*fire an event'
    emit:
      kind: event.fire
      params:
        name: {capture: name, from: literal}
"#;
        let matches = matches(html, yaml);
        assert_eq!(matches.len(), 2);
        for operation_matches in matches {
            assert_eq!(operation_matches.len(), 1);
            assert_eq!(operation_matches[0].params["name"], None);
            assert!(operation_matches[0]
                .issues
                .contains(&IssueCode::AmbiguousMatch));
            assert!(operation_matches[0]
                .issues
                .contains(&IssueCode::UnresolvedArgument));
        }
    }

    #[test]
    fn capture_must_cover_the_complete_literal_token() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="direct">run direct</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"foobar"</code>.</p></li></ol>
        </div>"##;
        let yaml = r#"schema: 1
package: test
effects:
  event.fire:
    category: events
    label: fire an event
    parameters:
      name: {type: string}
rules:
  - id: generic
    match: {anchor: TEST#fire}
    emit: {kind: event.fire, params: {name: null}}
  - id: partial
    match:
      anchor: TEST#fire
      text: '(?i)fire an event named `"(?P<name>foo)bar"`'
    emit:
      kind: event.fire
      params:
        name: {capture: name, from: literal}
"#;
        let matches = matches(html, yaml);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].len(), 1);
        assert_eq!(matches[0][0].params["name"], None);
        assert!(matches[0][0]
            .issues
            .contains(&IssueCode::UnresolvedArgument));
    }

    #[test]
    fn anchor_capture_must_cover_the_complete_link_text() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="direct">run direct</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a>.</p></li></ol>
        </div>"##;
        let yaml = r#"schema: 1
package: test
effects:
  operation.observe:
    category: script
    label: observe an operation
    parameters:
      target: {type: anchor}
rules:
  - id: generic
    match: {anchor: TEST#fire}
    emit: {kind: operation.observe, params: {target: null}}
  - id: partial
    match:
      anchor: TEST#fire
      text: '(?i)fire an (?P<target>event)'
    emit:
      kind: operation.observe
      params:
        target: {capture: target, from: anchor}
"#;
        let matches = matches(html, yaml);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].len(), 1);
        assert_eq!(matches[0][0].params["target"], None);
        assert!(matches[0][0]
            .issues
            .contains(&IssueCode::UnresolvedArgument));
    }

    #[test]
    fn ambiguous_span_without_generic_rule_keeps_a_site_diagnostic() {
        let html = r##"<div class="algorithm">
          <p>To <dfn id="direct">run direct</dfn>:</p>
          <ol><li><p><a href="#fire">Fire an event</a> named <code>"a"</code>;
          then <a href="#fire">fire an event</a> named <code>"b"</code>.</p></li></ol>
        </div>"##;
        let yaml = r#"schema: 1
package: test
effects:
  event.fire:
    category: events
    label: fire an event
    parameters:
      name: {type: string}
rules:
  - id: broad
    match:
      anchor: TEST#fire
      text: '(?i)fire an event named `(?P<name>[^`]+)`.*fire an event'
    emit:
      kind: event.fire
      params:
        name: {capture: name, from: literal}
"#;
        let matches = matches_with_issues(html, yaml);
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|matched| matched.effects.is_empty()));
        assert!(matches.iter().all(|matched| {
            matched.issues.contains(&IssueCode::AmbiguousMatch)
                && matched.issues.contains(&IssueCode::UnresolvedArgument)
        }));
    }
}
