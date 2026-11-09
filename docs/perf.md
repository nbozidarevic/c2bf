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
| addition | 13985 | 67 | 67 | 66 | 0.024 | 2.322 | 2.295 | 100 | 2 |
| addsub | 126266 | 205 | 205 | 204 | 0.255 | 40.137 | 39.593 | 50 | 28 |
| arrays | 113274 | 214 | 201 | 200 | 0.233 | 9.544 | 9.426 | 100 | 25 |
| assign | 257923 | 310 | 297 | 296 | 0.409 | 2.495 | 2.472 | 100 | 32 |
| bitassign | 127165 | 210 | 210 | 209 | 0.214 | 23.698 | 23.492 | 85 | 24 |
| bitwise | 460258 | 638 | 625 | 624 | 0.730 | 85.391 | 85.010 | 24 | 73 |
| braceinit | 292759 | 209 | 209 | 208 | 0.481 | 13.749 | 13.637 | 100 | 28 |
| branch | 5636 | 64 | 60 | 61 | 0.045 | 0.087 | 0.085 | 100 | 10 |
| break | 155254 | 161 | 148 | 147 | 0.328 | 6.160 | 6.111 | 100 | 24 |
| casts | 97203 | 139 | 139 | 138 | 0.159 | 31.668 | 29.783 | 64 | 22 |
| cmp | 234565 | 376 | 347 | 346 | 0.370 | 1.067 | 1.052 | 100 | 32 |
| complex | 74149 | 134 | 134 | 133 | 0.145 | 13.676 | 13.583 | 100 | 210 |
| continue | 209911 | 218 | 205 | 204 | 0.454 | 15.134 | 15.058 | 100 | 30 |
| define | 55445 | 101 | 88 | 87 | 0.091 | 0.680 | 0.667 | 100 | 12 |
| div | 146154 | 178 | 165 | 164 | 0.229 | 0.888 | 0.873 | 100 | 18 |
| dowhile | 127346 | 147 | 134 | 133 | 0.247 | 1.086 | 1.075 | 100 | 17 |
| enums | 104402 | 131 | 122 | 121 | 0.178 | 5.144 | 5.106 | 100 | 18 |
| fnptr | 1726728 | 525 | 512 | 511 | 2.637 | 22.687 | 22.407 | 89 | 46 |
| fordecl | 63868 | 109 | 96 | 95 | 0.122 | 1.198 | 1.172 | 100 | 10 |
| formats | 1772157 | 625 | 431 | 618 | 2.525 | 118.855 | 118.221 | 17 | 42 |
| functions | 803284 | 281 | 281 | 280 | 1.278 | 22.663 | 22.461 | 89 | 42 |
| goto_cleanup | 49605 | 91 | 83 | 82 | 0.091 | 2.507 | 2.480 | 100 | 4 |
| goto_cleanup_struct | 48650 | 116 | 82 | 81 | 0.088 | 2.833 | 2.804 | 100 | 4 |
| goto_dowhile | 35890 | 80 | 74 | 77 | 0.072 | 1.627 | 1.543 | 100 | 1 |
| goto_dowhile_struct | 33653 | 79 | 73 | 76 | 0.061 | 1.746 | 1.729 | 100 | 1 |
| goto_for | 33200 | 79 | 66 | 65 | 0.066 | 0.306 | 0.301 | 100 | 3 |
| goto_for_struct | 32948 | 79 | 66 | 65 | 0.063 | 0.334 | 0.327 | 100 | 3 |
| goto_fwd | 28219 | 67 | 52 | 51 | 0.050 | 0.093 | 0.091 | 100 | 1 |
| goto_fwd_struct | 28040 | 70 | 50 | 49 | 0.049 | 0.099 | 0.094 | 100 | 1 |
| goto_loop | 38600 | 76 | 76 | 75 | 0.078 | 1.870 | 1.850 | 100 | 4 |
| goto_loop_struct | 38456 | 91 | 91 | 90 | 0.077 | 2.206 | 2.191 | 100 | 4 |
| hello_world | 597 | 24 | 24 | 23 | 0.006 | 0.006 | 0.006 | 100 | 14 |
| io | 60929 | 104 | 102 | 101 | 0.106 | 6.925 | 6.859 | 100 | 11 |
| literals | 124481 | 142 | 122 | 121 | 0.196 | 18.322 | 17.961 | 100 | 25 |
| logic | 195177 | 213 | 200 | 199 | 0.313 | 0.662 | 0.647 | 100 | 28 |
| loop | 50087 | 90 | 90 | 89 | 0.098 | 1.097 | 1.085 | 100 | 8 |
| mod | 154842 | 304 | 291 | 290 | 0.244 | 0.897 | 0.878 | 100 | 18 |
| mul | 113344 | 205 | 205 | 204 | 0.188 | 28.986 | 27.845 | 69 | 24 |
| multidecl | 58453 | 113 | 100 | 99 | 0.101 | 0.413 | 0.405 | 100 | 12 |
| multidim | 5057282 | 635 | 635 | 634 | 7.214 | 89.368 | 87.956 | 23 | 75 |
| pointerret | 588182 | 80 | 80 | 79 | 0.824 | 19.061 | 18.030 | 100 | 10 |
| pointers | 3182667 | 484 | 484 | 483 | 4.542 | 51.981 | 51.625 | 39 | 58 |
| printfmt | 147112 | 187 | 187 | 186 | 0.225 | 54.740 | 54.359 | 37 | 33 |
| printnum | 333085 | 328 | 328 | 327 | 0.519 | 119.449 | 118.059 | 17 | 85 |
| prototypes | 365550 | 119 | 119 | 118 | 0.550 | 9.030 | 8.966 | 100 | 14 |
| ptrptr | 15378326 | 564 | 564 | 563 | 35.188 | 251.640 | 250.535 | 8 | 67 |
| scanfstr | 1368311 | 220 | 193 | 192 | 1.922 | 109.224 | 108.907 | 19 | 54 |
| sizeof | 56011 | 100 | 87 | 86 | 0.096 | 0.283 | 0.274 | 100 | 12 |
| storage | 57318 | 104 | 95 | 94 | 0.097 | 0.781 | 0.765 | 100 | 10 |
| strings | 1610128 | 215 | 215 | 214 | 2.263 | 92.310 | 91.873 | 22 | 63 |
| switch | 34464 | 173 | 160 | 159 | 0.126 | 0.416 | 0.408 | 100 | 21 |
| ternary | 156376 | 235 | 222 | 221 | 0.292 | 2.015 | 1.998 | 100 | 24 |
| typedefs | 799591 | 130 | 117 | 116 | 1.125 | 19.531 | 19.276 | 100 | 11 |
| typespec | 78768 | 161 | 148 | 147 | 0.149 | 16.455 | 15.926 | 100 | 21 |
| unary | 84407 | 172 | 137 | 136 | 0.144 | 0.575 | 0.563 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.005 | 0.004 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.750 | 0.731 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.409 | 0.395 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.206 | 0.195 | 100 | 0 |
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
