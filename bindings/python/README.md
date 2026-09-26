# webspec-index (Python bindings)

Python bindings for [`webspec-index`](https://github.com/jnjaeschke/webspec-index) —
query WHATWG, W3C, TC39, and IETF web specifications: sections, cross-references,
WebIDL definitions, reference graphs, full-text search, WHATWG PR previews, and
source-file step-comment analysis.

The bindings wrap the same Rust core as the `webspec-index` CLI. All functions are
**synchronous**; spec data is fetched and cached locally on first use.

## Install

```bash
pip install webspec-index
```

## Usage

```python
import webspec_index as wsi

# Query a section (SPEC#anchor or a full URL)
section = wsi.query("HTML#navigate")
print(section.title, section.section_type)
for ref in section.outgoing_refs:
    print(ref.spec, ref.anchor)

# Full-text search within a spec
results = wsi.search("tree order", spec="DOM", limit=5)
for hit in results.results:
    print(hit.anchor, hit.snippet)

# Existence check
print(wsi.exists("HTML#navigate").exists)

# Anchors by glob, headings, cross-references, WebIDL, graph
wsi.anchors("*-tree", spec="DOM")
wsi.list_headings("DOM")
wsi.refs("Window.navigation", direction="incoming")
wsi.idl("Window.open()")
wsi.graph("HTML#navigate", max_depth=2)

# WHATWG PR previews
wsi.query("HTML#navigate", pr=12345)
wsi.pr_diff("HTML", pr=12345)

# Source analysis
for file in wsi.analyze("src/", recursive=True):
    print(file.file, len(file.scopes))

# Every result object is typed and also offers .to_dict() / .to_json()
section.to_dict()
```

Field state (who writes a spec field):

```python
result = wsi.state("HTML#is-initial-about:blank")
print(result["field"]["name"], result["status"]["coverage"])
for site in result["writes"]:
    print(site["subject"], site["step_path"], site["text"])

coverage = wsi.state_coverage("HTML")
print(coverage["counters"]["owner_resolved"], "/", coverage["counters"]["written_fields"])
```

`state(selector, include_inits=True, unclassified=False, limit=None)` accepts the same
selectors as the CLI: `SPEC#anchor`, `TYPE`, `"TYPE.FIELD"`, `"TYPE.GLOB"`, or `"SPEC#GLOB"`.
`state_coverage(spec)` returns the stored coverage counters for one spec.

Possible specification effects use dedicated, versioned request dictionaries;
the existing `query()` result remains unchanged:

```python
request: wsi.EffectsRequest = {
    "schema_version": 1,
    "subject": {"spec": "WEBIDL", "anchor": "wait-for-all"},
}
summary = wsi.get_effect_summary(request)
for effect in summary["effects"]:
    print(effect["kind"], effect["execution"])

details = wsi.explain_effects({**request, "explanation": {"limit": 1}})
for explanation in details["explanations"]:
    print(explanation["effect_id"], explanation["witnesses"])
```

`get_effect_summary()` and `explain_effects()` are synchronous. Results are
JSON-compatible dictionaries following effects schema version 1. Use
`options.rule_paths` for additional local rule packages and `options.environment`
to select host implementation mappings.

`recompute_effects()` triggers an incremental effects rebuild for the indexed
corpus (equivalent to `webspec-index effects --all`). It returns when the
publication is up to date. When a spec's HTML was updated and effects have not
been rebuilt yet, `get_effect_summary()` raises `WebspecError` with a message
starting `snapshot_changed:`; call `recompute_effects()` once to rebuild, then
retry the query. From the CLI, `query` output carries
`"effects_status": {"state": "unavailable", "semantics": "may", "issues": ["snapshot_changed"], "omitted": 0}`
in that state.

Errors are raised as `webspec_index.WebspecError`.

## Development

This project uses [`uv`](https://docs.astral.sh/uv/) and
[`maturin`](https://www.maturin.rs/).

```bash
# from bindings/python/
uv sync --extra test     # build the extension + install test deps
uv run pytest            # run the test suite
```
