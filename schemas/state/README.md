# State catalog contract v1

State declarations and rules fill gaps in the structurally inferred object
model and field writers. Like effects, state answers have **may** semantics: a
declaration or rule adds or overrides what the analysis reports, and it never
deletes IR output.

State YAML lives in the same semantics package as the effects rules, under
`state/`. The package is partitioned by first path component: files under
`state/` go to the state loader, every other file to the effects loader. All
state files of a package share one `schema: 1` and one `package` identifier.
Public IDs are `{package}/{id}` and must be unique across every loaded package.

`catalog.schema.json` describes one YAML state file after YAML 1.2 decoding.
It reuses the effects schema's `id`, `anchor`, `match` and `capture`
definitions, so both layers have one rule model. A file has three optional
sections:

- `types`: type declarations. `type` is the type's anchor (a dfn or a heading).
  `name` binds `data-dfn-for` values to it, `kind` sets its kind, `alias_of`
  makes it an alias of another type, and `implemented_by` adds supertype edges
  from the listed types. Type keys are `idl:Name` or `SPEC#anchor`.
- `fields`: field declarations. `owner`, `type` and `initial` each override
  only their own attribute. `type` is a primitive word (`boolean`, `string`,
  `number`, `integer`, `byte_sequence`, `scalar_value_string`),
  `{nominal: SPEC#anchor}`, `{list: T}` (also `ordered_set`, `ordered_map`,
  `map`), `{union: [T, …]}` whose members may be `null`, or `{opaque: text}`.
  `initial` is a boolean, integer, `null` or string literal, the keyword
  `empty` or `unset`, or `{opaque: text}`. Because YAML quoting does not survive
  decoding, the strings `empty` and `unset` are always keywords.
- `rules`: text rules over state statement sources, with the effects rule
  model's `id`, `description` and `match`, and one of the emits below.

Every type and field declaration carries `expect_text` and `reason`.
`expect_text` is a Rust regex matched against the whitespace-normalized plain
text of a block (no backticks or asterisks): the declaration block of the
field's dfn, or of the type anchor's dfn. For a heading type anchor it must
match one of the blocks declaring fields owned through the declaration's
`name`. When it does not match, the declaration is ignored and
`declaration_mismatch` is reported.

| kind | params | meaning |
|---|---|---|
| `state.write` | `field: anchor`, `op: string` | declared write of a named field |
| `state.init` | `field: anchor` | declared initialization |
| `state.mutate` | `op: string`, `target: {capture, from: path}`, optional `operand: {capture, from: text}` | custom mutation verb; the target capture is parsed as a `PATH` |

`from: path` is the one capture source state adds to `literal|text|anchor`.
Every capture must name a group of `match.text`. The Rust loader rejects any
other kind or parameter, naming the rule.

The state digest is `sha256:` followed by the SHA-256 of the package's sorted
`path \0 content \0` pairs for its `state/` files. A package without state files
has no digest, and the stored representation version is then the bare state
version.

Query-time `--rules` packages may carry only `rules`; `types` and `fields` in
them are rejected.
