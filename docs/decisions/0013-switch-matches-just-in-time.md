# 0013 — Just-in-time `switch` matches

Date: 2026-11-06. Status: accepted.

## Context

`compile_switch` computed every case's `is_equal` match
upfront and held all match cells live across the whole
switch, plus `any_match`. Fuzz seed 5 (capped nesting, two
switches with loopy cases) still failed with "address is
outside the addressable range" after the 0012 reader shrink,
and bisecting put the failure inside the first switch block.
The spike (`spike/switch-lazy`, branch-scoped measurement
workflow) prototyped just-in-time matches before anything
landed on `master`, and never merged the spike branch itself:
the change was taken onto `master` from the spike tip.

## Decision

When there is at most one `default` and it comes last (the
usual shape, and the only shape the fuzzer emits), compute
each non-default case's match just before its guard and
reclaim the match and gate temporaries after the case's
guard runs. `any_match` accumulates per case into its
pre-loop cell, so the default's gate (which reads it) is
complete by then; `fall`, `broken`, the switch expression,
and the label cells are allocated before the per-case
checkpoint and survive the reclaim, as does everything the
case body flows out through. Any other `default` placement
(one before the last case, or several) keeps the eager shape,
which computes all matches upfront: the gate order for a
non-last default depends on matches not yet computed.

## Consequences

- Measured on the spike branch (unit tests and differential
  suite green; fuzz failed only on seed 5, which the change
  does not fix): `switch` falls to 34,431 bytes on 171 cells
  (from 35,409 on 279), `break` to 154,760 bytes on 167
  cells, `continue` to 208,952 bytes, `enums` to 104,402,
  `functions` to 789,726; every other fixture is
  byte-identical and cell-identical. Only the five changed
  `bf` goldens are re-snapshotted with the landing, together
  with the `formats` golden from 0012. Perf/size-history
  record the new milestone.
- The just-in-time shape does not by itself bring fuzz seed
  5 inside the addressable cells: the remaining pressure is
  case-body peaks (a loop with two `printf`s per case), not
  match overhead. The generator therefore also caps switch
  cases to flat bodies (no loops at any depth inside a
  case), recorded in `docs/fuzzing.md`. A program with a
  `default` before the last case still compiles via the
  eager shape, and that placement is covered by the `switch`
  oracle fixture (default in the middle, with cases after
  it), which passed against the g++ oracle on the spike
  branch alongside the default-last shapes.
