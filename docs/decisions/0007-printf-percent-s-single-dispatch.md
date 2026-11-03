# 0007 — `printf %s` pays the dispatch once per site

Date: 2026-11-03. Status: accepted.

## Context

`print_as_string` (`printf %s`) loaded each character through the
256-arm subtractive dispatch inside a `perform_while` condition.
`perform_while` evaluates its condition twice during emission, so
every `%s` site paid the dispatch twice. The `strings` fixture
(eight `%s` sites) measured 3,036,947 BF bytes, the largest in the
corpus, while `pointers` (2,978,087) paid one dispatch per
dereference site. The spike (`spike/dispatch`, branch-scoped
measurement workflow) measured the alternatives before any change
landed on `master`.

## Decision

Hand-roll the `%s` loop do-while style in `print_as_string`: the
character cell starts nonzero so the loop is entered once, each
pass loads one character through the dispatch and prints/advances
only when it is nonzero, and the loop re-tests the character cell
itself. The dispatch is emitted once per site; the runtime load
count per string is unchanged (one load per character plus the
terminating load).

Alternatives considered and rejected, with reasons:

- A sequential tape walk (advance the data pointer along the
  string instead of dispatching) would break the emitter's static
  pointer-tracking invariant: all pointer motion is compile-time,
  and runtime addressing exists only as dispatch. Not viable
  without a new untracked-motion primitive.
- A shared dispatch subroutine is not expressible: Brainfuck has
  no calls or return addresses, and a flag-and-trampoline
  restructure of the whole code generator is out of proportion
  to the saving.
- A lighter per-arm branch (custom one-sided test instead of
  `perform_if_else`) was not attempted: the saving is bounded by
  the branch machinery, uncertain, and the pointer juggling is
  easy to get wrong. It stays available if `pointers` needs
  another lever; the `%s` win does not depend on it.

## Consequences

- Measured on the spike branch (unit tests and the g++ oracle
  green): `strings` 3,036,947 to 1,639,379 bytes (-46%);
  `pointers`, `functions`, `printnum`, `printfmt` byte-identical.
- Only the `strings` golden changes; it is re-snapshotted with the
  landing, and perf/size-history record the new milestone.
- `*p` loads/stores still pay one full dispatch per site; no
  change to their emission or semantics.
