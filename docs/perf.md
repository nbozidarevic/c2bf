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
| addition | 13985 | 67 | 67 | 66 | 0.023 | 2.446 | 2.312 | 100 | 2 |
| addsub | 126266 | 205 | 205 | 204 | 0.255 | 41.407 | 37.982 | 49 | 28 |
| arrays | 110629 | 192 | 179 | 178 | 0.233 | 9.736 | 9.360 | 100 | 25 |
| assign | 254816 | 300 | 287 | 286 | 0.406 | 2.568 | 2.474 | 100 | 32 |
| bitassign | 125805 | 194 | 194 | 193 | 0.210 | 24.437 | 23.557 | 82 | 24 |
| bitwise | 460310 | 630 | 617 | 616 | 0.731 | 95.349 | 93.392 | 21 | 73 |
| braceinit | 292759 | 209 | 209 | 208 | 0.463 | 14.118 | 13.657 | 100 | 28 |
| branch | 5636 | 64 | 60 | 61 | 0.043 | 0.092 | 0.090 | 100 | 10 |
| break | 154240 | 159 | 146 | 145 | 0.322 | 6.323 | 6.109 | 100 | 24 |
| casts | 97203 | 139 | 139 | 138 | 0.159 | 33.963 | 33.143 | 59 | 22 |
| cmp | 234565 | 376 | 347 | 346 | 0.368 | 1.099 | 1.069 | 100 | 32 |
| complex | 74076 | 134 | 134 | 133 | 0.146 | 14.058 | 13.629 | 100 | 210 |
| continue | 207707 | 214 | 201 | 200 | 0.443 | 15.517 | 15.076 | 100 | 30 |
| define | 55445 | 101 | 88 | 87 | 0.091 | 0.690 | 0.668 | 100 | 12 |
| div | 146154 | 178 | 165 | 164 | 0.228 | 0.976 | 0.942 | 100 | 18 |
| dowhile | 126476 | 147 | 134 | 133 | 0.245 | 1.104 | 1.074 | 100 | 17 |
| enums | 104402 | 131 | 122 | 121 | 0.181 | 5.198 | 5.146 | 100 | 18 |
| fordecl | 63014 | 105 | 92 | 91 | 0.121 | 1.011 | 1.001 | 100 | 10 |
| formats | 1772157 | 625 | 431 | 618 | 2.633 | 124.664 | 121.437 | 17 | 42 |
| functions | 789726 | 273 | 273 | 272 | 1.269 | 25.440 | 23.425 | 79 | 42 |
| hello_world | 597 | 24 | 24 | 23 | 0.005 | 0.007 | 0.007 | 100 | 14 |
| io | 60929 | 104 | 102 | 101 | 0.106 | 7.129 | 6.851 | 100 | 11 |
| literals | 124481 | 142 | 122 | 121 | 0.192 | 19.315 | 18.191 | 100 | 25 |
| logic | 195177 | 213 | 200 | 199 | 0.310 | 0.699 | 0.663 | 100 | 28 |
| loop | 49584 | 88 | 88 | 87 | 0.096 | 0.982 | 0.928 | 100 | 8 |
| mod | 154842 | 304 | 291 | 290 | 0.240 | 0.959 | 0.899 | 100 | 18 |
| mul | 113344 | 205 | 205 | 204 | 0.180 | 29.737 | 23.013 | 68 | 24 |
| multidecl | 58453 | 113 | 100 | 99 | 0.105 | 0.423 | 0.400 | 100 | 12 |
| multidim | 4935462 | 619 | 619 | 618 | 7.502 | 90.624 | 89.011 | 23 | 75 |
| pointerret | 588182 | 80 | 80 | 79 | 0.879 | 20.496 | 18.405 | 98 | 10 |
| pointers | 2917520 | 436 | 436 | 435 | 4.234 | 57.156 | 54.027 | 35 | 58 |
| printfmt | 147112 | 187 | 187 | 186 | 0.221 | 57.130 | 55.134 | 36 | 33 |
| printnum | 333010 | 326 | 326 | 325 | 0.519 | 123.261 | 119.680 | 17 | 85 |
| prototypes | 365550 | 119 | 119 | 118 | 0.559 | 8.636 | 7.640 | 100 | 14 |
| ptrptr | 14138504 | 532 | 532 | 531 | 33.332 | 251.119 | 249.214 | 8 | 67 |
| scanfstr | 1370144 | 218 | 191 | 190 | 1.923 | 121.458 | 119.585 | 17 | 54 |
| sizeof | 56011 | 100 | 87 | 86 | 0.097 | 0.318 | 0.289 | 100 | 12 |
| storage | 57318 | 104 | 95 | 94 | 0.099 | 0.857 | 0.817 | 100 | 10 |
| strings | 1609721 | 213 | 213 | 212 | 2.251 | 97.239 | 95.982 | 21 | 63 |
| switch | 34431 | 171 | 158 | 157 | 0.125 | 0.425 | 0.412 | 100 | 21 |
| ternary | 156596 | 235 | 222 | 221 | 0.288 | 2.104 | 2.033 | 100 | 24 |
| typedefs | 812500 | 128 | 115 | 114 | 1.164 | 20.344 | 19.536 | 99 | 11 |
| typespec | 78768 | 161 | 148 | 147 | 0.146 | 17.010 | 16.076 | 100 | 21 |
| unary | 84407 | 172 | 137 | 136 | 0.141 | 0.595 | 0.561 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.006 | 0.005 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.884 | 0.843 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.492 | 0.453 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.210 | 0.201 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.063 | 0.062 | 100 | 4096 |
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
