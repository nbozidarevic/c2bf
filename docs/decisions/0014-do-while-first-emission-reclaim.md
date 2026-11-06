# 0014 — Reclaim the do...while first body emission

Date: 2026-11-06. Status: accepted.

## Context

`perform_do_while` emitted its body once unconditionally and
then again inside `perform_while`, with no reclaim between
the two emissions, so the first emission's temporaries
stacked with the loop's own. Fuzz seed 6 (nested
while/do...while, two based readers, a switch in a loop)
failed with "address is outside the addressable range" in
its trailing statements: bisecting showed the prefix through
the `if` block compiled (299 cells high-water) while the
trailing string literals did not — straight-line
accumulation had left the next free cell above 255. The
spike (`spike/do-while-reclaim`, branch-scoped measurement
workflow) prototyped reclaiming the first emission before
anything landed on `master`, and never merged the spike
branch itself: the change was taken onto `master` from the
spike tip.

## Decision

Checkpoint and release around the first body emission in
`perform_do_while`, before `perform_while` re-emits the
body. The re-emission deterministically reallocates the
same indices and every helper re-initializes its temps
before use, while loop `broken`/`continued` flags are
allocated once per loop statement before the body (and the
`continued` flag is cleared at each body execution), and
variables and anonymous string storage are guarded by the
reclaim floor — so no value the second emission needs is
lost. Statement-level reclaim as a whole stays rejected
(0009): this is one construct's double emission, not every
statement.

## Consequences

- Measured on the spike branch (unit tests and differential
  suite green): `dowhile` falls to 126,476 bytes on 147
  cells (from 128,632 on 181), `break` to 154,240 bytes on
  159 cells, `continue` to 207,707 bytes; every other
  fixture is byte-identical and cell-identical to the
  just-in-time-matches state (0013). The three changed `bf`
  goldens are re-snapshotted with the landing.
- The reclaim alone does not bring fuzz seed 6 inside the
  addressable cells: the remaining pressure is straight-line
  accumulation across the whole function body, which
  statement-level reclaim would address only at the byte
  cost measured in 0009. The generator therefore also caps
  total statements per program (see `docs/fuzzing.md`), and
  seed 6's shape is covered by that bound plus the
  hand-written `dowhile` fixture.
