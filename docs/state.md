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

## Coverage view

```sh
webspec-index state --coverage HTML --format markdown
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

Content-attribute reflection is tracked separately. When a field is reflected by an IDL
attribute, the type view notes the IDL attribute and content-attribute anchor. Query the
field anchor directly for its writes; the reflection relationship confirms that the IDL
setter also sets the content attribute.

## Set members

`state SPEC#anchor` where the anchor is a set concept (e.g. `HTML#sandboxing-flag-set`)
shows the member view: each member anchor and name, with the count of sites that add or
remove it. Individual flag anchors like `HTML#sandboxed-scripts-browsing-context-flag` also
resolve to the member view.

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
