# Changelog

All notable changes to this project. Crate versions were `0.0.0`
until the rename to `c2bf` (below); they are `0.1.0` from that
point, matching tag `v0.1.0`, `0.2.0` from the release below
that, matching tag `v0.2.0`, `0.3.0` from the release below
that, matching tag `v0.3.0`, `0.3.1` from the release below
that, matching tag `v0.3.1`, `0.4.0` from the release below
that, matching tag `v0.4.0`, `0.4.1` from the release below
that, matching tag `v0.4.1`, `0.5.0` from the release below
that, matching tag `v0.5.0`, `0.6.0` from the release below
that, matching tag `v0.6.0`, and `0.6.1` from the release at
the top, matching tag `v0.6.1`. Entries below are grouped by
the roadmap phases that produced them. See `docs/versioning.md`
for when and how versions change.

## Unreleased

- Language: the deferred function-pointer forms from
  record 0019 — typedef function-pointer types,
  function-pointer returns (typedef and direct
  spellings), and one-dimensional arrays of function
  pointers (including array parameters), with subscript
  calls, signature-checked assignment, and deref
  collapse. Pinned by the `fnptr_typedef`, `fnptrret`,
  and `fnptrarr` oracle fixtures. Decision record:
  `docs/decisions/0022-fnptr-deferred-forms.md`.
- Tests: differential fuzzer generates typedef
  function-pointer spellings and at most one
  function-pointer array per program, with per-element
  binding tracking and helper-bound element calls.

## v0.6.1

- Fixed: `scanf` number conversions (`%d`, `%u`, `%o`, `%x`,
  `%i`) ending exactly at end of input no longer spin
  forever: the readers detect EOF with a zero sentinel, as
  `%s` already did, and return the accumulated value.
  Pinned by the `scanf_eof` oracle fixture and unit tests.
  Decision record:
  `docs/decisions/0021-scanf-eof-sentinel.md`.
- Tests: differential fuzzer stdin now sometimes omits its
  trailing newline, so sweeps exercise EOF-terminated
  `scanf` conversions. 500-seed sweep (goto segments plus
  EOF-terminated inputs): 458 pass, 42 skip (address
  limit), 0 mismatches.

## v0.6.0

- Language: forward-only `goto` (`goto done;` to a label
  defined later in the same function), compiled with one
  flag cell per label; jumps out of loops and `if` arms and
  over initializers (which keep zero) are supported, while
  backward jumps and jumps into loops, `if` arms, or
  `switch` bodies are compile errors. Pinned by the
  `goto_*` oracle fixtures. Decision record:
  `docs/decisions/0020-goto-spike.md`.
- Tests: differential fuzzer generates at most one forward
  `goto` segment per program, jumping over `printf`s to a
  label carrying one scalar assignment. 500-seed sweep: 461
  pass, 39 skip (address limit), 0 mismatches.

## v0.5.0

- Language: function pointers (`int (*fp)(int)`) holding
  callee IDs, with signature-checked assignment,
  dereference collapsing, and indirect calls dispatching
  over matching definitions (`fp(args)`, `(*fp)(args)`).
  Null calls yield zero; arithmetic on function pointers
  and stores through dereferenced function pointers are
  compile errors. Pinned by the `fnptr` oracle fixture.
  Decision record:
  `docs/decisions/0019-function-pointers.md`.
- Tests: differential fuzzer generates function-pointer
  variables bound to the helper or null, indirect calls,
  comparisons, and rebinding with symbolic binding
  tracking. 500-seed sweep: 459 pass, 41 skip (address
  limit), 0 mismatches.

## v0.4.1

- Fixed: assignment expressions yield the value stored,
  captured in a fresh cell, instead of the target cell
  (`(x = 1) + (x = 2)` is 3, not 4). Measured on a spike:
  +4.81% BF bytes, +182 allocated cells, oracle suite
  green. Decision record:
  `docs/decisions/0017-assignment-value-capture.md`.
- Parser: a typedef name hidden by a same-named variable,
  parameter, or `for` declarator no longer expands in
  specifier position; it is a parse error, matching C
  scoping. Decision record:
  `docs/decisions/0018-hardening-shadowing-extern-coverage.md`.
- Fixed: using a name declared only as `extern` and never
  defined is a compile error (undefined reference) instead
  of allocating a zero cell. `sizeof` operands and uses in
  uncalled functions do not count as uses.
- Tests: differential fuzzer generates 3D arrays, `**`
  helpers, one file-scope typedef per program, and `scanf`
  `%s`/`%c` shapes (negative tokens stay ungenerated: they
  would need dual C/byte intervals). 500-seed sweep: 468
  pass, 32 skip (address limit), 0 mismatches.

## v0.4.0

- Language: pointer-to-pointer (`int **pp`, depth at most 2,
  including parameters, `**` returns, and typedef composition),
  addressed through the dereference dispatch (`**pp`,
  `pp[i][j]`, `(*pp)[j]`, `scanf` through `*pp`). Pinned by
  the `ptrptr` oracle fixture.
  Decision record: `docs/decisions/0016-pointer-to-pointer-multi-dim-arrays.md`.
- Language: multi-dimensional arrays (row-major, flat and
  nested initializer lists with zero fill, row decay,
  multi-dimensional parameters with stride dimensions,
  `sizeof` of rows). String initializers, mixed flat/nested
  lists, and unsized dimensions after the first are compile
  errors. Pinned by the `multidim` oracle fixture.
- Tests: differential fuzzer generates fixed `**` bindings
  with dereference chains and at most one 3x3
  two-dimensional array per program. 500-seed sweep: 460
  pass, 40 skip (address limit), 0 mismatches.

## v0.3.1

- Tests: differential fuzzer (`tests/fuzz.py`, `docs/fuzzing.md`)
  generating random C-subset programs against the g++ oracle,
  run for 50 seeds on every push plus an on-demand `fuzz-sweep`
  workflow for larger runs.
- Codegen: based `scanf` readers (`%o`/`%x`/`%i`) shrunk
  (one `%i` read: 502 to 171 cells, 270,091 to 73,489 BF
  bytes; the `formats` fixture: -1,326,964 bytes) via cheaper
  digit classification and rescan loops. Programs that
  previously failed with "address is outside the addressable
  range" after a based read now compile; deeply nested
  programs can still hit that limit (see `docs/fuzzing.md`).
  Decision record: `docs/decisions/0012-scanf-reader-shrink.md`.
- Codegen: `switch` matches are computed just in time per
  case and reclaimed after the case's guard (eager shape
  kept when `default` is not last): the `switch` fixture
  falls to 171 cells from 279. Decision record:
  `docs/decisions/0013-switch-matches-just-in-time.md`.
- Codegen: the do...while first body emission's temporaries
  are reclaimed before the loop re-emission: the `dowhile`
  fixture falls to 147 cells from 181. Decision record:
  `docs/decisions/0014-do-while-first-emission-reclaim.md`.
- Fixed: a string literal whose storage (including the NUL
  terminator) would cross cell 255 now fails to compile
  instead of printing past the tape (the print walk wrapped
  to cell 0 and read unrelated data; found by differential
  fuzzing, seed 65). The check is the whole-storage bound in
  `string_value`, pinned by `string_literal_straddling_255_errors`.
- Demo: samples for braced initializers (array + string lists)
  and I/O formats (`scanf %i` base detection, `printf`
  `%d`/`%o`/`%X`), asserted in the Node WASM smoke test like
  the existing samples.
- Tests: the fuzzer reports compiler rejections carrying only
  the documented addressable-cell limit as skips (capped at
  10% of seeds; any skip fails a single-seed run) instead of
  failures, since no generator bound separates fitting from
  over-limit programs. 500-seed sweep: 476 pass, 24 skip,
  0 mismatches. Decision record:
  `docs/decisions/0015-straddle-bound-and-skip-policy.md`.

## v0.3.0

- Language: braced initializer lists for arrays (in-order fill,
  zero fill, unsized sizing by count, one-string lists as string
  initializers) and one-element lists for scalar and `for`
  declarators; empty, nested, and designated lists are errors.
- I/O: `printf` `%o`/`%X` and `scanf` `%o`/`%x`/`%i`/`%u`
  (base detection and `0x` prefixes), length modifiers in both,
  and `scanf` assignment suppression (`%*...`); flags, widths,
  and precisions are still rejected.

## v0.2.0

- CLI: `check` command validates C source without emitting
  Brainfuck; `compile -s`/`--stats` prints one
  `BF bytes: N, cells: M` line to stderr while stdout stays pure
  Brainfuck.
- Demo: sample selector (arithmetic, loop + `scanf`, strings,
  functions, pointers); every sample is asserted in the Node WASM
  smoke test.
- WASM build stability: `wasm-pack` pinned to `v0.15.1` in CI and
  `wasm-opt` disabled, after nondeterministic "latest" resolution
  and an intermittent binaryen parse failure.
- Docs: `docs/semantics.md` tidied (variable indices, `sizeof`,
  and stale runner deviations corrected).
- Codegen: then-only `perform_if` replaces `perform_if_else` at
  every empty-else site (logic/comparison helpers, guards,
  division, `%s`/`scanf` checks), saving 507,524 BF bytes across
  the corpus with no fixture growing; allocated cells drop by 2
  on most fixtures.
- Codegen: the `bitwise_fold` early-exit test uses a guard
  (`negate` + `logical_and`) instead of a multiply, saving
  2,992 BF bytes across the bitwise fixtures with the rest
  byte-identical.
- Runner: cached tape index, ~9.5% faster in a within-run A/B
  (geomean over 41 bench cases); runtime footprints are
  unchanged.
- Language: `typedef` aliases (file-scope only, resolved at
  parse time), including pointer/array aliases, chained aliases,
  and alias spellings in parameters, return types, and
  prototypes; function signatures now also compare pointer
  returns.
- Language: `enum` definitions (file-scope only) with constant
  enumerator expressions, substituted at parse time, including
  shadowing by declared names; out-of-range values wrap like
  every byte value.
- Language: storage class specifiers (`static`/`extern` at file
  scope, `auto`/`register` ignored); a bare file-scope `extern`
  declarator is a non-defining declaration. `static`/`extern`
  in blocks, parameters, `for` declarations, and `typedef`s are
  parse errors.
- Language: direct pointer-return declarators (`char *f()`),
  including prototypes, with the same signature as the
  typedef-alias form; multi-word specifiers now also join
  before pointer declarators (`unsigned int *p`).

## Rename to `c2bf` (Phase 8)

The project was renamed from `freud` to `c2bf` in a single sweep, per
`docs/rename-checklist.md`: crate names (`c2bf-core`, `c2bf-ffi`,
`c2bf-wasm`), binaries (`c2bf`, `c2bf-bench`), the C ABI (`c2bf_*`,
`c2bf.h`), the WASM package, docs, tests, and CI. Golden outputs
were regenerated; the rename changed no code generation. The rename
also moved the crate versions from `0.0.0` to `0.1.0`. Entries
below keep the pre-rename names as history.

## Phase 7 — Distribution adapters

- C ABI adapter (`rs/ffi/`): separate `freud-ffi` crate exposing
  `freud_compile`/`freud_run`/`freud_format` with a hand-written
  `freud.h`, a C example, Rust ABI tests, and a `c-abi` CI job on
  Ubuntu and macOS. The core crate stays dependency-free.
- Web adapter (`rs/wasm/`, `demo/`): `wasm-bindgen` crate exporting
  `compile`/`run`/`format_code`, a static demo page, a Node smoke
  test, and a dispatch-only GitHub Pages workflow. All three
  `wasm-pack` targets (`web`, `nodejs`, `bundler`) are CI-verified;
  Pages deployment is a manual, owner-gated step. npm publishing is
  not planned.
- UniFFI (Kotlin/Swift) and JNI (Java) bridges are named and
  deferred; both build on the C ABI. See
  `docs/decisions/0006-distribution-adapters.md`.

## Phase 6 — Language expansion

- `do...while` completes the thesis iteration set (`while`/`for`/
  `do...while`).
- `break` inside loops and `switch`, via a flag protocol;
  break-free loops emit byte-identical code.
- Ternary conditional (`?:`) as a value-producing expression.
- `continue` inside loops, via a parallel flag protocol;
  `continue` binds the loop, not an enclosing `switch`, and the
  `for` update still runs.
- Decimal printing fixed (`print_as_number` rewritten around
  place values): numbers with interior zeros printed wrong
  (101 as 110, 209 as 2900) and cost tens of millions of extra
  interpreter steps; all byte values now print correctly. Covered
  by the `tests/compile/printnum.c` oracle fixture and a refreshed
  `docs/perf.md`.
- Unary `-`, `+`, `~` (byte-wrapping: `-x` is `0 - x`, `~x` is
  `255 - x`); `%d` of a wrapped negative differs from C `int`
  arithmetic, pinned by the wrap-back `unary` oracle fixture.
- `printf` formats `%%`, `%u`, and `%x` (lowercase hex, place-value
  printing like `%d`); the specifier is now checked before consuming
  an argument, so `%%` consumes none. Pinned by the `printfmt`
  oracle fixture.
- Integer literals in hex (`0x`), binary (`0b`), and octal (`0`
  prefix, 8/9 rejected as in g++), with `u`/`l` suffixes; values
  still truncate to a byte. Pinned by the `literals` oracle fixture.
- Multiple declarators per declaration (`int a = 1, b = 2;`, local
  and global); a declarator without an initializer is still an
  error, per declarator as before. The old `n5_multi_decl` negative
  fixture is retired; pinned by the `multidecl` oracle fixture.
- Multi-word type specifiers (`unsigned int`, `const char`, `long`,
  `signed`, `long long`), including `for` init declarators and
  function parameters; all spellings denote one byte. Pinned by
  the `typespec` oracle fixture.
- Casts (`(int)`, `(char)`, multi-word spellings) as byte no-ops;
  a lone parenthesized qualifier (e.g. `(unsigned);`) still parses
  as a parenthesized variable, not a cast. Pinned by the `casts`
  oracle fixture.
- `sizeof` (expression and type forms) as a constant `1`, with the
  operand unevaluated; documents the divergence from C for `int`.
  Pinned by the `sizeof` oracle fixture (char-typed uses only).
- Object-like `#define`/`#undef` expanded at tokenize time (chained,
  redefinable, self-reference-safe; function-like macros are an
  error). Other `#` lines are still skipped. Pinned by the `define`
  oracle fixture.
- Bitwise `& | ^` (eight-bit fold) and shifts `<< >>` (repeated
  double/halve; counts of eight or more give zero), with C
  precedence from loosest (`|` `^` `&`, then equality, relational,
  shift, additive) and right-to-left operand evaluation. Pinned
  by the `bitwise` oracle fixture, including double-side-effect
  order lines, and by `order_bitwise`/`order_shift` probes.
- Compound bitwise/shift assignment (`&= |= ^= <<= >>=`); the
  shift forms need three-char lexing so `<<=` is one token.
  Pinned by the `bitassign` oracle fixture.
- Bitwise/shift optimization: fused divide/modulo-by-two bit
  extraction (`divmod_two` over a probe-copy pair loop), in-place
  doubling/halving for shifts, and early fold exit once both
  quotients are exhausted. Shared divide/modulo/multiply helpers
  untouched; only the `bitwise`, `bitassign`, and order-probe
  goldens change. `bitwise` 3,034,532 -> 2,038,802 BF bytes,
  `bitassign` 1,007,169 -> 621,887; `docs/perf.md` refreshed.
- Follow-up: raw bit-combine loops and conditional place
  accumulation in the fold (`combine_and`/`or`/`xor`,
  `accumulate_bit`), replacing the general multiply/add/scale
  steps. `bitwise` 2,038,802 -> 2,013,207 BF bytes, `bitassign`
  621,887 -> 610,311; runtime flat in the sampled runs.
- Scoped temp reclaim: `perform_if_else`/`perform_while` hand
  out again the temporaries allocated inside (never below the
  largest variable cell; a high-water mark keeps the tape
  footprint honest). `bitwise` 2,013,207 -> 508,390 BF bytes
  on 632 cells (was 13,327), `bitassign` 610,311 -> 138,613
  bytes on 196 cells, `logic` on 215 cells (was 5,846);
  `hello_world` byte-identical. `docs/perf.md` refreshed.
- Arrays, constant-index stage: `char a[N]` (positive literal
  size, no initializers) with cleared, consecutive elements;
  constant subscripts resolve to static cells, so element
  assignment, increment, and compound assignment reuse the scalar
  paths, and `sizeof` an array is its element count. The reclaim
  floor now covers array cells too. Variable indices, pointer
  dereference, and address-of follow. Pinned by the `arrays`
  oracle fixture and `array_*` compile-error fixtures; pointer
  declarators parse but are not yet usable.
- Pointers and variable indices: address-of (`&`), dereference
  (`*`), pointer declarators, decay, pointer arithmetic, pointer
  subscripts, and `scanf` through addresses, over a subtractive
  dispatch (`load_from_cells`/`store_to_cells` in the emitter:
  one arm per cell, a counter that cannot re-match after
  wraparound). Addresses are bytes, so only the first 256 cells
  are addressable and taking a higher address is a compile
  error; out-of-range variable indices load zero and store
  nowhere. Pinned by the `pointers` oracle fixture,
  `order_index`/`order_deref` emission probes, and
  `pointer_*` compile-error fixtures.
- Functions, groundwork: parameters are parsed and kept
  (`params`/`param` in the parsedump, with `pointer`/`array`
  markers; array parameters decay to pointers), every function
  definition is scope-renamed before emission so shadowed
  declarations get distinct cells (previously all same-named
  declarations shared one cell), duplicate definitions and
  duplicate parameter names are rejected, and a lone definition
  without `main` is valid but emits nothing. Calls do not expand
  yet. The `non_main.c` error fixture is retired.
- Functions, inline expansion: calls expand the scope-renamed
  body at the call site under a fresh `returned` flag and result
  cell, with arguments evaluated left-to-right before binding,
  pointer/array parameters receiving addresses, and `return`
  gating the rest of the callee (including loop re-entry, the
  `for` update, and later `switch` cases). Recursion and wrong
  argument counts are compile errors. Pinned by the `functions`
  oracle fixture and `function_*` compile-error fixtures.
- Functions, parameter scope correction: a declaration at the top
  level of a function body reusing a parameter's name is now a
  compile error ("redefinition of parameter"), verified against
  g++ and gcc 15.2 (Ubuntu CI probe) and Apple Clang 21, which all
  reject it; the renamer previously shadowed it silently. Nested
  shadowing and `for`-initializer shadowing stay legal. Pinned by
  the `function_param_shadow` error fixture.
- Same-scope redeclaration correction: redeclaring a name in the
  same block is now a compile error ("redefinition") in every
  scalar/pointer combination, verified against the same compilers;
  the renamer previously overwrote silently. Array-involved
  redeclarations are unchanged (same-size re-clear, size-mismatch
  and variable/array errors still come from the emitter). Pinned
  by the `redeclare_scalar` error fixture.
- File-scope redefinition: a duplicate initialized definition at
  file scope is now a compile error ("redefinition") in every
  scalar/pointer combination, with array-involved duplicates still
  left to the emitter. A bare declarator without an initializer
  is not counted as a definition, so its existing unsupported
  error is preserved, and the tentative-definition question does
  not arise. Pinned by the `redeclare_global` error fixture.
- Division by zero is now defined instead of spinning forever:
  `a / 0` is `0`, `a % 0` is `a`. `scanf` `%d` honors a leading
  `-` (wrapping negation); `+` is ignored, as before. Pinned by
  unit tests and the `io` oracle fixture (wrap-back form).
- `printf %s` pays the 256-arm dispatch once per site instead of
  twice (hand-rolled do-while loop; see decision 0007): the
  `strings` fixture drops from 3,036,947 to 1,639,379 BF bytes
  (-46%), with all other goldens byte-identical.
- Dispatch arms hand-rolled with shared, reclaimed guard cells
  (see decision 0008): `pointers` 2,978,087 -> 2,955,444,
  `strings` 1,639,379 -> 1,615,153, `functions` 824,222 ->
  817,764 BF bytes, with allocated cells unchanged and all other
  goldens byte-identical. A spike without the reclaim grew
  `pointers` by 344K bytes; the dispatch lever is now exhausted
  short of a structural change.
- Strings: string-literal array initializers (sized and unsized
  `char a[] = "…"`, bytes plus zero fill), string literals in
  value position decaying to anonymous NUL-terminated storage,
  `printf` `%s` over the dispatch, and `sizeof` of a literal.
  Pinned by the `strings` oracle fixture and `strings_*`
  compile-error fixtures.
- `scanf %s`: whitespace-delimited string input, truncating to
  `len - 1` characters plus NUL into arrays (and constant
  element addresses), unbounded through pointers and variable
  element addresses; end of input before a token stores
  nothing. The terminating whitespace is consumed, not pushed
  back (a defined deviation; `%c` after `%s` reads the next
  character). Pinned by the `scanfstr` oracle fixture and the
  `scanf_string_not_address` error fixture.
- Function prototypes (forward declarations, file-scope only,
  optional parameter names): repeated compatible prototypes are
  allowed, conflicting signatures are compile errors, and
  calling a prototyped-but-undefined function is a compile
  error. Pinned by the `prototypes` oracle fixture and the
  `function_prototype_*` error fixtures.
- CLI: `check` validates source by compiling it fully and emits
  nothing (exit code 0 means valid); `compile -s`/`--stats`
  prints one `BF bytes: N, cells: M` line to stderr, keeping
  stdout pure Brainfuck. Pinned by CLI golden cases and unit
  tests.

## Phase 5 — Performance

- Execution perf harness (`freud-bench`): in-process timing of
  transpilation and execution for the corpus plus synthetic runner
  microbenchmarks; results published in `docs/perf.md` and
  refreshed with codegen changes.
- Runner optimization: jump table for brackets, buffered I/O.
- Peephole post-pass evaluated by measurement and rejected (zero
  occurrences in the golden corpus).
- Loop-based constant construction in `set_value`: large constants
  build as `a * b + r` over a scratch cell when smaller in emitted
  bytes and within 25% of direct in estimated steps; all 60+
  `+` runs in the golden corpus are gone, `hello_world` halves,
  corpus totals barely move (helpers dominate). Goldens
  regenerated; `docs/perf.md` refreshed.

## Phase 3 — Transpile-first reframe

- `compile` is the default command; `run`/`format` are auxiliary.
  Positional files, errors on stderr, exit code 1. Docs and CLI
  describe a C subset, not a Brainfuck toolset (decision 0001).

## Phase 2 — Rust migration

- Tokenizer, hand-written parser, codegen, runner, and formatter
  ported to Rust (`freud-core`), guarded by byte-compare goldens
  and the g++-oracle differential suite.
- C++ implementation and ANTLR toolchain retired after the Rust
  port matched it byte-for-byte.

## Phase 4 — Stack decision (pulled forward)

- Spikes: hand-written parser (feasible, better diagnostics),
  tree-sitter-c (weaker diagnostics), C++/Rust/Zig tokenizer
  differential (byte-identical). Verdict (decision 0005, amended):
  Rust core, hand-written parser, C ABI distribution floor,
  `wasm-pack` for web. Revisit trigger: subset approaching full C.

## Phases 0–1 — Hardening to the thesis spec

- Baseline record, semantics doc, and language-identity decision
  (honest C subset, decision 0001).
- Division/modulo differential tests; `switch` implemented;
  end-of-input pinned (cell unchanged); error paths given exit
  codes. The thesis test spec is the hardening scope (decision
  0002).

## Pre-history (university project, 2018–2019)

- C++ / ANTLR transpiler for a C subset to Brainfuck, with a
  Brainfuck runner and formatter, per the Master's thesis
  (Serbian) and the accompanying blog post. Retired in Phase 2;
  behavior preserved by the differential corpus.
