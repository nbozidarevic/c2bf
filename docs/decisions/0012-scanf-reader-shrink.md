# 0012 — Shrink the based `scanf` readers; reject I/O-call reclaim

Date: 2026-11-06. Status: accepted (shrink); rejected (I/O reclaim).

## Context

The differential fuzzer's first real finding was seed 3: a
ten-variable program with one `scanf("%i %d", ...)` failed with
"address is outside the addressable range". Probing in CI (a
temporary workflow, since removed) showed the based reader was
the hog, not the program: one `scanf("%i", ...)` peaked at 502
allocated cells and 270,091 BF bytes, against 79 cells and
16,200 bytes for `%d`, and the `formats` fixture (the based
readers' home) was 3,099,121 bytes on 715 cells. The old
`scan_digit` classified every digit with two-sided range tests
and multiply-based selection (~85 cells per emission) and
`perform_while` re-emitted it three times per loop (prologue,
body, back edge), six times per reader. With a `switch` on top,
live cells crossed 256 at an address-taking site. The spike
(`spike/scanf-shrink`, branch-scoped measurement workflow)
prototyped the shrink before anything landed on `master`, and
never merged the spike branch itself: the shrink was taken
onto `master` from the spike's fixed state.

## Decision

Rewrite the based reader (`read_as_number_base` in
`rs/src/codegen.rs`) only; the decimal reader's emission is
unchanged:

- `scan_digit_into` classifies into caller-owned
  `value`/`valid` cells, reusing them across rescans.
  Classification uses wrapping subtraction and one-sided
  bounds (a byte below `'0'` subtracts to 208..255, which
  fails `< 10`, and likewise for the letter ranges), and the
  value is selected with guarded copies (the digit classes are
  disjoint) instead of multiplies.
- Both loops are hand-rolled like `print_as_string`: each pass
  rescans into the same cells and re-tests the gate cell
  itself, so the classification is emitted twice per loop
  instead of three times. Classification temporaries are
  reclaimed after each loop, and the `%i` prefix check's
  temporaries are reclaimed after it.
- The decimal reader, the `scanf %s` readers, and all other
  emission are untouched.

One spike iteration is recorded because it hung: the first
shape tested the digit loop's *negated* gate, so the body ran
exactly when no digit remained and spun on the terminator
(unit tests hung in CI). The digit loop tests its `go` cell
itself, recomputed into the same cell each pass; only the
skip loop tests a negation.

## Consequences

- Measured on the spike branch (unit tests, differential
  suite, and g++ oracle green): one `%i` read falls to 171
  cells and 73,489 bytes (-66% cells, -73% bytes); `%o` and
  `%x` match it. `formats` falls to 1,772,157 bytes on 625
  cells (-1,326,964 bytes, -42.8%; -90 cells). Every other
  fixture is byte-identical and cell-identical to `master`
  (checked against the committed goldens and the `72a356f`
  cell counts), so only the `formats` goldens are
  re-snapshotted with the landing. Perf/size-history record
  the new milestone.
- The shrink is covered by the existing `io_formats_compile_and_run`
  unit test (all `%o`/`%x`/`%i` edges, including `0x`
  prefixes, `09`, signs, and uppercase letters), the `formats`
  oracle fixture, and the fuzzer (seeds 0-4 pass, including
  seed 3). The shrink does not fix every oversized program:
  fuzz seed 5 still fails (identically with `%d`), diagnosed
  as gradual accumulation across a `switch` and nested loops
  with many string literals, not reader pressure; the
  generator caps nesting depth to stay inside the documented
  256-cell addressable limit (see `docs/fuzzing.md`), and the
  limit itself is recorded in `docs/semantics.md` as a known
  limitation. Deep programs that exceed it still fail with
  the same error, by design.

## Rejected: reclaim after `printf`/`scanf` calls

A second spike commit on the same branch reclaimed
temporaries after each I/O call's emission (`compile_call`
returns the dummy cell `0` and writes only through
pre-existing cells, so the reclaim is sound). It moved the
seed-5 failure one block later (the `switch` block compiled)
but grew several fixtures substantially — `strings` +195,714
bytes, `scanfstr` +216,936, `prototypes` +36,014 — the
statement-reclaim pattern of 0009 (reclaim changes cell
layout, and `set_value` loop plans price the new layout
worse). The bar from 0010 is no fixture grows, so the reclaim
was rejected and never cherry-picked; only the reader shrink
landed. The lever is exhausted short of a layout-neutral
reclaim design.
