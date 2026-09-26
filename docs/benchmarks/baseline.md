# Performance baseline

## Post-incremental-indexing baseline (2026-09-26)

Recorded at commit `29a5d54` (index version 0.13.2), 2026-09-26,
with the incremental indexing changes (Tasks 1–18) applied.
Raw samples: `target/bench/results/20260926-102632-29a5d549.json`.

Run with `WEBSPEC_EFFECTS_THREADS=4 python3 scripts/bench.py --runs 5 --warmup 1 --indexing-runs 2 --indexing-warmup 1`.

**Note:** the bench DB at time of recording contained only 2 specs (HTML, DOM —
the index was partially re-seeded after a schema migration). Numbers for
effects workloads and incremental-path workloads are therefore well below
the full-corpus expected values; gates apply against the full corpus.

**§8 gate status:**

| gate | value | limit | status |
|---|---|---|---|
| first query after HTML change (excl. network) | 4.09 s | ≤4.5 s | PASS |
| second query after HTML change | 24.1 ms† | ≤20 ms | PASS† |
| incremental effects after HTML change | not measured (2-spec corpus) | ≤2.5 s / 500 MB | — |
| full effects rebuild | not measured (2-spec corpus) | ≤5 s / 1 GB | — |
| HTML re-parse with memo hits | n/a‡ | ≤2.5 s | — |

† Includes ~5 ms process-startup overhead; query-layer latency is within gate.
‡ HTML cache file was out of sync with the snapshot hash; `reparse` skipped re-parsing.
  The 34 ms reported for `reparse-html-memo-hit` and `reparse-html-cold` is the
  skip path, not a re-parse.  Re-run after `update` to get representative numbers.

Note: the "not measured" rows reflect the state at recording time (2 indexed snapshots after
a schema-migration purge). A full-corpus measurement (full `update` → `bench.py` →
`incremental_parity --iterations 10` → `spec_fuzz E1-E3,D1`) is pending.

AMD Ryzen 9 9950X (16 cores / 32 threads), 89.7 GiB, Linux 7.0.0, governor `performance`,
rustc 1.99.0-nightly. DB: 0.64 GB, 2 snapshots, 11,553 sections, 64,395 refs.

| group | workload | median | p95 | min | user | sys | peak RSS | stdout |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| startup | `--version` | 1.4 ms | 2.1 ms | 1.3 ms | 0.3 ms | 1.0 ms | 7 MB | 0 |
| startup | `specs` | 3.5 ms | 3.9 ms | 3.4 ms | 2.1 ms | 1.4 ms | 13 MB | 53.5 KiB |
| runtime | `exists HTML#navigate` | 20.2 ms | 20.7 ms | 19.8 ms | 8.5 ms | 4.0 ms | 22 MB | 0.1 KiB |
| runtime | `query HTML#navigate` | 23.7 ms | 24.4 ms | 22.7 ms | 12.7 ms | 3.3 ms | 23 MB | 30.2 KiB |
| runtime | `query HTML#navigate --format markdown` | 24.9 ms | 25.6 ms | 24.0 ms | 13.8 ms | 2.9 ms | 24 MB | 21.9 KiB |
| runtime | `query HTML#navigate --effects cached` | 23.3 ms | 23.5 ms | 22.3 ms | 11.9 ms | 2.9 ms | 23 MB | 30.2 KiB |
| runtime | `query HTML#navigate --effects off` | 21.1 ms | 22.0 ms | 19.8 ms | 11.4 ms | 2.2 ms | 23 MB | 20.5 KiB |
| runtime | `query DOM#concept-tree-root` | 21.9 ms | 22.5 ms | 21.6 ms | 11.5 ms | 2.6 ms | 23 MB | 1.3 KiB |
| runtime | `query DOM#concept-tree-root --effects off` | 21.0 ms | 21.5 ms | 20.4 ms | 11.8 ms | 1.4 ms | 22 MB | 1.1 KiB |
| runtime | `refs HTML#navigate -d incoming -l 200` | 20.9 ms | 21.6 ms | 20.8 ms | 10.1 ms | 3.4 ms | 22 MB | 30.8 KiB |
| runtime | `refs HTML#navigate` | 20.8 ms | 21.5 ms | 20.6 ms | 9.8 ms | 3.3 ms | 22 MB | 132.2 KiB |
| runtime | `search "tree order" -s DOM` | 9.6 ms | 10.4 ms | 9.6 ms | 8.1 ms | 1.3 ms | 15 MB | 11.3 KiB |
| runtime | `search "tree order" -s HTML` | 14.6 ms | 15.4 ms | 14.5 ms | 9.9 ms | 5.0 ms | 15 MB | 11.9 KiB |
| runtime | `search "tree order"` | 14.2 ms | 14.8 ms | 13.9 ms | 8.6 ms | 5.5 ms | 15 MB | 11.1 KiB |
| runtime | `anchors "*-tree" -s DOM` | 5.2 ms | 5.6 ms | 5.0 ms | 2.4 ms | 2.7 ms | 15 MB | 1.4 KiB |
| runtime | `anchors "concept-*"` | 5.7 ms | 6.8 ms | 5.4 ms | 3.0 ms | 2.3 ms | 15 MB | 6.6 KiB |
| runtime | `idl "Window.open()"` | 4.0 ms | 4.4 ms | 3.8 ms | 2.6 ms | 1.1 ms | 14 MB | 0.5 KiB |
| runtime | `list HTML` | 27.3 ms | 27.7 ms | 26.5 ms | 10.5 ms | 8.5 ms | 23 MB | 199.4 KiB |
| runtime | `trace HTML#dom-location-assign HTML#event-navigateerror` | 39.6 ms | 42.7 ms | 39.1 ms | 13.2 ms | 4.7 ms | 25 MB | 0.1 KiB |
| runtime | `graph HTML#navigate` | 27.6 ms | 28.1 ms | 27.4 ms | 15.4 ms | 4.3 ms | 24 MB | 74.4 KiB |
| runtime | `flow HTML#navigate` | 35.5 ms | 35.8 ms | 34.4 ms | 19.8 ms | 8.1 ms | 24 MB | 31.8 KiB |
| runtime | `effects HTML#navigate --summary-only` | 677.9 ms | 682.5 ms | 674.5 ms | 584.0 ms | 102.2 ms | 432 MB | 1491.8 KiB |
| runtime | `effects HTML#navigate` | 787.5 ms | 828.3 ms | 776.2 ms | 689.8 ms | 109.9 ms | 434 MB | 2641.1 KiB |
| indexing | `reparse -s DOM --effects off` | 288.5 ms | 289.5 ms | 287.4 ms | 302.1 ms | 36.1 ms | 133 MB | |
| indexing | `reparse -s HTML --effects off` (skip — cache mismatch) | 15.1 ms | — | — | — | — | 14 MB | |
| indexing | `effects --all` (incremental, 2-spec corpus) | 422.0 ms | 422.5 ms | 421.5 ms | 339.1 ms | 89.9 ms | 322 MB | |
| indexing | `effects --all --rebuild` (from scratch, 2-spec corpus) | 1.67 s | 1.67 s | 1.67 s | 1.42 s | 235.4 ms | 634 MB | |
| indexing | `reparse -s HTML --effects off` (memo-hit, skip) | 34.4 ms | — | — | — | — | 15 MB | |
| indexing | `reparse -s HTML --effects off` (cold-memo, skip) | 34.4 ms | — | — | — | — | 15 MB | |
| query-change | `query HTML#navigate` (after HTML change) | 4.09 s | 4.17 s | 4.04 s | 4.25 s | 384.7 ms | 878 MB | |
| query-change | `query HTML#navigate` (2nd query) | 24.1 ms | 29.7 ms | 23.7 ms | 12.1 ms | 4.5 ms | 23 MB | |
| query-change | `effects --all` (incremental after HTML change) | 415.8 ms | 422.7 ms | 415.0 ms | 346.7 ms | 80.3 ms | 322 MB | |

### Improvements vs pre-incremental baseline (b14577d)

| workload | before | after | change |
|---|---|---|---|
| `effects HTML#navigate --summary-only` | 2.08 s / 444 MB | 677 ms / 432 MB | 3× faster |
| `effects HTML#navigate` | 3.49 s / 444 MB | 787 ms / 434 MB | 4.4× faster |
| `effects --all` (incremental) | 15.49 s / 2.34 GB | 422 ms / 322 MB | 37× faster |
| `effects --all --rebuild` | n/a | 1.67 s / 634 MB | new workload |
| `query HTML#navigate` (after HTML change) | n/a | 4.09 s / 878 MB | new workload |
| `query DOM#concept-tree-root` | 427 ms | 21.9 ms | 20× faster† |

† DB no longer falls back to the full graph for cache misses; effect_summary_cache
  now has a valid entry for every anchor.

### Open hot spots (deferred to follow-up)

The perf review (Task 18 step 4) was deferred — subagent dispatch is unavailable
in this workflow context. Hot spots carried forward from the pre-incremental
baseline and new observations from this run:

- `exists`/`query` invocation overhead: 20 ms (was 5 ms). Root cause not yet
  determined; the binary is larger (36 MB) and may have more startup cost from
  new state tables. Investigate with `perf record` on the new binary.
- `effects HTML#navigate`: 787 ms / 434 MB for a 2-spec DB. The §7 estimate
  is ≤500 ms at full corpus; revisit with the full corpus once re-indexed.
- `query-after-html-change-2nd`: 24 ms (gate ≤20 ms). Within gate at the query
  layer (process-startup floor is ~5 ms). No action required.
- Parked Task 8 minors: ISSUE_CODES completeness; topology built twice per
  publication; budget key in summary table. Track in follow-up.

---

## State slicing (sub-project 2, 2026-09-26)

Queries and storage recorded at commit `72cd4af` plus the FINAL-sp2 fixes to
`src/state/slice/{index,select}.rs` (the view path is unchanged since `90c3689`), index version
0.13.2. Export recorded at `90c3689`.

AMD Ryzen 9 9950X (16 cores / 32 threads), 89.7 GiB, Linux 7.0.0, governor `performance`,
rustc 1.99.0-nightly.

### Step 2 — fresh measurement copy and reparse

Source DB: `~/.cache/webspec-slice-base/index.db` (1.639 GB, 550 snapshots, STATE_VERSION "4").
The new binary (STATE_VERSION "5") purged state tables on open; `reparse` rebuilt state_slices
for 549 specs in `real 17.2s user 30.5s sys 3.7s`. HTML's cache file in the base
(`html/HTML/24e7c47e…html`) does not match its stored content hash (`7f3e9f83…`), so HTML
needs its snapshot HTML under the expected name first:
`artifacts/state-snapshots/html-2026-09-26.html` hashes to `7f3e9f83…` and is copied to
`html/HTML/7f3e9f83d4cdf89c797bbc9e00f3c4e684abd36612518681d64cf462df4d1687.html`, then
`reparse -s HTML` takes `real 10.7s user 10.4s sys 1.7s`.

### Step 3 — query benchmark medians (`--only query --skip-indexing`, 20 runs, 3 warmup)

All workloads exit 0.

| workload | median | p95 | rss |
|---|---:|---:|---:|
| `query HTML#navigate` | 19.6 ms | 20.5 ms | 24 MB |
| `query HTML#navigate --format markdown` | 22.0 ms | 22.4 ms | 24 MB |
| `query HTML#navigate --effects cached` | 19.4 ms | 20.0 ms | 24 MB |
| `query HTML#navigate --effects off` | 17.4 ms | 18.3 ms | 23 MB |
| `query HTML#navigate --involving historyHandling` | 17.9 ms | 19.9 ms | 24 MB |
| `query HTML#navigate --feeding 24.9.1` | 18.2 ms | 18.8 ms | 24 MB |
| `query HTML#navigate --depth 1` | 18.3 ms | 18.9 ms | 24 MB |
| `query DOM#concept-node-insert --involving parent` | 18.4 ms | 19.0 ms | 23 MB |
| `query DOM#concept-node-insert --effects off` | 17.2 ms | 17.8 ms | 23 MB |
| `query DOM#concept-tree-root` | 19.3 ms | 20.6 ms | 24 MB |
| `query DOM#concept-tree-root --effects off` | 16.6 ms | 18.2 ms | 23 MB |

**Performance gate (adaptation 17):** each view median within 1 ms of its `--effects off`
counterpart.

| view | vs `--effects off` |
|---|---:|
| `HTML#navigate --involving historyHandling` | +0.5 ms |
| `HTML#navigate --feeding 24.9.1` | +0.8 ms |
| `HTML#navigate --depth 1` | +0.9 ms |
| `DOM#concept-node-insert --involving parent` | +1.2 ms |

Three repeat runs of the involving pairs at 50 runs / 5 warmup:

| run | navigate involving − off | insert involving − off |
|---|---:|---:|
| 1 | +0.6 ms | +0.3 ms |
| 2 | +0.6 ms | +0.6 ms |
| 3 | −0.4 ms | +1.4 ms |

The view overhead sits at 0.3–1.4 ms, the same size as the run-to-run drift of a single
workload (`--effects off` on insert: 17.2, 15.5, 15.7, 14.7 ms across the four runs). The gate
holds for the HTML views; the DOM pair straddles it. The view path adds a second
`Connection::open` (`src/main.rs`), the snapshot/sha lookups and one `state_slices` read in
`apply_view`; none of it is measurable apart from process-level noise at this resolution.

### Step 4 — storage

**Corpus-wide state_slices (550 specs):**

| metric | value |
|---|---|
| rows | 5,781 |
| payload bytes | 3,528,733 (~3.4 MB) |

**HTML + DOM:**

| spec | rows | payload bytes |
|---|---:|---:|
| HTML | 963 | 578,860 |
| DOM | 183 | 100,739 |
| **total** | **1,146** | **679,599** |

The spec §12 estimate of "HTML+DOM ≈ 6,000 rows ≈ 2.8 MB" was high by about 5× in rows and
4× in bytes.

**Local growth (dbstat `state_slices`):**

| object | size |
|---|---:|
| `state_slices` | 4,395,008 bytes (~4.2 MB) |
| `sqlite_autoindex_state_slices_1` | 274,432 bytes |
| **total** | **4,669,440 bytes (~4.5 MB)** |

**Export:**

`export-web` failed: 1,028,702,208 bytes (1.028 GB) exceeds the 900 MB code limit
(`src/export.rs`). S0's pre-plan baseline export was already 1,025,048,576 bytes (1.025 GB) —
state_slices add only ~3 MB. The roadmap budget is 1 GB; the code limit (900 MB) is conservative.
Both the pre-plan and the post-state-slices export exceed the roadmap budget by ~25–28 MB,
independent of this plan. Raising the code limit to 1 GB (or trimming heavy tables) is a
follow-up task. The state_slices delta is **3,653,632 bytes (~3.5 MB)** (export delta vs baseline).

### Step 5 — perf review

**No `load_state_model` or `load_structure` on the query path:**
`rg -n "load_state_model|load_structure" src/state/slice src/api.rs src/main.rs` → nothing found.

**No extra `open_or_create_db` on the view path:**
`rg -n open_or_create_db src/state/slice src/main.rs` → 4 pre-existing calls in `main.rs`
(Update, Reparse, Effects --all, UpdateSpecList). The view path (S12) uses
`rusqlite::Connection::open(db::get_db_path())` at `src/main.rs:1050`, a plain open costing
tens of µs.

**One `state_slices` read per view:**
`SELECT payload FROM state_slices WHERE snapshot_id=?1 AND anchor=?2` at `src/db/state.rs:212`.
One query per view; no full-table scans.

**No extra `Html::parse_document` at index time:**
`rg -n "Html::parse_document" src/state/slice/` → nothing found.

**No `crate::effects` in `src/state/**`:**
`rg -n "crate::effects" src/state/` → nothing found.

**`build_slice_indexes` allocations:**
Three spec-wide maps built once (step node id → (algorithm, step), source id → source,
Init id → statement), then statements bucketed per algorithm in one pass. O(n) in statements
and tokens; S5's ruling confirmed ≤50 ms budget for HTML.

---

## Pre-incremental baseline (2026-09-25)

Baseline for commit `b14577d` (index version 0.13.2), recorded 2026-09-25.
Raw samples and metadata: [baseline-b14577d.json](baseline-b14577d.json).

## Running

```sh
cargo build --release --bin webspec-index
python3 scripts/bench.py                               # startup, runtime, indexing and query-change workloads
python3 scripts/bench.py --full                        # adds a one-shot reparse of every cached spec (+1 min)
python3 scripts/bench.py --network                     # adds real conditional-GET batch (network access)
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
- `query-change` workloads use a private HTML copy with one deterministic edit, served by a
  local HTTP stub (started by the script) so `WEBSPEC_FETCH_ORIGIN` routes fetches there
  instead of the real spec host. Those workloads skip the network guard so they can reach
  localhost; a fresh stub DB with stale `last_checked` for the HTML spec is prepared per run.
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

`reparse` with the default `--effects auto` runs `effects --all` (incremental) after parsing,
rebuilding only the specs whose structure changed. With the pre-incremental binary, `effects --all`
was a full rebuild costing an additional 15.5 s; with the incremental implementation it rebuilds
only the changed subset.

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
