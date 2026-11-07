# 0017 — Assignment expressions yield the stored value (spike verdict: accept)

Date: 2026-11-07. Status: accepted.

## Context

An assignment expression yielded its target cell, not a value
copy: two assignments to the same variable inside one
operator's operands were both read after both assignments
(`(x = 1) + (x = 2)` computed `2 + 2`), unlike C, which
captures each assigned value. The fixture corpus avoided
the shape, so the deviation was documented in
`docs/semantics.md` rather than fixed. The fix is one
capture: `store_value` duplicates the value into a fresh
cell before storing, and returns the copy, for every store
destination (static cell, array range, and dispatch).

## Spike measurement

Measured on `spike/assign-copy` (commit `3d5eb73`) against
the committed goldens at `ad17991`, with the oracle suite
and unit tests green on the spike:

- Total BF bytes: 34,015,355 -> 35,651,049 (+1,635,694,
  +4.81%). 24 of 53 fixtures change; 29 are byte-identical.
- Worst real fixtures: `ptrptr` +8.8% (+1,239,822 bytes),
  `pointers` +9.1% (+265,147), `multidim` +2.5%. The worst
  relative growth is the 32-byte `micro_order_assign`
  probe (+121.9%, +39 bytes absolute), which is immaterial.
- Allocated cells: +182 across 17 fixtures (worst:
  `pointers` +48, `ptrptr` +32, `arrays` +22). No fixture
  crosses a new addressability boundary because of the
  capture alone: the growth is temporaries inside dispatch
  regions, and the 500-seed sweep re-validates the skip cap
  after landing.

## Decision: accept

Bar, set before measuring: accept if total growth is at
most 25% and no real fixture more than doubles. The spike
is well inside it, and the fix is complete for every
assignment form (plain, compound, increment, and
decrement paths all flow through `store_value`). A
capture only for static-cell targets was rejected: the
right-hand-side cell can itself be a variable later
reassigned in the same expression (`(a[0] = x) + (x = 5)`),
so a partial fix would leave the same class of bug in
range and dispatch stores. Skipping the capture for fresh
temporaries was likewise rejected: cell provenance is not
tracked, and address arithmetic can in principle land on a
temporary index, so the saving is not provably safe.

## Consequences

Goldens for the 24 changed fixtures re-snapshot (the
one-dimensional byte-identity of the 0016 batch no longer
holds for assignment-heavy programs), and the
size-history/perf record refreshes with a new milestone.
The `spike/assign-copy` branch is deleted after the
cherry-pick; the numbers above are the record.
