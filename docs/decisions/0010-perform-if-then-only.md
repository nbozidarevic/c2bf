# 0010 — Then-only `perform_if` for empty else branches

Date: 2026-11-04. Status: accepted.

## Context

Every conditional in the emitter went through `perform_if_else`,
which emits both a then-loop and an else-loop (with its own
`else_value` cell), even when the else branch was the empty
closure `|_| Ok(())`. Empty-else sites were pervasive: the
logical/comparison helpers (`is_equal`, `logical_and`,
`logical_or`, `less_than`), `divide_values`, `accumulate_bit`,
the `bitwise_fold` early-exit test, the `%s` / `scanf` guards,
and the break/continue/return statement guards in
`rs/src/compile.rs`. The else loop in those sites only zeroed a
dead temporary. The spike (`spike/dead-code`, branch-scoped
measurement workflow) measured a then-only form before any
change landed on `master`.

## Decision

Add `Emitter::perform_if`: the `perform_if_else` guard shape
(duplicate the condition, loop on the copy, reclaim afterwards)
without the else loop or the `else_value` cell, and convert
every empty-else call site to it. Sites with a non-empty else
(`negate`, `combine_xor`, number printing, the `scanf %s`
inverted branches, `if`/`else` and ternary statements, switch
cases) keep `perform_if_else`. The pointer ends on the guard
cell instead of the else cell afterwards; the emitter tracks
the pointer through `move_pointer`, and all callers position
explicitly, so no call site changes beyond the call itself.

## Consequences

- Measured on the spike branch (unit tests, differential suite,
  and g++ oracle green): corpus total 11,476,921 to 10,969,857
  BF bytes (-507,064, -4.4%); every one of the 36 fixtures
  shrank or stayed equal (largest: `bitwise` -46,040,
  `pointers` -37,924, `printnum` -36,192, `cmp` -26,654;
  smallest: `hello_world` unchanged); allocated cells down by
  2 on most fixtures (the saved `else_value` cell).
- All changed goldens are re-snapshotted with the landing, and
  perf/size-history record the new milestone.
- `perform_if` is covered by the `perform_if_runs_then_only_when_nonzero`
  unit test (0/1/255 conditions); the converted helpers keep
  their existing exhaustive unit tests and oracle fixtures.
