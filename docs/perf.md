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

| program | BF bytes | cells alloc | cells touched | peak cell | compile mean (ms) | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|---|---|
| addition | 56339 | 417 | 401 | 416 | 0.079 | 6.356 | 6.247 | 100 | 2 |
| addsub | 520649 | 3764 | 2499 | 3763 | 1.179 | 101.672 | 101.104 | 20 | 28 |
| assign | 1787399 | 6423 | 3745 | 6422 | 2.489 | 17.061 | 16.789 | 100 | 32 |
| bitassign | 610311 | 3548 | 3042 | 3547 | 0.861 | 57.114 | 56.781 | 36 | 24 |
| bitwise | 2013207 | 13327 | 7722 | 13326 | 2.818 | 171.976 | 171.613 | 12 | 73 |
| branch | 10781 | 228 | 210 | 227 | 0.042 | 0.153 | 0.146 | 100 | 10 |
| break | 632807 | 4748 | 3033 | 4747 | 0.983 | 14.267 | 13.904 | 100 | 24 |
| casts | 396460 | 2896 | 2428 | 2895 | 0.561 | 78.629 | 70.597 | 26 | 22 |
| cmp | 938027 | 7206 | 2301 | 7205 | 1.303 | 4.535 | 4.475 | 100 | 32 |
| complex | 377184 | 2081 | 1546 | 2080 | 0.548 | 46.880 | 46.555 | 43 | 210 |
| continue | 855741 | 6200 | 4621 | 6199 | 1.322 | 32.194 | 30.942 | 63 | 30 |
| define | 226449 | 1655 | 1030 | 1654 | 0.320 | 2.938 | 2.538 | 100 | 12 |
| div | 585138 | 4448 | 2088 | 4447 | 0.818 | 4.040 | 4.013 | 100 | 18 |
| dowhile | 515169 | 3866 | 2242 | 3865 | 0.749 | 3.745 | 3.387 | 100 | 17 |
| fordecl | 280864 | 1835 | 1365 | 1834 | 0.405 | 3.418 | 3.383 | 100 | 10 |
| hello_world | 597 | 24 | 24 | 23 | 0.005 | 0.009 | 0.009 | 100 | 14 |
| io | 123335 | 857 | 729 | 856 | 0.180 | 8.443 | 8.351 | 100 | 8 |
| literals | 509240 | 3704 | 2439 | 3703 | 0.713 | 47.237 | 47.026 | 43 | 25 |
| logic | 812538 | 5846 | 1372 | 5845 | 1.150 | 3.091 | 3.050 | 100 | 28 |
| loop | 236198 | 1458 | 1189 | 1457 | 0.342 | 3.474 | 3.308 | 100 | 8 |
| mod | 617886 | 4574 | 1997 | 4573 | 0.862 | 3.553 | 3.508 | 100 | 18 |
| mul | 462570 | 3363 | 2474 | 3362 | 0.651 | 68.853 | 67.443 | 30 | 24 |
| multidecl | 279181 | 1669 | 849 | 1668 | 0.391 | 2.017 | 2.003 | 100 | 12 |
| printfmt | 645117 | 4111 | 3096 | 4110 | 0.895 | 142.160 | 141.643 | 15 | 33 |
| printnum | 1359034 | 9900 | 8876 | 9899 | 1.893 | 363.740 | 362.388 | 6 | 85 |
| sizeof | 227220 | 1685 | 839 | 1684 | 0.321 | 1.572 | 1.510 | 100 | 12 |
| switch | 105679 | 1283 | 1031 | 1282 | 0.198 | 0.945 | 0.900 | 100 | 21 |
| ternary | 726950 | 4515 | 3071 | 4514 | 1.042 | 8.945 | 8.871 | 100 | 24 |
| typespec | 361423 | 2264 | 1973 | 2263 | 0.517 | 53.735 | 53.411 | 38 | 21 |
| unary | 340913 | 2543 | 743 | 2542 | 0.485 | 1.832 | 1.810 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.008 | 0.006 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.727 | 0.710 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.626 | 0.611 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.217 | 0.210 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.069 | 0.067 | 100 | 4096 |
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
