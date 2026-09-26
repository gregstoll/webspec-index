# Possible writers

The `state` command answers: **who writes a field, and under what shape**. An answer uses
**may-semantics**: a listed site may set the field but no reachability or feasibility proof
is made. Reads are counted but not listed — use the count to know how many algorithms guard
on this field.

**Write** vs **initialization**: a write (Set/Unset/flag toggle/mutation) occurs in ordinary
algorithm flow. An initialization is a construction form (`Let d be a new Document, with:
field → value`). Both are shown by default; pass `--no-inits` to hide initializations.

Every answer carries issue codes and coverage counts, explained in the sections below.

```sh
webspec-index state HTML#is-initial-about:blank --format markdown
webspec-index state "Document.*sandbox*"
webspec-index state Document
webspec-index state --coverage HTML
```

## Selector grammar

| Form | Example | Result |
|---|---|---|
| `SPEC#anchor` of a field | `HTML#is-initial-about:blank` | field view |
| `SPEC#anchor` of a type anchor | `DOM#interface-node`, `HTML#navigable` | type view |
| `TYPE` | `Document`, `navigable`, `"environment settings object"` | type view (IDL name exact, then concept name) |
| `TYPE.FIELD` | `"Element.node document"` | field view via inheritance lookup (shows path) |
| `TYPE.GLOB` | `"Document.*sandbox*"` | field list: own and inherited fields of TYPE whose name or anchor matches |
| `SPEC#GLOB` | `"HTML#*sandbox*"` | field list over the spec's fields by anchor or name |

`SPEC#anchor` of a set member gives the member view. A full spec URL is accepted wherever
`SPEC#anchor` is. Glob matching is case-insensitive (`*` matches any substring). `--limit N`
caps rows per group (default 50 for field views, 20 for field lists, no cap for type views).

## Field view

```sh
webspec-index state HTML#is-initial-about:blank --format markdown
```

```
## HTML#is-initial-about:blank — is initial `about:blank`

- Owner: `Document` (idl:Document) — declaration sentence, declared in HTML#the-document-object:
  "Each Document has an is initial about:blank, which is a boolean, initially false."
- Type: boolean · Initial: false

### Writes (1)
- HTML#document-open-steps:13 — Set *document*'s is initial `about:blank` to false.

### Initializations (1)
- HTML#creating-a-new-browsing-context:15 — Let *document* be a new `Document`, with:
  is initial `about:blank` → true

Coverage: may · complete — 0 unclassified, 0 possible unlinked writes. Reads are not listed
(9 occurrences).
```

## Type view

```sh
webspec-index state Document --format markdown
```

```
## Document (idl:Document)
Anchors: DOM#concept-document (concept alias), DOM#document (defining), HTML#document (partial), …
Supertypes: Node → EventTarget
Includes: NonElementParentNode, DocumentOrShadowRoot, ParentNode, XPathEvaluatorBase (DOM); GlobalEventHandlers (HTML)

### Fields (declared)
| Field | Type | Initial | Basis | Writes | Inits |
|---|---|---|---|---:|---:|
| DOM#document-allow-declarative-shadow-roots allow declarative shadow roots | boolean | — | dfn-for | 0 | 0 |
| DOM#concept-document-content-type content type | string | — | dfn-for | 1 | 5 |
| DOM#document-custom-element-registry custom element registry | null or ⟦L0⟧ object | — | dfn-for | 2 | 2 |
| DOM#concept-document-encoding encoding | encoding | — | dfn-for | 2 | 0 |
| DOM#concept-document-mode mode | — | — | dfn-for | 4 | 1 |
| DOM#concept-document-origin origin | origin | — | dfn-for | 1 | 5 |
| DOM#concept-document-type type | — | — | dfn-for | 2 | 2 |
| DOM#concept-document-url URL | URL | — | dfn-for | 2 | 2 |
| HTML#concept-document-about-base-url about base URL | URL or null | null | sentence | 0 | 2 |
| HTML#active-parser-was-aborted active parser was aborted | boolean | false | sentence | 1 | 0 |
| HTML#active-sandboxing-flag-set active sandboxing flag set | sandboxing flag set | — | dfn-for | 0 | 2 |
| HTML#autofocus-candidates autofocus candidates | — | empty | sentence | 4 | 0 |
| HTML#current-document-readiness current document readiness | string | "complete" | sentence | 1 | 1 |
| HTML#design-mode-enabled design mode enabled | boolean | false | sentence | 2 | 0 |
| HTML#is-initial-about:blank is initial `about:blank` | boolean | false | sentence | 1 | 1 |
| HTML#latest-entry latest entry | session history entry or null | — | sentence | 2 | 0 |
| HTML#concept-document-coop opener policy | opener policy | a new opener policy | sentence | 1 | 1 |
| HTML#page-showing page showing | boolean | true | sentence | 3 | 0 |
| HTML#concept-document-policy-container policy container | policy container | a new policy container | dfn-for | 1 | 1 |
| HTML#concept-document-salvageable salvageable | — | true | sentence | 4 | 0 |
… (trimmed — 47 more declared fields, plus inherited from Node, EventTarget, and concept-tree)

### Fields with `data-dfn-for` but no recognized declaration
| Field | Type | Initial | Basis | Writes | Inits |
|---|---|---|---|---:|---:|
| HTML#concept-document-bc `Document`'s browsing context | — | null | dfn-for | 1 | 2 |

Coverage: may · partial — 7 unclassified in own fields.
```

The output lists declared fields, `data-dfn-for` fields without a recognized declaration,
and inherited fields from each supertype, each in its own table. Pass `--limit N` to cap
rows per table.

## Field list (glob)

```sh
webspec-index state "Document.*sandbox*" --format markdown
```

```
## Document.*sandbox* — 1 fields

| Field | Type | Initial | Basis | Writes | Inits | Unclassified |
|---|---|---|---|---:|---:|---:|
| HTML#active-sandboxing-flag-set active sandboxing flag set | sandboxing flag set | — | dfn-for | 0 | 2 | 0 |

### HTML#active-sandboxing-flag-set active sandboxing flag set
Owner: `Document` (idl:Document)
Initializations (2):
- HTML#creating-a-new-browsing-context:15 — Let *document* be a new `Document`, with:
  active sandboxing flag set → *sandboxFlags*
- HTML#initialise-the-document-object:9 — Let *document* be a new `Document`, with
  active sandboxing flag set → *navigationParams*'s final sandboxing flag set

Coverage: may · complete — 0 unclassified in listed fields.
```

## Member view (set)

When the anchor is a set concept, the command shows members and their add/remove site counts
instead of fields:

```sh
webspec-index state HTML#sandboxing-flag-set --format markdown
```

```
## sandboxing flag set (HTML#sandboxing-flag-set)
Anchors: HTML#sandboxing-flag-set (defining)

### Members
| Member | Adds | Removes |
|---|---:|---:|
| HTML#one-permitted-sandboxed-navigator one permitted sandboxed navigator | 1 | 0 |
| HTML#sandbox-propagates-to-auxiliary-browsing-contexts-flag sandbox propagates to auxiliary browsing contexts flag | 0 | 0 |
| HTML#sandboxed-automatic-features-browsing-context-flag sandboxed automatic features browsing context flag | 0 | 0 |
| HTML#sandboxed-auxiliary-navigation-browsing-context-flag sandboxed auxiliary navigation browsing context flag | 0 | 0 |
| HTML#sandboxed-custom-protocols-navigation-browsing-context-flag sandboxed custom protocols navigation browsing context flag | 0 | 0 |
| HTML#sandboxed-document.domain-browsing-context-flag sandboxed document.domain browsing context flag | 0 | 0 |
| HTML#sandboxed-downloads-browsing-context-flag sandboxed downloads browsing context flag | 0 | 0 |
| HTML#sandboxed-forms-browsing-context-flag sandboxed forms browsing context flag | 0 | 0 |
| HTML#sandboxed-modals-flag sandboxed modals flag | 0 | 0 |
| HTML#sandboxed-navigation-browsing-context-flag sandboxed navigation browsing context flag | 0 | 0 |
| HTML#sandboxed-origin-browsing-context-flag sandboxed origin browsing context flag | 0 | 0 |
| HTML#sandboxed-scripts-browsing-context-flag sandboxed scripts browsing context flag | 0 | 0 |
| HTML#sandboxed-top-level-navigation-with-user-activation-browsing-context-flag sandboxed top-level navigation with user activation browsing context flag | 0 | 0 |
| HTML#sandboxed-top-level-navigation-without-user-activation-browsing-context-flag sandboxed top-level navigation without user activation browsing context flag | 0 | 0 |

Coverage: may · complete — 0 unclassified in own fields.
```

## Coverage view

```sh
webspec-index state --coverage HTML --format markdown
```

```
## State coverage — HTML

| Rule | Concept dfns |
|---|---:|
| declaration_sentence | 284 |
| dfn_for | 212 |
| override | 72 |
| property_list | 44 |
| struct_items | 94 |

| Written fields | Count |
|---|---:|
| Total | 450 |
| declaration_sentence | 197 |
| dfn_for | 91 |
| override | 72 |
| property_list | 18 |
| struct_items | 43 |

Resolved: 421 / 450 (93.6%)

| Form | Count |
|---|---:|
| init:dl_entries | 283 |
| init:whose_list | 48 |
| let | 1898 |
| mutate:append:field:var | 27 |
| mutate:append:var | 118 |
| mutate:map_set:var_subscript | 63 |
| opaque:pronoun_root:set | 35 |
| opaque:unsupported_form:add | 102 |
| opaque:unsupported_form:append | 183 |
| set:to:field:var | 633 |
| set:to:var | 615 |
… (trimmed — 57 more form rows)

Set statements with a structured target: 1051 / 1697 (61.9%)

| Class | Count |
|---|---:|
| init | 269 |
| read | 2212 |
| read_path | 76 |
| unclassified | 76 |
| write | 907 |

Unclassified: 2.1%

| Prose | Count |
|---|---:|
| Sources | 719 |
| Callouts excluded | 32 |
| Mentions | 32703 |

### Unclassified review (527, all targets)
- HTML#rules-for-parsing-a-list-of-dimensions:5.7.2 — INFRA#ascii-whitespace — Remove all ASCII whitespace in *s*.
- HTML#start-intersection-observing-a-lazy-loading-element:2 — INTERSECTIONOBSERVER#intersectionobserver — If *doc*'s lazy load intersection observer is null, set it to a new `IntersectionObserver` instance, initialized as follows:
- HTML#update-the-image-data:7.4.1 — HTML#ignore-higher-layer-caching — Set the ignore higher-layer caching flag for that entry.
… (trimmed — 524 more unclassified entries)
```

Prints the stored counters for one spec: owner inference by rule, statement forms,
occurrence classes, prose sources, and a review list of unclassified occurrences (subject,
step path, text). Use this to check regression floors.

## Coverage and issue codes

A field view is **complete** only when the field has zero unclassified occurrences, zero
possible unlinked writes, and a resolved owner. Otherwise the status is **partial**.

| Issue code | Meaning |
|---|---|
| `owner_unknown` | field owner not inferred and not overridden (hint shown) |
| `owner_conflict` | R1 disagrees with R2–R4; R1 kept |
| `field_not_declared` | sites target the anchor, but it is not a field; sites are listed |
| `ambiguous_selector` / `ambiguous_field` | several types or fields match; candidates listed |
| `unresolved_link` | a target hop link without a resolved anchor |
| `missing_spec` | a type or owner referenced by name or link whose spec is not indexed |
| `unclassified_occurrence` | count > 0 for this field |
| `possible_unlinked_write` | opaque write whose target text contains the field name |
| `declaration_mismatch` | a YAML declaration's `expect_text` did not match; ignored |
| `outside_structure` | informational: some writers come from prose sources |

A missing target spec never fails a query; writes into an unindexed spec's fields are still
listed under the anchor.

## Prose sources and their limit

The state layer scans normative sentences that carry an explicit verb ("The X method steps
are to set …", "User agents must set …") and attributes them as write sites. Only sentences
where the subject resolves to an indexed spec anchor are picked up. Descriptive prose — "It
is set when the Window object is created" — stays unclassified or uncounted. If a write is
described only in explanatory prose with no verb, it will not appear in `state` output.

## TC39 limitation

ECMA-262 clauses that contain several `<emu-alg>` blocks (e.g. an overloaded operation
defined by multiple abstract algorithms) are indexed with the **last** algorithm of the
clause. Writers discovered in earlier algorithms of the same clause are attributed to the
clause anchor but their step paths reflect only the last algorithm's numbering. This affects
approximately 147 ECMA-262 clauses. The symptom is a step path that does not match the
clause text at the cited location.

## Reflection

Content-attribute reflection is tracked separately. Query the IDL-attribute anchor to see
the reflection relationship:

```sh
webspec-index state HTML#dom-a-target --format markdown
```

```
## HTML#dom-a-target — target

- Not a declared field.
- Reflects: the `target` content attribute (HTML#attr-hyperlink-target) — [Reflect]

### Writes (0)

### Initializations (0)

Coverage: may · partial — 0 unclassified, 0 possible unlinked writes. Reads are not listed
(0 occurrences).
```

The reflection line confirms that the IDL setter also sets the content attribute. Query the
content-attribute anchor (`HTML#attr-hyperlink-target`) directly for its declared-field writes.

An IDL attribute that is _also_ a declared field (because the spec declares it with `has a`)
shows no reflection line. Reflections whose content-attribute definition is not indexed cannot
be queried from the IDL anchor.

## Set members

`state SPEC#anchor` where the anchor is a set concept (e.g. `HTML#sandboxing-flag-set`)
shows the member view: each member anchor and name, with the count of sites that add or
remove it. Individual flag anchors like `HTML#sandboxed-scripts-browsing-context-flag` also
resolve to the member view.

## Slices and outlines

Four flags of `query` cut a stored algorithm to a focused view. Every omitted step is counted in a
marker; nothing is dropped silently.

```sh
webspec-index query HTML#navigate --involving historyHandling --format markdown
webspec-index query HTML#navigate --feeding 24.9.1 --format markdown
webspec-index query HTML#navigate --depth 1 --format markdown
webspec-index query HTML#navigate --steps 24 --format markdown
```

`--involving`, `--feeding` and `--steps` are mutually exclusive. `--depth N` combines with any of
them. Plain `query` output is unchanged.

### Forward slice: `--involving VAR[,VAR]`

Answers: "what steps does this variable influence?"

The **seeds** are the given names. Each must be mentioned by some step in the algorithm; if not, the
query returns `slice_unknown_variable` and lists the algorithm's variables in first-mention order.

**Slice variables** start as the seeds and grow to a fixed point: a definition edge of kind `Let`,
`Set`, or local `Mutate` whose uses intersect the slice variables adds the variable it defines. The
closure is flow-insensitive — a variable in the slice is in the slice at every step — matching
may-semantics. Each derived variable records the edge kind, the step, and which slice variables were
in its uses.

**Matched steps** are steps whose own text mentions a slice variable. Own text is the step's segments
and branch labels; child steps and notes are not own text. A step that rebinds a slice variable
(`Set *historyHandling* to "push"`) is also matched.

**Context steps** are the proper path prefixes of matched steps. Their text is rendered in full; their
non-kept children are elided.

**Inherited steps** are var-less children of matched steps (steps mentioning no variable at all). They
can only be read in terms of their matched parent: "Return.", "Abort these steps.", pronoun steps such
as "Remove its children." A child that mentions other variables is not inherited.

**Stores** — a `Set`/`Mutate` whose target has hops or a non-variable root (`Set *d*'s origin to *v*`)
is reported under `Stored into object state, not followed` with the step, the target path, and the
slice variables in its value. The root variable does not join the slice. Use `--involving` from that
root to follow it.

### Backward slice: `--feeding STEP[:VAR,…]`

Answers: "which earlier steps define the variables that step STEP uses?"

The **target** is the step with the given path; an unknown path returns `slice_unknown_step`. The
**seeds** are the variables the target mentions, or the given subset (each must be mentioned by the
target; otherwise `slice_invalid_selector` lists the target's variables).

Kept steps: the target (role `target`), all definition edges of slice variables at steps before the
target in document order (role `definition`), and the context steps of all kept steps. **Field stores
into a variable's object** (`Set *documentState*'s origin to`) count as backward definitions — the
step that built up the object feeds the using step.

Also listed:
- **Inputs** — slice variables with no definition edge anywhere in the algorithm (parameters, or
  variables bound by forms the IR does not yet parse: loop variables, named-body parameters).
- **Later definitions** — definition edges of slice variables at or after the target, listed rather
  than kept, since they matter only when a loop encloses both.

### Definition edges

The slice index turns statement IR entries into edges, each at the statement's step:

| Statement | Edge |
|---|---|
| `Let *x* be V` | defines *x*; uses = variables of V (including `New`/`Init` entry values) |
| `Set *x* to V` (plain variable target) | defines *x*; uses = variables of V |
| `Append/prepend/map-set *x*` (Mutate, no hops) | modifies *x*; uses = operand and subscript variables |
| `Set`/`Mutate` with hops or a non-variable root | Store; root variable tagged (no slice growth) |
| `Init` | no own edge; its entry values count as uses of the enclosing `Let`/`Set` |
| `Opaque` | Opaque; uses = variables in the statement span |

`Let *documentState* be a new document state with: origin → *navigable*'s …` links the initializer
values to *documentState* through the `New` / `Init` pair, so *navigable* is a use of *documentState*
and the enclosing `Let`.

### Named-argument labels

A `<var>` that is exactly one link (`<a href="…"><var>x</var></a>`) is a callee's parameter name being
bound by the caller, not a use of the caller's variable *x*. Such tokens are excluded from step
mentions. A `<var>` inside longer link text ("`[number of days in month *month*](…)`") is a real use.

This is why `DOM#concept-node-insert --involving suppressObservers` returns only step 8: the other
occurrences of *suppressObservers* in the algorithm are argument labels.

### Not followed

- **Opaque statement.** A matched step has a mutation verb whose target the IR did not parse. Its
  uses include a slice variable, so it may assign a variable the slice does not follow. Listed under
  `Not followed: … (opaque statement)`.
- **Loop binding.** A matched step has a clause-initial `For each *x* (of|in|from)` and *x* is not
  in the slice. Sub-project 3's `ForEach` edges will replace this marker. Listed under `Not followed:
  … binds *x* in a loop`.

### Rebound names

Variable identity is the name within one algorithm. When a name is `Let`-bound at more than one step
(two branches of an `If`/`Otherwise`), it is treated as one variable. The result lists such names
under `Rebound names treated as one variable: *x* (Let at 3, 9)` in the summary and under `rebound`
in JSON.

### Markers

Omitted runs are replaced by marker lines:

```
- [step 15.2 omitted: no use of *historyHandling*]
- [steps 16–20 omitted (7 steps): no use of *historyHandling*]
```

A single sibling uses `step P omitted`; several siblings use `steps P–Q omitted` (en dash). `(N
steps)` appears when the run hides more steps than its visible sibling count (substeps included).
The reason identifies the selector:

| Selector | Reason |
|---|---|
| `--involving a` | `no use of *a*` / `no use of *a* or *b*` / `no use of *a*, *b* or *c*` (seeds only) |
| `--feeding 24.9.1` | `does not feed step 24.9.1` |
| `--feeding 24.9.1:historyEntry` | `does not feed *historyEntry* in step 24.9.1` |
| `--steps 24.8` | `outside 24.8` / `outside 24.8, 3.2` |
| `--depth 1` | `below depth 1` |
| `--depth 1` under a slice selector | `below depth 1, 3 in slice` (hidden slice steps) |

Kept steps plus the step counts of all markers equal the algorithm's total step count.

### Summary block

In view mode, `## Content` becomes a titled heading followed by a summary. Example for
`HTML#navigate --involving historyHandling`:

```
## Content — steps involving *historyHandling*

Kept 14 of 67 steps: 9 involve the slice variables, 5 enclose them (15, 21, 22, 24, 24.9). 53 omitted
in 8 markers.
Slice variables: *historyHandling*; *continue* (Let at 22.4).
Semantics: may; this algorithm only, callees are not followed.
```

Optional lines (each only when non-empty): `Inherited (no variables, under a matched step): 4.1`;
`Stored into object state, not followed: 7.2 *parent*'s children (*node*)`;
`Not followed: 7.7 binds *inclusiveDescendant* in a loop; 11.1 binds *inclusiveDescendant* in a loop`;
`Rebound names treated as one variable: *x* (Let at 3, 9)`.
Backward adds `Inputs: *navigable*, *url*, …` and `Later definitions, not followed: …`.

### JSON `slice` object

With `--format json`, a top-level `slice` object is added to the response. Example for
`query DOM#concept-node-insert --involving parent`:

```json
"slice": {
  "algorithm": "DOM#concept-node-insert",
  "view": {"involving": ["parent"], "depth": null},
  "variables": [
    {"name": "parent", "basis": "seed"},
    {"name": "previousSibling", "basis": "let", "step": "6", "from": ["parent"]}
  ],
  "steps": [
    {"path": "5", "role": "context"}, {"path": "5.1", "role": "match"}, {"path": "5.2", "role": "match"},
    {"path": "6", "role": "match"}, {"path": "7", "role": "context"}, {"path": "7.1", "role": "match"},
    {"path": "7.2", "role": "match"}, {"path": "7.3", "role": "match"}, {"path": "7.4", "role": "match"},
    {"path": "7.5", "role": "match"}, {"path": "8", "role": "match"}, {"path": "9", "role": "match"}
  ],
  "omitted": [
    {"parent": null, "first": "1", "last": "4", "steps": 6, "reason": "no use of *parent*"},
    {"parent": "7", "first": "7.6", "last": "7.7", "steps": 9, "reason": "no use of *parent*"},
    {"parent": null, "first": "10", "last": "12", "steps": 4, "reason": "no use of *parent*"}
  ],
  "stores": [], "unfollowed": [], "rebound": [],
  "status": {
    "semantics": "may", "scope": "algorithm", "rendering": "aligned",
    "counts": {"steps": 31, "kept": 12, "matched": 10, "context": 2, "inherited": 0, "omitted": 19},
    "issues": []
  }
}
```

### May-semantics

Slicing uses may-semantics: a variable in the slice is in the slice at every step (flow-insensitive),
and only the algorithm itself is analyzed. Callees are not followed — an operation given a slice
variable may change that variable's object; every view says so in the summary. Sub-projects 4 and 5
will replace this blanket statement with specific store evidence and cross-algorithm binding.

### Full-content fallback

When the stored markdown's ordered-list numbering differs from the structural step paths, the content
is printed in full and the summary block says:

```
Rendering fell back to the full algorithm: the stored content's step numbering differs from the
structure (M items vs N steps). Steps in the view: 5, 5.1, 6, …
```

JSON `status.rendering` is `"unaligned"` and `issues` contains `"render_unaligned"`. The `slice`
object (kept paths, variables) is still computed and returned.

### PR previews and old indexes

PR snapshot sections have no slice index. A view request on a PR snapshot returns error code
`slice_unavailable`. The same code on a non-PR section means the local index was built before
slicing was added; re-parse to fix it:

```sh
webspec-index update -s SPEC
```

### Receiver-like parameters

A variable that appears in nearly every step (a "receiver" like *navigable* in HTML#navigate) produces
a forward slice that keeps most of the algorithm. Use `--depth 1` for a compact outline first, then
drill into the steps that matter with `--steps`. Reserve `--involving` for parameters with a tighter
footprint — *historyHandling* in HTML#navigate keeps 14 of 67 steps (35% of content), while *navigable*
keeps 53 of 67.

## Writing `state/` YAML

State declarations and verb rules supplement the structurally inferred object model. They
live under `state/` in the same semantics package as effects rules. The first path component
partitions files: `state/` files go to the state loader, every other file to the effects
loader. Query-time `--rules` packages may contain **only** `rules`; `types` and `fields` in
them are rejected.

A state YAML file has three optional sections: `types`, `fields`, and `rules`. Every
declaration carries `expect_text` (a Rust regex matched against the whitespace-normalized
plain text of the declaration block) and `reason`. When `expect_text` does not match, the
declaration is ignored and a `declaration_mismatch` issue is reported — this guards against
declarations silently misattributing fields after a spec edit.

### Complete example package

```yaml
schema: 1
package: my-project

types:
  - id: html-media-element-alias
    type: HTML#media-element
    alias_of: idl:HTMLMediaElement
    expect_text: 'HTMLMediaElement objects .* are simply known as media elements'
    reason: '"media element" is defined as HTMLMediaElement instances; names differ.'

fields:
  - id: html-option-selectedness
    field: HTML#concept-option-selectedness
    owner: [HTML#the-option-element]
    type: boolean
    initial: false
    expect_text: '(?i)selectedness'
    reason: 'Declared in element-level prose that the structural parser does not match.'

rules:
  - id: add-to-field-collection
    description: Unlinked "Add X to R's F" adds to a set- or list-valued field.
    match:
      text: '(?:^|, then |; )Add (?P<operand>.+?) to (?P<target>(?:the |this )?\S.*?)(?:\.|,|;|$)'
      exclude_text:
        - '(?i)\badd \S+ to \*\w+\*(?:\.|$)'
        - '(?:^|, then |; )Add to \*\w+\* '
    emit:
      kind: state.mutate
      params:
        op: append
        target: {capture: target, from: path}
        operand: {capture: operand, from: text}
```

The `field` key of a field declaration is the spec anchor (a dfn). `owner` is a list of
anchors. `type` is a primitive word (`boolean`, `string`, `number`, `integer`,
`byte_sequence`, `scalar_value_string`), `{nominal: SPEC#anchor}`, `{list: T}` (also
`ordered_set`, `ordered_map`, `map`), `{union: [T, …]}`, or `{opaque: text}`. `initial` is
a boolean, integer, `null`, a string, the keyword `empty` or `unset`, or `{opaque: text}`.
Because YAML quoting does not survive decoding, `empty` and `unset` are always keywords.

State rule emits support three kinds:

| kind | params | meaning |
|---|---|---|
| `state.write` | `field: anchor`, `op: string` | declared write of a named field |
| `state.init` | `field: anchor` | declared initialization |
| `state.mutate` | `op: string`, `target: {capture, from: path}`, optional `operand: {capture, from: text}` | custom mutation; target capture is parsed as a PATH |

For the full schema see [schemas/state/](../schemas/state/).
