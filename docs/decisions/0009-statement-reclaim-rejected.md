# 0009 — Statement-level temporary reclaim rejected

Date: 2026-11-04. Status: rejected (spike measured, not landed).

## Context

Scoped reclaim already covers `perform_if_else` / `perform_while`,
dispatch load/store, the `%s` loop, and function expansion
(`Memory::release_temps` in `rs/src/memory.rs`). Straight-line
statement temporaries were never reclaimed: cell indices grew
monotonically through a function body, so later statements sat
farther up the tape. The spike (`spike/stmt-reclaim`,
branch-scoped measurement workflow) added a checkpoint/release
around every block item, switch case item, and file-scope
declaration in `rs/src/compile.rs`, on the argument that values
only flow out of a statement through variables (protected by the
reclaim floor) or through cells allocated before the statement
(loop/switch/return flags, the switch discriminant), which sit
below every checkpoint taken inside it.

## Decision

Rejected. The spike passed all unit tests and the differential /
g++ oracle suite, and collapsed the allocated-cell footprint
(`pointers` high-water 438 to 97, `bitwise` 632 to 93, runtime
cells-touched down comparably), but grew total emitted code:
11,476,921 bytes on `master` to 11,885,644 on the spike
(+408,723, +3.6% over 36 fixtures). 27 fixtures shrank; 9 grew,
and the three grows outweighed all wins combined: `strings`
1,616,153 to 1,957,013 (+340,860, +21%), `scanfstr` 1,386,140
to 1,655,689 (+269,549, +19%), `prototypes` 371,582 to 409,823
(+38,241, +10%). The largest single win was `pointers`
(-112,290). Runtime signal from the quick bench was mixed
(`strings` slower, `scanfstr` marginally faster) and, per
`docs/perf.md`, cross-run timing is not conclusive; the byte
total is exact and decides it.

The growth mechanism was not isolated: per-arm dispatch travel
and `set_value` loop plans both favor the tighter layout the
spike produces, so the regression must come from a subtler
interaction (candidate: helper/scratch aliasing fallbacks firing
more often at low indices). Bisecting it (block-only vs
switch/file-scope wraps, then per-region byte diffs) was left
undone because the net result already fails the bar: a footprint
win that costs a fifth of the string fixtures' code size is not
a win for a project whose tracked metrics are BF bytes and
execution speed.

## Consequences

- Nothing landed on `master`; the spike branch was deleted with
  its measurement workflow. No golden, perf, or size-history
  refresh was needed.
- The footprint observation stands as evidence for the
  cell-layout spike: keeping indices low is valuable, but any
  layout change must be measured fixture-by-fixture, with
  `strings`, `scanfstr`, and `prototypes` as canaries — a net
  total win that regresses any of them by >1% needs the
  mechanism understood before landing.
- `Memory::release_temps` and its existing call sites are
  unchanged: reclaim stays scoped to constructs whose
  callers guarantee no live value in reclaimed cells.
