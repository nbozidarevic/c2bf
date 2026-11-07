# 0018 — Hardening: typedef shadowing, undefined externs, fuzzer coverage

Date: 2026-11-07. Status: accepted.

## Typedef shadowing

A typedef name is now hidden in any scope where a variable,
parameter, or `for` declarator of the same name is declared:
using it as a specifier there is a parse error, and it is
visible again when the scope ends. The check reuses the
parser's existing scope stack (the same tracking that hides
enumerators), in `resolve_spec`, so it costs no new state.
The one corner it does not cover is shadowing inside the
parameter list itself (`int f(int T, T x)`), where parameter
names are not yet in scope while the list parses; that
shape stays accepted, and no fixture or test depends on
it. The previously documented deviation ("the alias always
wins") is removed from `docs/semantics.md`.

## Undefined externs

An `extern`-only declarator is now kept in the AST flagged
`is_extern` instead of dropped. `compile_program`
computes the undefined set (declared only as `extern`,
defined nowhere in the unit, order-independent) and every
name-resolution path rejects a use with "undefined
reference". A definition before or after the declaration
satisfies it; `sizeof` operands stay unevaluated and
uncalled functions never expand, so neither counts as a
use. The unit test that pinned the old zero-cell behavior
was updated to the new errors, with the reasoning recorded
there: the old behavior was the documented deviation being
fixed, not a contract.

## Fuzzer coverage, and what stays out

The generator now covers 3D arrays, `**` helpers, one
file-scope typedef per program, and `scanf` in exactly
one of three shapes (numeric, one fitting `%s` word, one
`%c` letter). Negative `scanf` tokens stay ungenerated:
a negative token's C value and byte value differ, and
tracking both would need dual intervals through every
expression the value flows into; the defined wrap-back
behavior stays covered by the `io` fixture. The sweep
below is the evidence that the new shapes agree.

## Sweep and budget

The 500-seed sweep on the final hardening tree passed 468
seeds, skipped 32 (6.4%), and mismatched none. No budget
tightening was needed: the skip rate fell from 40 (8.0%)
after 0016 despite the assignment capture's cell cost,
because the generator reshuffle changed which programs the
seeds produce. Headroom under the 10% cap is 18 seeds.
Tightening (a lower statement budget or lower new-shape
probabilities) stays the response if a future sweep
exceeds the cap; it is not applied pre-emptively.
