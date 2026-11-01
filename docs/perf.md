# Performance

Execution speed is the metric that matters here: transpiled Brainfuck
programs do "manual" memory management, so running them is where time
goes. Transpile time is measured too, for context.

## Method

- Harness: `rs/src/bin/c2bf-bench.rs`, run from the repo root after
  `cargo build --release --manifest-path rs/Cargo.toml`:
  `./rs/target/release/c2bf-bench`.
- In-process timing (excludes process startup). Each case runs at
  least 3 iterations and until ~2s total elapsed (cap 100 iterations);
  the mean and min over those iterations are reported.
- Corpus: every `tests/compile/*.c` with its `.in`, plus synthetic
  runner microbenchmarks (loop nesting, tape growth, cell bumps,
  skip-scans, I/O echo).
- CI runners are shared and noisy: compare numbers within one run,
  not across runs, and re-run before drawing conclusions.

## Results

<!-- BEGIN RESULTS -->
In-process timing on linux-x86_64 (`c2bf-bench`, adaptive iterations: at least 3, until ~2s total, cap 100). Times exclude process startup.

| program | BF bytes | compile mean (ms) | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|
| addition | 56339 | 0.080 | 6.262 | 6.197 | 100 | 2 |
| addsub | 520649 | 1.149 | 99.500 | 98.925 | 21 | 28 |
| assign | 1787399 | 2.475 | 16.786 | 16.511 | 100 | 32 |
| branch | 10781 | 0.038 | 0.132 | 0.129 | 100 | 10 |
| break | 632807 | 0.971 | 13.524 | 13.434 | 100 | 24 |
| cmp | 938027 | 1.308 | 4.057 | 4.030 | 100 | 32 |
| complex | 377184 | 0.545 | 39.184 | 38.922 | 52 | 210 |
| continue | 855741 | 1.312 | 29.996 | 29.759 | 67 | 30 |
| div | 585138 | 0.817 | 3.521 | 3.500 | 100 | 18 |
| dowhile | 515169 | 0.744 | 3.734 | 3.467 | 100 | 17 |
| fordecl | 280864 | 0.402 | 3.563 | 3.530 | 100 | 10 |
| hello_world | 597 | 0.005 | 0.008 | 0.008 | 100 | 14 |
| io | 123335 | 0.178 | 8.127 | 8.072 | 100 | 8 |
| logic | 812538 | 1.138 | 3.044 | 3.020 | 100 | 28 |
| loop | 236198 | 0.337 | 3.262 | 3.240 | 100 | 8 |
| mod | 617886 | 0.864 | 3.491 | 3.462 | 100 | 18 |
| mul | 462570 | 0.649 | 67.309 | 67.023 | 30 | 24 |
| printnum | 1359034 | 1.893 | 355.648 | 354.902 | 6 | 85 |
| switch | 105679 | 0.212 | 1.005 | 0.982 | 100 | 21 |
| ternary | 726950 | 1.059 | 9.165 | 9.089 | 100 | 24 |

Runner microbenchmarks:

| name | BF bytes | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|
| nested-loops | 43 | 0.009 | 0.006 | 100 | 1 |
| tape-walk | 200002 | 0.816 | 0.763 | 100 | 1 |
| cell-bump | 100001 | 0.629 | 0.580 | 100 | 1 |
| skip-scan | 100002 | 0.215 | 0.209 | 100 | 0 |
| io-echo | 5 | 0.070 | 0.067 | 100 | 4096 |
<!-- END RESULTS -->

## Freshness mandate

Refresh this file after any change to the runner, the code emitter,
or the harness itself: regenerate with
`./rs/target/release/c2bf-bench --write docs/perf.md` and commit the
result (or run the `perf-snapshot` workflow from the Actions tab and
commit its artifact). Perf-affecting changes without a refreshed
results section are incomplete.

## Optimization log

- Baseline (no optimizations yet; see the results above).
- Runner jump table + buffered I/O (`rs/src/runner.rs`): `[` skips
  and `]` jumps are O(1) via a precomputed table; `,`/`.` go through
  8 KiB buffers. Shared-runner noise dominates corpus means (compile
  times, which this change cannot affect, also moved ~2x between
  runs); heavy-loop programs improved on minima (addsub run-min
  79.4 -> 46.6 ms, mul 65.4 -> 38.4 ms), consistent with cheaper hot
  loops. Buffered I/O shows no change in-process (the harness uses
  in-memory input); its win is at real pipes/files.
- Peephole post-pass measured and rejected: across all golden
  outputs, zero occurrences of `<>`, `><`, `+-`, `-+`, `[]`, and
  doubled clears — the emitter already tracks the pointer exactly,
  so a cancellation pass would change nothing.
- Loop-based constant construction (`set_value`): large constants
  build as `a * b + r` in a loop over a scratch cell, taken only
  when smaller in emitted bytes and within 25% of direct in
  estimated steps. All 101 runs of 60 or more `+`s in the golden
  corpus are gone (longest now 48); `hello_world` halves (1209 ->
  597 BF bytes). Corpus totals barely move — arithmetic-helper
  expansion dominates — so the remaining size lever is helper
  code, not constants. Run means moved both ways between runs
  and are not comparable across runs.
