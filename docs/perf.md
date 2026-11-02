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
| addition | 56339 | 0.080 | 6.109 | 6.053 | 100 | 2 |
| addsub | 520649 | 1.158 | 96.773 | 96.173 | 21 | 28 |
| assign | 1787399 | 2.520 | 17.347 | 17.154 | 100 | 32 |
| bitassign | 1007169 | 1.419 | 226.053 | 225.633 | 9 | 24 |
| bitwise | 3034532 | 4.313 | 815.953 | 814.895 | 3 | 73 |
| branch | 10781 | 0.041 | 0.134 | 0.131 | 100 | 10 |
| break | 632807 | 0.992 | 15.623 | 13.832 | 100 | 24 |
| casts | 396460 | 0.567 | 72.545 | 72.208 | 28 | 22 |
| cmp | 938027 | 1.327 | 4.271 | 4.197 | 100 | 32 |
| complex | 377184 | 0.551 | 40.592 | 39.391 | 50 | 210 |
| continue | 855741 | 1.344 | 30.798 | 30.604 | 65 | 30 |
| define | 226449 | 0.325 | 2.947 | 2.916 | 100 | 12 |
| div | 585138 | 0.830 | 3.647 | 3.571 | 100 | 18 |
| dowhile | 515169 | 0.760 | 3.587 | 3.532 | 100 | 17 |
| fordecl | 280864 | 0.408 | 3.623 | 3.580 | 100 | 10 |
| hello_world | 597 | 0.006 | 0.008 | 0.008 | 100 | 14 |
| io | 123335 | 0.179 | 8.189 | 8.130 | 100 | 8 |
| literals | 509240 | 0.722 | 44.467 | 41.586 | 45 | 25 |
| logic | 812538 | 1.154 | 3.485 | 3.427 | 100 | 28 |
| loop | 236198 | 0.342 | 4.023 | 3.996 | 100 | 8 |
| mod | 617886 | 0.873 | 3.569 | 3.522 | 100 | 18 |
| mul | 462570 | 0.658 | 68.910 | 68.660 | 30 | 24 |
| multidecl | 279181 | 0.441 | 2.026 | 2.003 | 100 | 12 |
| printfmt | 645117 | 0.905 | 135.259 | 135.039 | 15 | 33 |
| printnum | 1359034 | 2.003 | 354.315 | 354.108 | 6 | 85 |
| sizeof | 227220 | 0.325 | 1.538 | 1.520 | 100 | 12 |
| switch | 105679 | 0.241 | 1.014 | 0.999 | 100 | 21 |
| ternary | 726950 | 1.055 | 9.468 | 9.347 | 100 | 24 |
| typespec | 361423 | 0.523 | 52.552 | 52.431 | 39 | 21 |
| unary | 340913 | 0.488 | 2.077 | 2.040 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|
| nested-loops | 43 | 0.009 | 0.006 | 100 | 1 |
| tape-walk | 200002 | 0.804 | 0.672 | 100 | 1 |
| cell-bump | 100001 | 0.594 | 0.579 | 100 | 1 |
| skip-scan | 100002 | 0.217 | 0.208 | 100 | 0 |
| io-echo | 5 | 0.071 | 0.070 | 100 | 4096 |
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
- Bitwise/shift helpers (this phase): each `& | ^` folds eight bits
  through divide/modulo/multiply expansions, and each shift repeats
  a multiply/divide; in the table above, the 25-line `bitwise`
  fixture emits 3,034,532 BF bytes and runs in ~816 ms (3 iters),
  and `bitassign` 1,007,169 bytes in ~226 ms. Helper expansion,
  not constants or the runner, is now the dominant cost for
  programs using these operators; a shared fold loop or cheaper
  bit extraction is the next structural lever, unmeasured so far.
