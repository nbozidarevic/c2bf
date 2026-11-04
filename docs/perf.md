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
| addition | 15493 | 69 | 69 | 68 | 0.009 | 1.004 | 0.983 | 100 | 2 |
| addsub | 139838 | 207 | 207 | 206 | 0.140 | 17.347 | 16.612 | 100 | 28 |
| arrays | 121233 | 194 | 181 | 180 | 0.092 | 4.667 | 4.544 | 100 | 25 |
| assign | 278222 | 302 | 289 | 288 | 0.146 | 1.235 | 1.198 | 100 | 32 |
| bitassign | 138613 | 196 | 196 | 195 | 0.082 | 11.619 | 11.260 | 100 | 24 |
| bitwise | 508390 | 632 | 619 | 618 | 0.288 | 43.555 | 41.633 | 46 | 73 |
| branch | 6242 | 64 | 61 | 61 | 0.024 | 0.044 | 0.043 | 100 | 10 |
| break | 173494 | 183 | 170 | 169 | 0.137 | 3.218 | 2.913 | 100 | 24 |
| casts | 107759 | 141 | 141 | 140 | 0.062 | 15.149 | 14.907 | 100 | 22 |
| cmp | 261219 | 378 | 349 | 348 | 0.132 | 0.518 | 0.495 | 100 | 32 |
| complex | 81926 | 136 | 136 | 135 | 0.058 | 6.934 | 6.496 | 100 | 210 |
| continue | 233382 | 231 | 218 | 217 | 0.187 | 7.304 | 7.002 | 100 | 30 |
| define | 61477 | 103 | 90 | 89 | 0.034 | 0.455 | 0.333 | 100 | 12 |
| div | 163254 | 180 | 167 | 166 | 0.083 | 0.463 | 0.438 | 100 | 18 |
| dowhile | 142908 | 183 | 170 | 169 | 0.101 | 0.536 | 0.525 | 100 | 17 |
| fordecl | 69726 | 107 | 94 | 93 | 0.048 | 0.539 | 0.526 | 100 | 10 |
| functions | 817764 | 275 | 275 | 274 | 0.415 | 13.950 | 13.296 | 100 | 42 |
| hello_world | 597 | 24 | 24 | 23 | 0.003 | 0.005 | 0.005 | 100 | 14 |
| io | 67913 | 106 | 104 | 103 | 0.041 | 3.033 | 2.978 | 100 | 11 |
| literals | 138053 | 144 | 124 | 123 | 0.071 | 7.981 | 7.752 | 100 | 25 |
| logic | 216689 | 215 | 202 | 201 | 0.113 | 0.326 | 0.313 | 100 | 28 |
| loop | 54968 | 90 | 90 | 89 | 0.038 | 0.553 | 0.531 | 100 | 8 |
| mod | 171942 | 306 | 293 | 292 | 0.088 | 0.471 | 0.461 | 100 | 18 |
| mul | 125408 | 207 | 207 | 206 | 0.068 | 14.614 | 14.129 | 100 | 24 |
| multidecl | 64485 | 115 | 102 | 101 | 0.037 | 0.203 | 0.199 | 100 | 12 |
| pointers | 2955444 | 438 | 438 | 437 | 1.175 | 31.867 | 29.990 | 63 | 58 |
| printfmt | 162174 | 189 | 189 | 188 | 0.080 | 23.049 | 22.578 | 87 | 33 |
| printnum | 369202 | 328 | 328 | 327 | 0.190 | 63.096 | 61.487 | 32 | 85 |
| prototypes | 371582 | 121 | 121 | 120 | 0.161 | 5.065 | 4.941 | 100 | 14 |
| scanfstr | 1386140 | 219 | 193 | 192 | 0.551 | 72.147 | 71.107 | 28 | 54 |
| sizeof | 62091 | 102 | 89 | 88 | 0.038 | 0.139 | 0.134 | 100 | 12 |
| strings | 1616153 | 215 | 215 | 214 | 0.641 | 54.956 | 54.035 | 37 | 63 |
| switch | 39825 | 281 | 268 | 267 | 0.062 | 0.221 | 0.217 | 100 | 21 |
| ternary | 172820 | 237 | 224 | 223 | 0.113 | 1.054 | 1.016 | 100 | 24 |
| typespec | 87040 | 163 | 150 | 149 | 0.058 | 9.231 | 9.015 | 100 | 21 |
| unary | 93455 | 174 | 137 | 136 | 0.054 | 0.309 | 0.298 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.005 | 0.004 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.380 | 0.363 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.303 | 0.296 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.081 | 0.079 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.036 | 0.035 | 100 | 4096 |
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
