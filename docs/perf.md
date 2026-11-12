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
| addition | 13985 | 67 | 67 | 66 | 0.008 | 0.720 | 0.705 | 100 | 2 |
| addsub | 126266 | 205 | 205 | 204 | 0.082 | 12.051 | 11.798 | 100 | 28 |
| arrays | 113274 | 214 | 201 | 200 | 0.095 | 3.588 | 3.498 | 100 | 25 |
| assign | 257923 | 310 | 297 | 296 | 0.139 | 0.987 | 0.969 | 100 | 32 |
| bitassign | 127165 | 210 | 210 | 209 | 0.077 | 8.708 | 8.425 | 100 | 24 |
| bitwise | 460258 | 638 | 625 | 624 | 0.252 | 35.374 | 32.675 | 57 | 73 |
| braceinit | 292759 | 209 | 209 | 208 | 0.164 | 5.940 | 5.523 | 100 | 28 |
| branch | 5636 | 64 | 60 | 61 | 0.024 | 0.034 | 0.033 | 100 | 10 |
| break | 155254 | 161 | 148 | 147 | 0.136 | 2.355 | 2.309 | 100 | 24 |
| casts | 97203 | 139 | 139 | 138 | 0.056 | 12.301 | 11.746 | 100 | 22 |
| cmp | 234565 | 376 | 347 | 346 | 0.125 | 0.446 | 0.421 | 100 | 32 |
| complex | 77274 | 135 | 135 | 134 | 0.061 | 5.357 | 5.240 | 100 | 210 |
| continue | 209911 | 218 | 205 | 204 | 0.208 | 5.051 | 4.940 | 100 | 30 |
| define | 55445 | 101 | 88 | 87 | 0.059 | 0.284 | 0.261 | 100 | 12 |
| div | 146154 | 178 | 165 | 164 | 0.081 | 0.401 | 0.383 | 100 | 18 |
| dowhile | 127346 | 147 | 134 | 133 | 0.099 | 0.448 | 0.433 | 100 | 17 |
| enums | 104402 | 131 | 122 | 121 | 0.066 | 2.382 | 2.258 | 100 | 18 |
| fnptr | 1726728 | 525 | 512 | 511 | 0.808 | 9.608 | 9.318 | 100 | 46 |
| fnptr_typedef | 457329 | 262 | 249 | 248 | 0.239 | 5.145 | 5.031 | 100 | 19 |
| fnptrarr | 1241446 | 412 | 399 | 398 | 0.544 | 4.758 | 4.611 | 100 | 28 |
| fnptrret | 502191 | 300 | 300 | 299 | 0.261 | 3.056 | 3.020 | 100 | 20 |
| fordecl | 63868 | 109 | 96 | 95 | 0.048 | 0.482 | 0.464 | 100 | 10 |
| formats | 1815284 | 628 | 434 | 621 | 0.727 | 42.435 | 41.598 | 48 | 42 |
| functions | 803284 | 281 | 281 | 280 | 0.432 | 10.380 | 10.137 | 100 | 42 |
| goto_cleanup | 52714 | 92 | 84 | 83 | 0.036 | 1.053 | 1.033 | 100 | 4 |
| goto_cleanup_struct | 51729 | 117 | 83 | 82 | 0.033 | 1.085 | 1.025 | 100 | 4 |
| goto_dowhile | 39003 | 85 | 79 | 78 | 0.025 | 0.675 | 0.640 | 100 | 1 |
| goto_dowhile_struct | 36726 | 84 | 78 | 77 | 0.024 | 0.658 | 0.635 | 100 | 1 |
| goto_for | 33200 | 79 | 66 | 65 | 0.025 | 0.133 | 0.128 | 100 | 3 |
| goto_for_struct | 32948 | 79 | 66 | 65 | 0.024 | 0.134 | 0.131 | 100 | 3 |
| goto_fwd | 28219 | 67 | 52 | 51 | 0.018 | 0.037 | 0.037 | 100 | 1 |
| goto_fwd_struct | 28040 | 70 | 50 | 49 | 0.018 | 0.037 | 0.036 | 100 | 1 |
| goto_loop | 38600 | 76 | 76 | 75 | 0.031 | 0.852 | 0.809 | 100 | 4 |
| goto_loop_struct | 38456 | 91 | 91 | 90 | 0.031 | 0.906 | 0.854 | 100 | 4 |
| hello_world | 597 | 24 | 24 | 23 | 0.004 | 0.003 | 0.003 | 100 | 14 |
| io | 66999 | 110 | 110 | 109 | 0.041 | 2.763 | 2.724 | 100 | 11 |
| literals | 124481 | 142 | 122 | 121 | 0.064 | 6.728 | 6.592 | 100 | 25 |
| logic | 195177 | 213 | 200 | 199 | 0.106 | 0.279 | 0.274 | 100 | 28 |
| loop | 50087 | 90 | 90 | 89 | 0.038 | 0.457 | 0.427 | 100 | 8 |
| mod | 154842 | 304 | 291 | 290 | 0.081 | 0.386 | 0.369 | 100 | 18 |
| mul | 113344 | 205 | 205 | 204 | 0.065 | 11.423 | 11.195 | 100 | 24 |
| multidecl | 58453 | 113 | 100 | 99 | 0.037 | 0.168 | 0.165 | 100 | 12 |
| multidim | 5069027 | 642 | 638 | 637 | 2.165 | 41.469 | 40.399 | 49 | 75 |
| pointerret | 588182 | 80 | 80 | 79 | 0.237 | 7.287 | 7.194 | 100 | 10 |
| pointers | 3201856 | 487 | 487 | 486 | 1.280 | 24.091 | 23.439 | 84 | 58 |
| printfmt | 147112 | 187 | 187 | 186 | 0.073 | 19.131 | 18.701 | 100 | 33 |
| printnum | 333085 | 328 | 328 | 327 | 0.180 | 54.697 | 51.140 | 37 | 85 |
| prototypes | 365550 | 119 | 119 | 118 | 0.169 | 3.754 | 3.669 | 100 | 14 |
| ptrptr | 15390064 | 570 | 566 | 565 | 16.963 | 120.183 | 118.380 | 17 | 67 |
| scanf_eof | 363777 | 209 | 209 | 208 | 0.162 | 5.856 | 5.495 | 100 | 9 |
| scanfstr | 1371878 | 221 | 194 | 193 | 0.571 | 51.129 | 48.325 | 40 | 54 |
| sizeof | 56011 | 100 | 87 | 86 | 0.036 | 0.112 | 0.110 | 100 | 12 |
| storage | 57318 | 104 | 95 | 94 | 0.036 | 0.313 | 0.300 | 100 | 10 |
| strings | 1610128 | 215 | 215 | 214 | 0.650 | 39.415 | 38.521 | 51 | 63 |
| switch | 34464 | 173 | 160 | 159 | 0.063 | 0.165 | 0.161 | 100 | 21 |
| ternary | 156376 | 235 | 222 | 221 | 0.117 | 0.825 | 0.793 | 100 | 24 |
| typedefs | 799906 | 131 | 118 | 117 | 0.335 | 8.620 | 8.434 | 100 | 11 |
| typespec | 78768 | 161 | 148 | 147 | 0.060 | 7.070 | 6.913 | 100 | 21 |
| unary | 84407 | 172 | 137 | 136 | 0.050 | 0.254 | 0.249 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.003 | 0.002 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.380 | 0.371 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.201 | 0.198 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.081 | 0.079 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.032 | 0.029 | 100 | 4096 |
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
  bit extraction is the next structural lever, unmeasured at the
  time; the fused-divmod and raw combine/scale entries below
  address it.
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
- Scoped temp reclaim: `perform_if_else`/`perform_while` hand out
  again the temporaries allocated inside (never below the
  largest variable cell; high-water mark preserved).
  Single-construct probes pinned the waste to zero-iteration loops
  (divide 89% of allocated cells untouched), skipped branches
  (78%), and nested comparison/logic temporaries. The effect is
  much larger than allocation alone, because reused indices
  shorten pointer motion everywhere: `bitwise` 2,013,207 ->
  508,390 BF bytes on 632 cells (was 13,327), `bitassign`
  610,311 -> 138,613 bytes on 196 cells, `logic` on 215
  cells (was 5,846), `cmp` on 378 (was 7,206). `hello_world`
  (no branches, loops, or comparisons) is byte-identical.
- Arrays and pointers: constant indices resolve to static cells
  (`arrays` 121,233 BF bytes on 194 cells, comparable to the
  other small fixtures), but each dereference pays a full
  256-arm dispatch (`pointers` 2,952,282 BF bytes on 432 cells;
  the single-dereference `order_deref` probe alone is 351,123
  bytes). Runtime stays modest (the pointers fixture runs in
  ~55 ms); code size, not speed, is the cost, and sharing the
  dispatch or searching it in fewer steps is the next size
  lever for dereference sites; sharing was measured for
  `%s` only, and hand-rolled arms next (see below).
- Strings: eight `%s` sites, each paying the dispatch once
  via a hand-rolled do-while loop instead of once per
  character (`strings` 1,615,153 BF bytes on 215 cells;
  1,639,379 after the single-dispatch change, 3,036,947
  before it). The dispatch cost is per site, not per
  character: printing longer strings costs runtime steps, not
  more BF bytes.
- Hand-rolled dispatch arms (see decision 0008): each arm keeps
  the `perform_if_else` branch shape but shares one set of
  guard cells per dispatch (reclaimed afterwards) and emits
  its copy loop directly. The saving is ~1%: `pointers`
  2,978,087 -> 2,955,444, `strings` 1,639,379 -> 1,615,153,
  `functions` 824,222 -> 817,764 BF bytes, cells unchanged.
  A spike without the reclaim grew `pointers` by 344K bytes —
  the reclaim, not the arm shape, is what makes it a win. The
  copy's pointer traversals dominate each arm, so this lever
  is exhausted short of a structural change.
- `scanf %s` (a feature, not an optimization): each input
  character pays a dispatch store, so the `scanfstr` fixture
  emits 1,386,140 BF bytes on 219 cells. The cost is per
  character read, and bounded array reads pay the smaller
  offset dispatch instead of the full 256-arm one.
