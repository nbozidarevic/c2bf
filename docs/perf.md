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
| addition | 15493 | 69 | 69 | 68 | 0.024 | 2.412 | 2.372 | 100 | 2 |
| addsub | 139838 | 207 | 207 | 206 | 0.289 | 40.190 | 37.997 | 50 | 28 |
| assign | 277812 | 302 | 289 | 288 | 0.406 | 2.654 | 2.614 | 100 | 32 |
| bitassign | 138613 | 196 | 196 | 195 | 0.212 | 25.409 | 25.307 | 79 | 24 |
| bitwise | 508390 | 632 | 619 | 618 | 0.785 | 80.684 | 79.221 | 25 | 73 |
| branch | 6242 | 64 | 61 | 61 | 0.035 | 0.096 | 0.095 | 100 | 10 |
| break | 173494 | 183 | 170 | 169 | 0.308 | 6.085 | 6.024 | 100 | 24 |
| casts | 107759 | 141 | 141 | 140 | 0.163 | 33.840 | 33.639 | 60 | 22 |
| cmp | 261219 | 378 | 349 | 348 | 0.379 | 1.105 | 1.088 | 100 | 32 |
| complex | 79457 | 134 | 134 | 133 | 0.137 | 14.269 | 14.205 | 100 | 210 |
| continue | 233174 | 231 | 218 | 217 | 0.434 | 14.622 | 14.546 | 100 | 30 |
| define | 61477 | 103 | 90 | 89 | 0.094 | 0.737 | 0.725 | 100 | 12 |
| div | 161256 | 180 | 167 | 166 | 0.233 | 0.917 | 0.897 | 100 | 18 |
| dowhile | 142908 | 183 | 170 | 169 | 0.252 | 1.149 | 1.133 | 100 | 17 |
| fordecl | 69726 | 107 | 94 | 93 | 0.115 | 1.171 | 1.158 | 100 | 10 |
| hello_world | 597 | 24 | 24 | 23 | 0.005 | 0.009 | 0.009 | 100 | 14 |
| io | 32074 | 82 | 82 | 81 | 0.054 | 3.734 | 3.703 | 100 | 8 |
| literals | 138053 | 144 | 124 | 123 | 0.199 | 18.891 | 18.715 | 100 | 25 |
| logic | 216689 | 215 | 202 | 201 | 0.318 | 0.712 | 0.696 | 100 | 28 |
| loop | 54968 | 90 | 90 | 89 | 0.091 | 0.994 | 0.976 | 100 | 8 |
| mod | 169722 | 306 | 293 | 292 | 0.243 | 0.905 | 0.885 | 100 | 18 |
| mul | 125408 | 207 | 207 | 206 | 0.183 | 26.067 | 25.942 | 77 | 24 |
| multidecl | 64485 | 115 | 102 | 101 | 0.098 | 0.422 | 0.409 | 100 | 12 |
| printfmt | 162174 | 189 | 189 | 188 | 0.230 | 56.700 | 56.547 | 36 | 33 |
| printnum | 369202 | 328 | 328 | 327 | 0.542 | 130.052 | 129.847 | 16 | 85 |
| sizeof | 62091 | 102 | 89 | 88 | 0.096 | 0.310 | 0.299 | 100 | 12 |
| switch | 39825 | 281 | 268 | 267 | 0.111 | 0.445 | 0.430 | 100 | 21 |
| ternary | 172820 | 237 | 224 | 223 | 0.279 | 2.220 | 2.191 | 100 | 24 |
| typespec | 86826 | 163 | 150 | 149 | 0.139 | 20.281 | 20.173 | 99 | 21 |
| unary | 93455 | 174 | 137 | 136 | 0.142 | 0.551 | 0.540 | 100 | 14 |

Runner microbenchmarks:

| name | BF bytes | cells touched | peak cell | run mean (ms) | run min (ms) | iters | out bytes |
|---|---|---|---|---|---|---|---|
| nested-loops | 43 | 3 | 2 | 0.009 | 0.007 | 100 | 1 |
| tape-walk | 200002 | 2 | 100000 | 0.673 | 0.649 | 100 | 1 |
| cell-bump | 100001 | 1 | 0 | 0.641 | 0.627 | 100 | 1 |
| skip-scan | 100002 | 1 | 0 | 0.201 | 0.195 | 100 | 0 |
| io-echo | 5 | 1 | 0 | 0.069 | 0.068 | 100 | 4096 |
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
