# Changelog

All notable changes to this project. Crate versions were `0.0.0`
until the rename to `c2bf` (below); they are `0.1.0` from that
point, matching tag `v0.1.0`. Entries below are grouped by the
roadmap phases that produced them.

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
