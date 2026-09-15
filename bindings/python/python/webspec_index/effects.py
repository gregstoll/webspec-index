"""Typed dictionary contracts for the versioned, UI-independent effects API.

Results describe possible behavior in indexed specification text. An empty
effect list is not a guarantee that an algorithm has no effects.
"""
from typing import Any, Literal, Optional, TypedDict, Union

EffectValue = Optional[Union[str, bool, int]]
Execution = Literal["inline", "separate", "unknown"]


class _SubjectBase(TypedDict):
    spec: str
    anchor: str


class SubjectSelector(_SubjectBase, total=False):
    step_path: list[int]
    step_id: str
    body_id: str


class Subject(SubjectSelector):
    snapshot_sha: str


class DiscoveryBudgets(TypedDict):
    max_bodies: int
    max_relationships: int
    max_states: int


class EffectsOptions(TypedDict, total=False):
    mode: Literal["auto", "cached", "off"]
    rule_paths: list[str]
    environment: str
    analysis_id: str
    budgets: DiscoveryBudgets


class EffectFilter(TypedDict, total=False):
    kind: str
    category: str
    rule_id: str
    effect_id: str
    occurrence_id: str


class _EffectsRequestBase(TypedDict):
    schema_version: Literal[1]
    subject: SubjectSelector


class EffectsRequest(_EffectsRequestBase, total=False):
    options: EffectsOptions
    filter: EffectFilter


class ExplanationOptions(TypedDict, total=False):
    max_depth: int
    max_states: int
    limit: int


class ExplainEffectsRequest(EffectsRequest, total=False):
    explanation: ExplanationOptions


class _RecomputeRequestBase(TypedDict):
    schema_version: Literal[1]
    scope: dict[str, Any]


class RecomputeEffectsRequest(_RecomputeRequestBase, total=False):
    options: EffectsOptions


class _EffectLocationBase(TypedDict):
    spec: str
    anchor: str
    url: str


class EffectLocation(_EffectLocationBase, total=False):
    step_path: list[int]


class _EffectSummaryBase(TypedDict):
    id: str
    kind: str
    params: dict[str, EffectValue]
    execution: list[Execution]


class EffectSummary(_EffectSummaryBase, total=False):
    location: EffectLocation
    other_locations: list[EffectLocation]
    additional_locations: int


class _StatusBase(TypedDict):
    state: Literal["ready", "pending", "unavailable", "error", "disabled"]
    semantics: Literal["may"]
    issues: list[str]
    omitted: int


class EffectsStatus(_StatusBase, total=False):
    coverage: Literal["complete", "partial"]
    analysis_id: str


class DefinedBody(TypedDict):
    subject: Subject
    effects: list[EffectSummary]
    effects_status: EffectsStatus


class _SummaryBase(TypedDict):
    schema_version: Literal[1]
    subject: Subject
    effects: list[EffectSummary]
    effects_status: EffectsStatus
    defined_bodies: list[DefinedBody]
    issues: list[dict[str, Any]]


class EffectSummaryResult(_SummaryBase, total=False):
    input_manifest: dict[str, Any]


class ExplainEffectsResult(EffectSummaryResult):
    explanations: list[dict[str, Any]]


class RecomputeEffectsResult(TypedDict):
    schema_version: Literal[1]
    analysis_id: str
    input_manifest: dict[str, Any]
    processed_subjects: list[Subject]
    unprocessed_subjects: list[Subject]
    body_count: int
    relationship_count: int
    state_count: int
    issues: list[dict[str, Any]]
