# Performance baseline

Baseline for commit `b14577d` (index version 0.13.2), recorded 2026-09-25.
Raw samples and metadata: [baseline-b14577d.json](baseline-b14577d.json).

## Running

```sh
cargo build --release --bin webspec-index
python3 scripts/bench.py            # startup, runtime and indexing workloads (~4 min)
python3 scripts/bench.py --full     # adds a one-shot reparse of every cached spec (+1 min)
python3 scripts/bench.py --skip-indexing --only query --runs 10
```

Results go to `target/bench/results/<timestamp>-<sha>.{json,md}` (`--out-dir` to change).
To compare a change, run the script before and after on the same machine and diff the
medians; for a binary built outside this working tree, pass `--binary PATH --commit SHA`.
This baseline was taken with a binary built from a clean `git archive b14577d` export.

The script needs only Python 3 and the release binary. It never writes to
`~/.webspec-index/index.db`:

- The index is copied with SQLite's backup API (read-only on the source) to
  `target/bench/db/index.db`; the binary is pointed at the copy with `SPEC_INDEX_TEST_DB`.
  `target/bench/db/html` is a symlink to `~/.webspec-index/html`, which `reparse` only reads.
- `update_checks.last_checked` is set to now in the copy, so lookups stay inside the 24h
  freshness window instead of re-fetching.
- `MOZTOOLS_UPDATE_CHECK=0` disables the crates.io version check.
- Every process runs under `unshare -rn` (no network), so a workload that tries the
  network fails visibly instead of timing a download.
- The script aborts if the source index version differs from the binary version: opening
  such a DB purges it.

Each workload is a fresh process (the CLI is invoked once per query). Runtime workloads get
3 warmup + 20 measured runs (the two `effects` subjects 1 + 8), indexing workloads 1 + 3.
Wall time is spawn-to-exit; user/sys come from `wait4`, peak RSS from GNU time.
`unshare` and GNU time add roughly 0.5 ms to every run, so compare numbers against the
`version` floor rather than against zero. Indexing workloads rewrite the copy, so they run
last, and each invocation starts from a fresh copy (`--reuse-db` skips the copy).

## Baseline

AMD Ryzen 9 9950X (16 cores / 32 threads), 89.7 GiB, Linux 7.0.0, governor `performance`,
rustc 1.99.0-nightly. Index: 1.21 GB, 550 snapshots, 98,049 sections, 523,454 refs.

| group | workload | median | p95 | min | user | sys | peak RSS | stdout |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| startup | `--version` | 2.6 ms | 2.9 ms | 2.2 ms | 1.4 ms | 3.9 ms | 8 MB | 0 |
| startup | `specs` | 5.1 ms | 5.9 ms | 4.8 ms | 3.0 ms | 4.5 ms | 13 MB | 54 KiB |
| runtime | `exists HTML#navigate` | 5.1 ms | 5.9 ms | 4.9 ms | 3.7 ms | 5.0 ms | 13 MB | 0.1 KiB |
| runtime | `query HTML#navigate` | 15.2 ms | 16.2 ms | 14.6 ms | 9.3 ms | 9.5 ms | 19 MB | 31 KiB |
| runtime | `query HTML#navigate --format markdown` | 14.6 ms | 15.1 ms | 14.3 ms | 10.7 ms | 7.3 ms | 19 MB | 22 KiB |
| runtime | `query HTML#navigate --effects cached` | 14.6 ms | 15.2 ms | 14.0 ms | 9.4 ms | 9.3 ms | 19 MB | 31 KiB |
| runtime | `query HTML#navigate --effects off` | 6.8 ms | 7.2 ms | 6.4 ms | 4.8 ms | 5.4 ms | 15 MB | 21 KiB |
| runtime | `query DOM#concept-tree-root` | 427 ms | 437 ms | 416 ms | 338 ms | 92 ms | 281 MB | 1.3 KiB |
| runtime | `query DOM#concept-tree-root --effects off` | 5.6 ms | 5.9 ms | 5.3 ms | 3.7 ms | 5.0 ms | 15 MB | 1.1 KiB |
| runtime | `refs HTML#navigate -d incoming -l 200` | 5.5 ms | 6.1 ms | 5.3 ms | 3.3 ms | 4.8 ms | 14 MB | 51 KiB |
| runtime | `refs HTML#navigate` | 5.8 ms | 6.3 ms | 5.5 ms | 3.7 ms | 5.4 ms | 14 MB | 153 KiB |
| runtime | `search "tree order" -s DOM` | 80.1 ms | 82.2 ms | 79.6 ms | 78.0 ms | 6.1 ms | 16 MB | 12 KiB |
| runtime | `search "tree order" -s HTML` | 388 ms | 398 ms | 382 ms | 382 ms | 9.2 ms | 18 MB | 12 KiB |
| runtime | `search "tree order"` | 7.0 ms | 7.6 ms | 6.8 ms | 4.2 ms | 5.5 ms | 15 MB | 11 KiB |
| runtime | `anchors "*-tree" -s DOM` | 5.2 ms | 5.5 ms | 4.9 ms | 3.3 ms | 4.5 ms | 14 MB | 1.4 KiB |
| runtime | `anchors "concept-*"` | 5.1 ms | 5.6 ms | 4.8 ms | 2.8 ms | 5.6 ms | 14 MB | 6.6 KiB |
| runtime | `idl "Window.open()"` | 17.7 ms | 18.4 ms | 17.3 ms | 14.4 ms | 6.5 ms | 15 MB | 0.5 KiB |
| runtime | `list HTML` | 49.2 ms | 50.2 ms | 46.0 ms | 12.1 ms | 40.3 ms | 63 MB | 199 KiB |
| runtime | `trace HTML#dom-location-assign HTML#event-navigateerror` | 6.1 ms | 6.8 ms | 5.9 ms | 4.3 ms | 5.3 ms | 15 MB | 0.1 KiB |
| runtime | `graph HTML#navigate` | 10.5 ms | 11.0 ms | 10.2 ms | 8.2 ms | 5.2 ms | 17 MB | 78 KiB |
| runtime | `flow HTML#navigate` | 25.7 ms | 26.5 ms | 24.2 ms | 16.4 ms | 12.2 ms | 17 MB | 37 KiB |
| runtime | `effects HTML#navigate --summary-only` | 2.08 s | 2.31 s | 1.97 s | 1.93 s | 149 ms | 444 MB | 2.7 MiB |
| runtime | `effects HTML#navigate` | 3.49 s | 3.72 s | 3.25 s | 3.34 s | 161 ms | 444 MB | 3.9 MiB |
| indexing | `reparse -s DOM --effects off` | 542 ms | 603 ms | 531 ms | 430 ms | 83 ms | 87 MB | |
| indexing | `reparse -s HTML --effects off` | 9.64 s | 9.70 s | 9.62 s | 8.91 s | 538 ms | 434 MB | |
| indexing | `effects --all` (graph build run by `update`/`reparse`) | 15.49 s | 15.96 s | 15.30 s | 14.82 s | 1.45 s | 2.34 GB | |
| indexing | `reparse --effects off` (all 550 indexed specs, 1 run) | 49.2 s | | | 41.1 s | 4.0 s | 458 MB | |

`reparse` with the default `--effects auto` is the `reparse --effects off` time plus the
`effects --all` time: the graph build is a full rebuild over every spec whatever was reparsed.

## Where the time goes

Profiled with `perf record --call-graph fp` on a release build with
`-C force-frame-pointers=yes` and line tables; SQL plans checked against the bundled
SQLite 3.51.3 (rusqlite 0.39), which is not the system SQLite.

### Fixed cost per invocation (about 5 ms of every lookup)

`--version` costs 2.6 ms and `specs`/`exists` 5.1 ms; the difference is `open_or_create_db`.

- `spec_list::fetch_and_seed` runs on every open: 576 `SELECT id, base_url FROM specs
  WHERE name = ?` statements, each prepared from scratch and each its own autocommit read
  transaction. `strace -c` of one `exists`: 2,556 `fcntl` (lock/unlock),
  1,272 failed `newfstatat` (hot-journal probe per transaction), 680 `pread64`, 644 `fstat`.
  In the `query HTML#navigate` profile, `seed_spec` is 24% of samples.
- `#[tokio::main]` starts the multi-threaded runtime: 32 worker threads (32 `clone3`) for a
  process that does one synchronous SQLite query. `TOKIO_WORKER_THREADS=1` cuts
  `--version` from 1.6 to 0.7 ms and `exists` from 4.7 to 3.8 ms (unwrapped, 30 runs).
- The effects rule catalog (`effects::default_catalog`) is parsed and digested from the
  bundled sources in every process that shows effects: about 18% of `query HTML#navigate`.

### Stored effect preview reads the 20 MB topology blob to reach one column

`load_graph_meta` selects `opaque_anchor_issue_id`, which is declared after the 20.5 MB
`topology` BLOB in `effect_graph`. SQLite walks the blob's ~5,000 overflow pages to reach
it: 2.0 ms per call against 0.01 ms without that column. `getOverflowPage` is 17% of
`query HTML#navigate`. This, the catalog, and the seed loop explain most of the 8 ms gap
between `query` with and without effects.

### `query` on anchors without a stored summary loads the whole effects graph (427 ms)

`effect_summary_cache` holds a `"null"` miss marker for 6,731 of its 65,778 keys: graph
anchors that `summary_records` does not emit. `get_cached_effect_preview_on` treats
`"null"` as a cache miss, so `query` falls back to `get_effect_summary`: it inflates the
topology (20.5 MB deflate to 59.8 MB JSON), deserializes it into `BTreeMap<String, _>`s,
synthesises a node and runs `analyze_graph`. `load_graph` is 75% of the profile
(inflate 17%, SipHash 7%, malloc/free and `String::clone` most of the rest).
`DOM#concept-tree-root` (a definition) is one of those anchors; with `--effects off` the
same query takes 5.6 ms.

### `effects SUBJECT` (2.1 s / 3.5 s) is dominated by a quadratic lookup

`AnalysisArtifact::summary` (`src/effects/engine.rs`) materialises every summary record
(`summary_records`, 8%), then runs `find` over them, and for each record runs another linear
`find` over all nodes to compare subjects by string: 52% of the profile
(`IntoIter::try_fold` 42% + `memcmp` 11%). The rest: `analyze_graph` 17.6% (the analysis
artifact is recomputed per process, never stored), `load_graph` 15%. `--summary-only`
still writes 2.7 MiB of JSON.

### Missing planner statistics pick bad plans (search 80–388 ms, flow 26 ms)

The DB has no `sqlite_stat1`: `ANALYZE` has never run. Without it, SQLite 3.51.3 plans:

- `search -s SPEC`: `SEARCH sections USING INDEX idx_sections_parent (snapshot_id=?)`
  then `SCAN sections_fts VIRTUAL TABLE INDEX 32:=M3`, i.e. one FTS5 MATCH + bm25 per
  section of the spec (78 ms for DOM, 376 ms for HTML in-process), instead of one FTS scan.
  The unfiltered search uses the FTS-first plan and takes 7 ms end to end.
- `flow` (`get_outgoing_algorithm_calls`): `SEARCH r USING INDEX idx_refs_incoming
  (snapshot_id=?)`, scanning every ref of the snapshot, instead of `idx_refs_outgoing
  (snapshot_id, from_anchor)`: 20–50 ms against 0.2 ms.

On an analysed copy (`ANALYZE` takes 0.1 s), both switch to the good plans:
`search -s HTML` 388 → 6.9 ms and `flow HTML#navigate` 25.7 → 6.3 ms end to end.
`FROM sections_fts CROSS JOIN sections` forces the good search plan without statistics
(1.5 ms in-process).

### `list HTML` (49 ms, 40 ms of it sys)

`db::queries::list_headings` selects `content_text` for every heading, which
`heading_to_list_entry` never reads, and sorts the rows by `rowid` in a temp B-tree that
spills to temp files (`unixWrite` 23%, `vdbeMergeEngineStep` 20%). Dropping the column:
34 ms → 6 ms in-process.

### `idl NAME` (18 ms)

The name lookup in `query_idl_from_conn` filters with `LOWER(col) = ?` / `LOWER(col) LIKE ?`
over five columns, which no index can serve: a full scan of `idl_defs` calling `lowerFunc`
per row, with SQLite's malloc mutex (`pthread_mutex_lock` 35%) around each call.
`ANALYZE` does not help this one.

### `reparse -s HTML` (9.6 s, single-threaded)

- 82% parsing, 18% writing (`insert_sections_bulk` 8.4%, FTS5 flush 6%, delete of the old
  snapshot 3.7%).
- `steps::document_order_position` walks the whole document from the root for every
  algorithm candidate: quadratic, about 21% (the `ego_tree::Traverse::next` self time).
- `steps::dom_path` rebuilds the path string for every step, counting preceding siblings at
  each level: 17%.
- The HTML is parsed into a DOM four times (`parse_spec`, `extract_references`,
  `extract_idl_definitions`, `extract_step_structure`): `Html::parse_document` is 8% in total.
- Markdown conversion serialises each element back to HTML and re-parses it with htmd
  (`htmd::convert` 12.8%, of which `html_to_tree` 8%).
- `reparse` over all specs is a sequential loop (41 s user of 49 s wall on a 32-thread
  machine); HTML alone is a fifth of it.

### `effects --all` (15.5 s, 2.3 GB)

`build_and_store_graph` 67%: `build_graph` 25%; `local::input_key` 19%, which serialises
each spec's entire step structure into a `serde_json::Value`, canonicalises and SHA-256s it
on every build; hex digests built with one `format!` per byte (`sha256_bytes`) 9%;
`digest_serializable::<GraphIssue>` 10%. `store_compact_summaries` 12%, `store_graph`
11.6% (deflate 7%), `analyze_graph` 9%. Wall ≈ user: almost all of it single-threaded.

## Database size

`dbstat` of the 1.21 GB index (4 KiB pages, rollback journal, no free pages):

| object | size | share |
|---|---:|---:|
| `sections` (of which `content_text` 346 MB) | 382 MB | 31.6% |
| `effect_structures` (uncompressed `structure_json`) | 204 MB | 16.9% |
| `sections_fts_data` (external-content FTS5 index) | 203 MB | 16.8% |
| `effect_sites` | 98 MB | 8.1% |
| `refs` | 81 MB | 6.7% |
| `effect_summary_cache` | 48 MB | 4.0% |
| `idx_refs_incoming` | 26 MB | 2.2% |
| `idx_refs_to` | 25 MB | 2.1% |
| `effect_anchors` autoindex | 24 MB | 1.9% |
| `effect_graph` (one 20.5 MB topology row) | 21 MB | 1.7% |
| `effect_anchors` | 20 MB | 1.7% |
| `idx_refs_outgoing` | 20 MB | 1.7% |
| everything else | 56 MB | 4.6% |

`idx_refs_incoming (snapshot_id, to_spec, to_anchor)` and `idx_refs_to (to_spec, to_anchor)`
overlap. `effect_structures` is stored as plain JSON while the other effect payloads are
deflated.
