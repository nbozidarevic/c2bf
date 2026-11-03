# 0008 — Hand-rolled dispatch arms with shared guard cells

Date: 2026-11-03. Status: accepted.

## Context

Every `*p` load/store and variable array index pays a 256-arm
subtractive dispatch (`load_from_cells` / `store_to_cells` in
`rs/src/codegen.rs`). Each arm was a `perform_if_else` guard (which
duplicates the counter) plus a `copy_value` body. `pointers`
measured 2,978,087 BF bytes on 438 cells, 29% of the corpus, and a
single dereference (`order_deref` probe) 351,123 bytes. The spike
(`spike/deref-dispatch`, branch-scoped measurement workflow)
measured a lighter arm before any change landed on `master`.

## Decision

Hand-roll each dispatch arm, keeping `perform_if_else`'s branch
shape but sharing one set of guard cells (`if_value`,
`else_value`, copy helper) across all arms of a dispatch, and
emitting the copy loop directly instead of through `copy_value`
(the fired arm clears the shared helper first; non-firing arms
never touch it). The shared cells are reclaimed after the
dispatch via the existing checkpoint/release idiom, so later code
keeps its layout. A consuming counter test (move the count into a
test cell, restore it in a loop, run the body on fallthrough) was
designed and discarded during the spike: straight-line emission
after such a loop cannot tell the zero case from the restored
case without a flag, which costs back the saving.

V1 of the spike omitted the reclaim: `pointers` grew by 344,445
bytes and 53 cells, because every dispatch site left three cells
allocated and shifted all later pointer motion. V2 (reclaim)
measures a strict improvement with no cell cost.

Alternatives considered and rejected, with reasons:

- A fewer-step search (binary or nibble dispatch) cannot beat the
  per-arm copy: each of the 256 target cells still needs its own
  copy site mentioning its index, or a runtime tape walk. The
  hierarchy would add compare machinery on top of the copies.
- A sequential tape walk (advance the data pointer to the target
  instead of dispatching) would break the emitter's static
  pointer-tracking invariant, as in 0007. Not viable without a new
  untracked-motion primitive.
- A shared dispatch subroutine is not expressible: Brainfuck has
  no calls or return addresses, as in 0007.
- Reducing the copy's six pointer traversals per arm (the
  dominant cost, proportional to the result–target distance)
  would need the walk or the subroutine above; the guard was
  never the bottleneck, which is why this change saves ~1%
  rather than an order of magnitude. The dispatch lever is now
  exhausted short of a structural change.

## Consequences

- Measured on the spike branch (unit tests, differential suite,
  and g++ oracle green): `pointers` 2,978,087 to 2,955,444
  (-22,643), `strings` 1,639,379 to 1,615,153 (-23,226),
  `functions` 824,222 to 817,764 (-6,458), `order_deref`
  351,123 to 348,815 (-2,308), `order_index` 423 to 415 (-8);
  the other 38 fixtures byte-identical; allocated cells and
  runtime footprint unchanged everywhere.
- The five changed goldens are re-snapshotted with the landing,
  and perf/size-history record the new milestone.
- Known limitation, pre-existing and unchanged: dispatch scratch
  cells are fresh temporaries, which the reclaim floor keeps above
  all live variable/array cells (see `Memory::release_temps`), so
  a scratch cell is never a live target in real compilation.
  Addressing a scratch index directly (e.g. by pointer
  arithmetic past a variable) was already outside the model the
  dispatch assumes; the hand-rolled arms keep the same exposure.
  The `cell == helper` fallback arms in the new code exist only
  as a guard against a non-terminating copy loop in that case.
