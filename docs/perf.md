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
| addition | 56339 | 0.048 | 4.351 | 3.548 | 100 | 2 |
| addsub | 520649 | 0.888 | 88.155 | 86.650 | 23 | 28 |
| assign | 1787399 | 1.396 | 12.134 | 11.266 | 100 | 32 |
| bitassign | 610311 | 0.500 | 37.701 | 36.838 | 54 | 24 |
| bitwise | 2013207 | 1.642 | 154.301 | 152.891 | 13 | 73 |
| branch | 10781 | 0.037 | 0.109 | 0.107 | 100 | 10 |
| break | 632807 | 0.611 | 12.799 | 11.244 | 100 | 24 |
| casts | 396460 | 0.413 | 51.987 | 42.973 | 39 | 22 |
| cmp | 938027 | 0.762 | 4.069 | 3.904 | 100 | 32 |
| complex | 377184 | 0.324 | 28.969 | 21.872 | 70 | 210 |
| continue | 855741 | 0.797 | 28.644 | 22.187 | 70 | 30 |
| define | 226449 | 0.189 | 1.945 | 1.513 | 100 | 12 |
| div | 585138 | 0.474 | 3.635 | 3.396 | 100 | 18 |
| dowhile | 515169 | 0.448 | 3.409 | 3.318 | 100 | 17 |
| fordecl | 280864 | 0.239 | 3.486 | 3.392 | 100 | 10 |
| hello_world | 597 | 0.004 | 0.008 | 0.007 | 100 | 14 |
| io | 123335 | 0.108 | 7.630 | 7.397 | 100 | 8 |
| literals | 509240 | 0.453 | 26.956 | 18.987 | 75 | 25 |
| logic | 812538 | 0.662 | 3.054 | 2.341 | 100 | 28 |
| loop | 236198 | 0.202 | 3.168 | 3.115 | 100 | 8 |
| mod | 617886 | 0.500 | 3.484 | 3.360 | 100 | 18 |
| mul | 462570 | 0.383 | 63.008 | 62.397 | 32 | 24 |
| multidecl | 279181 | 0.226 | 1.655 | 1.604 | 100 | 12 |
| printfmt | 645117 | 0.518 | 122.072 | 120.588 | 17 | 33 |
| printnum | 1359034 | 1.093 | 296.597 | 296.125 | 7 | 85 |
| sizeof | 227220 | 0.190 | 1.344 | 1.287 | 100 | 12 |
| switch | 105679 | 0.146 | 0.788 | 0.764 | 100 | 21 |
| ternary | 726950 | 0.614 | 6.489 | 6.187 | 100 | 24 |
| typespec | 361423 | 0.300 | 47.336 | 39.393 | 43 | 21 |
| unary | 340913 | 0.283 | 1.839 | 1.745 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|
| nested-loops | 43 | 0.007 | 0.006 | 100 | 1 |
| tape-walk | 200002 | 0.601 | 0.557 | 100 | 1 |
| cell-bump | 100001 | 0.665 | 0.499 | 100 | 1 |
| skip-scan | 100002 | 0.208 | 0.204 | 100 | 0 |
| io-echo | 5 | 0.060 | 0.058 | 100 | 4096 |
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
- Raw combine/scale (follow-up): the fold combines bits with
  single raw loops (`combine_and`/`or`/`xor` over the `0`/`1`
  cells) and accumulates the place value only when the bit is
  set (`accumulate_bit`), instead of general multiply/add/scale
  expansions. A further small byte reduction only: `bitwise`
  2,038,802 -> 2,013,207 BF bytes, `bitassign` 621,887 -> 610,311,
  order probe 13,676 -> 8,927. Runtime was flat in the sampled
  runs; the remaining fold cost is the divmod pair loop, the
  early-exit negates, and printing.
