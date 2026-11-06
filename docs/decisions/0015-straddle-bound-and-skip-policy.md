# 0015 — String-literal straddle bound; I/O reclaim rejected again; fuzzer skip policy

Date: 2026-11-06. Status: accepted (straddle bound, skip
policy); rejected (I/O reclaim, second reading).

## Context

Differential fuzzing's 500-seed sweep found one real
compiler bug (seed 65): `printf("%s, %s\n", "gbe", "yxeb")`
printed `yxe\x01c2e\x14\t\x0c\x0c\x0c` instead of `gbe,
yxeb`. A tape dump after the run showed the literal intact
at cells 253-256 with its NUL at 257: `string_value`
checked only the literal's base against the 256 addressable
cells, so a literal straddling cell 255 compiled, and the
print walk wrapped from 255 to cell 0 and read unrelated
tape (the `a1` array contents). Narrowing probes had already
shown the trigger needed a preceding `%s` printf plus the
two-argument final printf — pure layout dependence, which
is why the dump, not code review, settled it.

## Decision: whole-storage bound

`string_value` now requires the whole storage, terminator
included, to fit in the addressable cells, with the same
"address is outside the addressable range" error as the base
check. A straddling literal is a compile error, not silent
corruption — consistent with `&x` past 255 and oversized
arrays. Pinned by `string_literal_straddling_255_errors`
and documented in `docs/semantics.md`.

## Rejected, second reading: reclaim after `printf`/`scanf`

Re-measured on the current tree (0012's shrink, 0013's
just-in-time matches, 0014's do...while reclaim already
landed) on `spike/io-reclaim`: the reclaim fixes seed 65
(the literal fits and prints correctly) and the spike run
is fully green, and it shrinks `formats` by a further
309,524 bytes and `functions` by 36,470 — but the canary
fixtures grow by exactly the 0012 amounts (`strings`
+195,714 bytes, `scanfstr` +216,936, `prototypes` +36,014).
The growth is stable across two very different trees, so it
is the mechanism (reclaim churns cell layout), not the
moment, and the 0010 bar — no fixture grows — fails again.
The lever stays exhausted; statement-level reclaim stays
rejected for the same reason (0009).

## Decision: fuzzer skip policy for the address limit

The sweep also left 23 address-range rejections in 500
seeds at the statement budget of 20. No generator bound
separates them: a fitted weighted estimator (readers,
switches, loops, derefs, literals, printfs, lines) scores a
passing seed 10 at 280 against a failing seed 376 at 110,
and statement counts overlap (failing seeds use 8-28 calls,
passing seeds up to 35), because the limit depends on
compiler-internal allocation order, not surface features.
Lowering the budget is a lottery, and the compiler levers
are exhausted above. The driver therefore treats a
rejection carrying exactly the documented limit message as
a reported skip: printed per seed (`FUZZ SKIP`), counted in
the summary, and capped — the run fails if skips exceed 10%
of seeds (any skip fails a single-seed run), so a future
allocation regression that pushes more programs over the
limit fails loudly instead of hiding. Every other
rejection and every output mismatch still fails hard, and
the golden cell history stays an independent check on
allocation growth. Sweep evidence at the decision: 476/500
pass, 24 skip (4.8%), 0 mismatches after the generator
agreement fixes in `docs/fuzzing.md`.
