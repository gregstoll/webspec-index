# Effects performance fixture

`corpus.json` uses the checked-in acceptance HTML, so the effects benchmark runs
without network access. CI runs it as an architectural regression check: after
preparing effects, the harness corrupts graph topology in its temporary database
and checks that a fresh-process query still returns the prepared result. CI
uploads the JSON report as `effects-benchmark`. There are no wall-clock failure
thresholds.

To measure an optimized build locally:

```sh
cargo run --release --example benchmark_effects -- tests/fixtures/effects/performance/corpus.json
```

The command accepts another manifest with the same format and local HTML files
for a larger corpus. To inspect preparation scaling, run the same manifest with
`WEBSPEC_EFFECTS_THREADS=1`, `4`, and `8`. Compare reports from the same machine,
build profile, and corpus. `warm_query_median_ms` measures the old per-process
analysis cache before preparation; `prepared_query_median_ms` measures the
materialized lookup after recomputation. `fresh_process_ms` includes process
startup, while `fresh_process_query_ms` measures the query inside the child
process.
