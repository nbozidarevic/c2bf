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
| addition | 13985 | 67 | 67 | 66 | 0.013 | 1.988 | 1.870 | 100 | 2 |
| addsub | 126266 | 205 | 205 | 204 | 0.152 | 34.658 | 34.501 | 58 | 28 |
| arrays | 113274 | 214 | 201 | 200 | 0.138 | 9.012 | 8.335 | 100 | 25 |
| assign | 257923 | 310 | 297 | 296 | 0.218 | 1.842 | 1.792 | 100 | 32 |
| bitassign | 127165 | 210 | 210 | 209 | 0.120 | 18.739 | 18.516 | 100 | 24 |
| bitwise | 460258 | 638 | 625 | 624 | 0.398 | 81.132 | 71.572 | 25 | 73 |
| braceinit | 292759 | 209 | 209 | 208 | 0.275 | 9.847 | 9.671 | 100 | 28 |
| branch | 5636 | 64 | 60 | 61 | 0.033 | 0.062 | 0.060 | 100 | 10 |
| break | 155254 | 161 | 148 | 147 | 0.193 | 4.770 | 4.630 | 100 | 24 |
| casts | 97203 | 139 | 139 | 138 | 0.087 | 24.463 | 21.588 | 82 | 22 |
| cmp | 234565 | 376 | 347 | 346 | 0.192 | 0.741 | 0.662 | 100 | 32 |
| complex | 74149 | 134 | 134 | 133 | 0.085 | 8.468 | 8.256 | 100 | 210 |
| continue | 209911 | 218 | 205 | 204 | 0.272 | 9.305 | 9.199 | 100 | 30 |
| define | 55445 | 101 | 88 | 87 | 0.049 | 0.409 | 0.394 | 100 | 12 |
| div | 146154 | 178 | 165 | 164 | 0.123 | 0.635 | 0.560 | 100 | 18 |
| dowhile | 127346 | 147 | 134 | 133 | 0.144 | 0.686 | 0.626 | 100 | 17 |
| enums | 104402 | 131 | 122 | 121 | 0.098 | 3.771 | 3.220 | 100 | 18 |
| fnptr | 1726728 | 525 | 512 | 511 | 1.250 | 17.222 | 14.696 | 100 | 46 |
| fordecl | 63868 | 109 | 96 | 95 | 0.071 | 0.762 | 0.711 | 100 | 10 |
| formats | 1772157 | 625 | 431 | 618 | 1.143 | 71.168 | 68.020 | 29 | 42 |
| functions | 803284 | 281 | 281 | 280 | 0.638 | 19.705 | 16.214 | 100 | 42 |
| hello_world | 597 | 24 | 24 | 23 | 0.004 | 0.004 | 0.004 | 100 | 14 |
| io | 60929 | 104 | 102 | 101 | 0.061 | 4.318 | 4.296 | 100 | 11 |
| literals | 124481 | 142 | 122 | 121 | 0.100 | 13.096 | 11.442 | 100 | 25 |
| logic | 195177 | 213 | 200 | 199 | 0.173 | 0.451 | 0.421 | 100 | 28 |
| loop | 50087 | 90 | 90 | 89 | 0.058 | 0.666 | 0.657 | 100 | 8 |
| mod | 154842 | 304 | 291 | 290 | 0.133 | 0.620 | 0.586 | 100 | 18 |
| mul | 113344 | 205 | 205 | 204 | 0.100 | 22.779 | 22.729 | 88 | 24 |
| multidecl | 58453 | 113 | 100 | 99 | 0.056 | 0.249 | 0.244 | 100 | 12 |
| multidim | 5057282 | 635 | 635 | 634 | 3.201 | 62.179 | 59.685 | 33 | 75 |
| pointerret | 588182 | 80 | 80 | 79 | 0.370 | 13.886 | 13.770 | 100 | 10 |
| pointers | 3182667 | 484 | 484 | 483 | 2.023 | 42.957 | 42.276 | 47 | 58 |
| printfmt | 147112 | 187 | 187 | 186 | 0.112 | 47.860 | 47.733 | 42 | 33 |
| printnum | 333085 | 328 | 328 | 327 | 0.268 | 113.862 | 113.672 | 18 | 85 |
| prototypes | 365550 | 119 | 119 | 118 | 0.261 | 7.604 | 7.537 | 100 | 14 |
| ptrptr | 15378326 | 564 | 564 | 563 | 20.881 | 187.932 | 177.298 | 11 | 67 |
| scanfstr | 1368311 | 220 | 193 | 192 | 0.870 | 90.580 | 89.686 | 23 | 54 |
| sizeof | 56011 | 100 | 87 | 86 | 0.056 | 0.212 | 0.202 | 100 | 12 |
| storage | 57318 | 104 | 95 | 94 | 0.055 | 0.513 | 0.497 | 100 | 10 |
| strings | 1610128 | 215 | 215 | 214 | 1.050 | 71.210 | 69.959 | 29 | 63 |
| switch | 34464 | 173 | 160 | 159 | 0.088 | 0.280 | 0.255 | 100 | 21 |
| ternary | 156376 | 235 | 222 | 221 | 0.167 | 1.297 | 1.230 | 100 | 24 |
| typedefs | 799591 | 130 | 117 | 116 | 0.519 | 14.758 | 14.122 | 100 | 11 |
| typespec | 78768 | 161 | 148 | 147 | 0.082 | 17.334 | 17.033 | 100 | 21 |
| unary | 84407 | 172 | 137 | 136 | 0.075 | 0.443 | 0.420 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.004 | 0.003 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.594 | 0.481 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.317 | 0.298 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.144 | 0.133 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.051 | 0.050 | 100 | 4096 |
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
