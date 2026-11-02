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
| addition | 56339 | 0.037 | 4.134 | 3.895 | 100 | 2 |
| addsub | 520649 | 0.708 | 65.043 | 52.883 | 31 | 28 |
| assign | 1787399 | 1.085 | 9.497 | 8.992 | 100 | 32 |
| bitassign | 621887 | 0.403 | 30.894 | 26.127 | 65 | 24 |
| bitwise | 2038802 | 1.300 | 123.003 | 119.637 | 17 | 73 |
| branch | 10781 | 0.029 | 0.096 | 0.084 | 100 | 10 |
| break | 632807 | 0.507 | 10.085 | 8.758 | 100 | 24 |
| casts | 396460 | 0.257 | 44.507 | 38.106 | 45 | 22 |
| cmp | 938027 | 0.596 | 3.181 | 3.044 | 100 | 32 |
| complex | 377184 | 0.252 | 20.903 | 14.421 | 96 | 210 |
| continue | 855741 | 0.656 | 22.385 | 21.125 | 90 | 30 |
| define | 226449 | 0.146 | 1.532 | 1.284 | 100 | 12 |
| div | 585138 | 0.371 | 2.782 | 2.483 | 100 | 18 |
| dowhile | 515169 | 0.350 | 2.662 | 2.584 | 100 | 17 |
| fordecl | 280864 | 0.187 | 2.676 | 2.510 | 100 | 10 |
| hello_world | 597 | 0.004 | 0.006 | 0.005 | 100 | 14 |
| io | 123335 | 0.084 | 5.200 | 4.997 | 100 | 8 |
| literals | 509240 | 0.321 | 20.649 | 16.725 | 97 | 25 |
| logic | 812538 | 0.517 | 2.393 | 2.060 | 100 | 28 |
| loop | 236198 | 0.156 | 2.558 | 2.421 | 100 | 8 |
| mod | 617886 | 0.392 | 2.757 | 2.616 | 100 | 18 |
| mul | 462570 | 0.294 | 49.890 | 42.772 | 41 | 24 |
| multidecl | 279181 | 0.176 | 1.345 | 1.240 | 100 | 12 |
| printfmt | 645117 | 0.405 | 79.468 | 56.053 | 26 | 33 |
| printnum | 1359034 | 0.850 | 198.888 | 169.848 | 11 | 85 |
| sizeof | 227220 | 0.148 | 1.021 | 1.000 | 100 | 12 |
| switch | 105679 | 0.116 | 0.648 | 0.556 | 100 | 21 |
| ternary | 726950 | 0.474 | 5.083 | 4.843 | 100 | 24 |
| typespec | 361423 | 0.236 | 35.367 | 30.364 | 57 | 21 |
| unary | 340913 | 0.224 | 1.415 | 1.193 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|
| nested-loops | 43 | 0.005 | 0.004 | 100 | 1 |
| tape-walk | 200002 | 0.464 | 0.429 | 100 | 1 |
| cell-bump | 100001 | 0.408 | 0.387 | 100 | 1 |
| skip-scan | 100002 | 0.167 | 0.158 | 100 | 0 |
| io-echo | 5 | 0.046 | 0.045 | 100 | 4096 |
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
- Fused divmod + in-place double/halve (this phase): bit extraction
  now uses one pair-removal loop per operand (`divmod_two`, via
  `halve_with_remainder`) instead of a divide plus a modulo per
  bit, shifts double/halve in place instead of general
  multiply/divide, and the fold stops once both quotients are
  exhausted; the shared divide/modulo/multiply helpers are
  untouched. `bitwise` falls 3,034,532 -> 2,038,802 BF bytes and
  `bitassign` 1,007,169 -> 621,887; the single-operation order
  probes fall further (196,706 -> 13,676 and 947 -> 370). In this
  run both fixtures also run faster with more adaptive iterations
  (17 and 65, from 3 and 9); run means are not comparable across
  runs, so the byte counts are the durable evidence here.
