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
| addition | 56339 | 0.079 | 5.932 | 5.872 | 100 | 2 |
| addsub | 521520 | 1.150 | 95.332 | 92.730 | 21 | 28 |
| assign | 1787399 | 2.498 | 16.327 | 16.005 | 100 | 32 |
| branch | 11037 | 0.042 | 0.125 | 0.122 | 100 | 10 |
| break | 632935 | 0.989 | 18.092 | 17.888 | 100 | 24 |
| cmp | 938027 | 1.311 | 4.407 | 4.221 | 100 | 32 |
| complex | 376767 | 0.540 | 43.725 | 43.407 | 46 | 210 |
| continue | 855743 | 1.311 | 39.612 | 39.183 | 51 | 30 |
| div | 585138 | 0.816 | 3.911 | 3.811 | 100 | 18 |
| dowhile | 515169 | 0.740 | 4.625 | 4.508 | 100 | 17 |
| fordecl | 280864 | 0.402 | 4.946 | 4.896 | 100 | 10 |
| hello_world | 1209 | 0.005 | 0.007 | 0.007 | 100 | 14 |
| io | 123200 | 0.176 | 10.777 | 10.720 | 100 | 8 |
| logic | 812538 | 1.141 | 3.286 | 3.229 | 100 | 28 |
| loop | 236198 | 0.336 | 4.534 | 4.499 | 100 | 8 |
| mod | 617886 | 0.859 | 3.831 | 3.778 | 100 | 18 |
| mul | 462766 | 0.647 | 72.849 | 72.414 | 28 | 24 |
| printnum | 1360845 | 1.894 | 302.271 | 301.415 | 7 | 85 |
| switch | 106465 | 0.191 | 0.959 | 0.944 | 100 | 21 |
| ternary | 726950 | 1.031 | 8.821 | 8.626 | 100 | 24 |

Runner microbenchmarks:

| name | BF bytes | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|
| nested-loops | 43 | 0.008 | 0.007 | 100 | 1 |
| tape-walk | 200002 | 0.842 | 0.826 | 100 | 1 |
| cell-bump | 100001 | 0.622 | 0.611 | 100 | 1 |
| skip-scan | 100002 | 0.216 | 0.210 | 100 | 0 |
| io-echo | 5 | 0.066 | 0.065 | 100 | 4096 |
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
  so a cancellation pass would change nothing. The remaining
  structural lever is loop-based constant construction (`[-]` plus
  up to 255 `+`s occurs 12,816 times); it needs a scratch cell and
  would churn every golden, so it is deferred to future codegen work.
