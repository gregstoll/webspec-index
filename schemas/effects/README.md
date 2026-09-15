# Effects contract v1

These schemas define the stable wire boundary shared by the Rust library, CLI,
Python bindings, and editor clients. Effects always have **may** semantics: a
reported effect is a supported possibility, and a graph witness does not prove
that a particular runtime path takes it. An empty result means only that this
analysis detected no effects. Consumers must inspect `state`, `coverage`, and
`issues` before interpreting an empty list.

Unknown effect parameters are present as `null`; omitted fields are optional
metadata. `execution` describes the effect relative to the selected subject's
synchronous flow. `inline` means the same flow, `separate` means a scheduling
boundary was crossed, and `unknown` means timing could not be resolved. Values
are sorted and deduplicated in that order.

`catalog.schema.json` describes one YAML catalog file after YAML 1.2 decoding.
The Rust loader additionally validates cross-file and cross-package properties:
IDs and effect kinds are unique in their required scopes, referenced kinds and
parameters exist, regexes compile with Rust `regex`, capture groups exist, and
constant values have their declared types. It rejects duplicate mapping keys,
aliases, anchors, merge keys, custom tags, and multiple documents.

Canonical digests use compact UTF-8 JSON with object keys recursively sorted
and arrays kept in their contract-defined order. Effect handles start with the
first 16 lowercase hexadecimal digest characters and extend on collisions
within a run. Handles identify a `(kind, params)` group within a subject query;
they do not identify a source occurrence or remain valid across analysis runs.
