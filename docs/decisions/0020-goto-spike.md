# 0020 — Forward-only `goto` by per-label flags (spike verdict)

Date: 2026-11-09. Status: accepted (restricted subset; landing
pending the conditions below). Spike branch `spike/goto` measured
and deleted, never merged.

## Context

Emission is single-pass and linear: Brainfuck has no jump to an
arbitrary address, and control flow is built from BF loops plus
flag-guarded skipping (`break`/`continue`/`return`). Labels
already parsed (`goto-label`) but compiled as no-ops, and `goto`
was not a keyword. A general `goto` would need a program-counter
state machine over the whole function body; the spike tested
whether the existing flag machinery covers a useful subset
instead.

## Decision: flags, forward-only, no jumping in

A `goto` sets its label's flag cell (one per label, allocated
zero before the function body). Every statement or declaration
compiled while a flag is pending is guarded on that flag being
clear, composing with the break/continue/return guards; the
label statement clears its own flag and compiles its body
unguarded by it. Loop conditions and `for` updates gate on flags
for jumps out of the loop (like `break`); a `do...while` is
additionally guarded as a whole, since its first body emission
is unconditional. A `goto` inside a loop whose label is inside
the same loop body is not condition-gated: the next iteration
re-enters the body, the pre-label statements are guarded, and
the label clears the flag.

Validation (per function, before emission) rejects, each with
its own message: duplicate labels, undefined labels, backward
jumps, jumps into a loop, jumps into an `if` arm the `goto` is
outside of, and any `goto` or label inside a `switch`. Jumping
over a declaration leaves its cell zero (the initializer is
guarded); that shape is an error under the g++ oracle, so it is
pinned by a unit test, not a fixture.

## Measurements (spike run 42704788511, all green)

The spike (`857125b`, `1c7bc22`, `8b4eabd`, `1244a06` on the
deleted branch) passed the oracle suite, 50 fuzz seeds, all six
error fixtures, and left every goto-free program byte-identical
to its `v0.5.0` golden. Against hand-structured equivalents in
the same bench run:

| program | goto BF bytes | structured | goto cells | structured | goto run (ms) | structured |
|---|---|---|---|---|---|---|
| fwd | 28219 | 28040 (+0.6%) | 67 | 70 | 0.060 | 0.060 |
| cleanup | 49605 | 48650 (+2.0%) | 91 | 116 | 2.114 | 2.135 |
| loop | 38600 | 38456 (+0.4%) | 76 | 91 | 1.548 | 1.614 |
| for | 33200 | 32948 (+0.8%) | 79 | 79 | 0.225 | 0.222 |
| dowhile | 35890 | 33653 (+6.7%) | 80 | 79 | 6.225 | 1.312 |

Costs are small except the `dowhile` runtime outlier (+375% in
this one run, on a shared runner; the guard shape is
straight-line code, so no cause is claimed). Profile it before
landing: if it reproduces, the double guard around `do...while`
is the first suspect. Cells are usually fewer than the
structured equivalent, because one flag replaces duplicated
branch bodies.

## Incidental finding (not a goto bug, separate follow-up)

The `%d` scanf reader loops while the input cell holds a digit,
and `,` leaves the cell unchanged at EOF, so numeric input that
ends exactly at EOF (no trailing newline) hangs forever. It
hits goto and structured programs alike, and every existing
fixture dodges it with a trailing newline. It needs its own
fix, tests, and golden refresh; it must not ride along with
the goto landing.

## Landing conditions

Cherry-pick the spike's code commits only (not its workflow or
debug commits), then: extend the differential fuzzer with
forward gotos, add golden snapshots for the new fixtures,
document the subset in `docs/semantics.md`, refresh
`docs/perf.md` and `docs/size-history.md`, and bump the minor
version (new backwards-compatible capability, per
`docs/versioning.md`). Deferred: backward `goto` (needs the
state-machine redesign), jumps into loops/branches/`switch`,
and any richer label semantics.
