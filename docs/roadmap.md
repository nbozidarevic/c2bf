# Roadmap

Evolve the C-to-Brainfuck transpiler into a tested, documented, portable
toolchain. Current position: Phases 0–5, 7, and 8 complete; Phase 6
(language expansion) remains open-ended, one construct at a time. The
project was renamed to `c2bf` and tagged `v0.1.0`, then `v0.2.0`,
`v0.3.0`, `v0.3.1`, `v0.4.0`, and `v0.4.1`; npm publishing is declined (see
`docs/decisions/0006-distribution-adapters.md`). Versioning rules
are in `docs/versioning.md`.

Phase order is deliberate: harden, then decide the stack, then modernize
the build (no point polishing a build the stack decision might replace),
then optimize, extend, and distribute.

- [x] Phase 0 — Ground truth: semantics doc, language identity (C subset),
      baseline record.
- [x] Phase 1 — Harden to the thesis test spec: differential tests per
      construct, runner memory model, switch/break, errors with exit codes.
- [x] Phase 4 — Stack decision (pulled forward): spikes + records 0003
      (hand parser: feasible), 0004 (tree-sitter: weaker diagnostics),
      core-lang differential (Rust/Zig/C++ byte-identical) → 0005 verdict:
      Rust core, hand-written parser.
- [x] Phase 2 — Rust migration (tokenizer, parser, codegen, runner,
      formatter, unified CLI) + golden CI; C++ retired.
- [x] Phase 3 — Reframe CLI/docs to transpile-first (compile is the
      default command; `run`/`format` auxiliary; errors on stderr).
- [x] Phase 5 — Performance: harness, published results, runner
      optimization; peephole rejected by measurement.
- [ ] Phase 6 — Language expansion (thesis order).
- [x] Phase 7 — Native-first distribution adapters; web adapter gated.
      7a C ABI (`rs/ffi/`), 7b WASM/demo (`rs/wasm/`, `demo/`) done,
      record 0006; UniFFI/JNI deferred, named; Pages auto-deploys
      on relevant pushes (manual dispatch retained);
      npm publishing declined.
- [x] Phase 8 — Rename, final docs, changelog, tag.
      Renamed `freud` → `c2bf`, tagged `v0.1.0`; `v0.2.0` followed
      for the CLI/storage/typedef/enum batch (see `CHANGELOG.md`).

## Operating loop (CI-primary)

Local binary execution is blocked by Santa, so CI is the verification
authority: implement locally, compile-check locally (compiling needs no
allows), commit, push; CI builds and runs the suite on Ubuntu + macOS.
Spike work happens on `spike/*` branches with branch-scoped workflows;
spikes run sequentially (stop when the decision is clear) and conclude
with a record under `docs/decisions/`.
