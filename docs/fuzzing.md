# Differential fuzzing

`tests/fuzz.py` generates random C-subset programs, compiles each
with g++ (the oracle, exactly as `tests/test` does) and with
`c2bf`, runs both, and compares stdout byte-for-byte. It is the
check that hand-written fixtures cannot be: it exercises
combinations — expression shapes, control flow, arrays,
pointers, helper functions, based `scanf` conversions — that no
one would write by hand, against the compiler the subset is
measured against.

## Running it

From `tests/` (a Rust toolchain and g++ are required):

```sh
python3 fuzz.py --count 50
python3 fuzz.py --seeds 1234 --verbose
python3 fuzz.py --count 500 --seed-base 1000 --continue-on-failure
```

CI runs 50 seeds (from seed 0) on every push in the `fuzz` job.
Bigger sweeps run on demand from the Actions tab via the
`fuzz-sweep` workflow, which takes a seed count and a seed base;
its log prints the failing seed and program. Every seed is
deterministic: the same seed always generates the same program,
so a failure reproduces with `--seeds <seed>`.

A failure exits 1 and prints the seed, the program, the stdin,
and the output diff. A generator problem — g++ rejecting a
generated program, or the oracle exiting nonzero — exits 2 and
says so: that is a bug in the generator's contract, not in
`c2bf`. Individual runs time out after 15 seconds; a timeout is
reported as a fuzz failure, since generated loops are bounded
and must terminate.

A seed the compiler rejects only with "address is outside
the addressable range" is outside the fuzzed subset: it
exceeded the 256 addressable cells (see
`docs/semantics.md`), which no generator bound separates
cleanly from fitting programs (statement counts of failing
and passing seeds overlap, and a fitted surface-feature
estimator scores a passing seed above a failing one). The
driver reports such seeds as skips (`FUZZ SKIP`), counts
them in the summary, and fails the run if skips exceed 10%
of seeds (any skip fails a single-seed run) — so an
allocation regression that pushes more programs over the
limit fails loudly instead of hiding. Every other rejection
and every output mismatch is a failure. The statement
budget below reduces the skip rate but does not eliminate
it: the 500-seed sweep after the 0016 generator extensions
passed 460 seeds, skipped 40 (8.0%), and mismatched none
(the previous sweep, before those extensions, passed 476
and skipped 24).

## Generator guarantees

The generator only emits programs on which C `int` arithmetic
and the byte model provably agree, so any mismatch is a real
bug. The rules, enforced with interval tracking (every value
keeps a `[lo, hi]` bound):

- Every value stays in 0..255. Operations that would leave the
  range are retried with different operands or fall back to a
  literal. Division and modulo divisors are nonzero. Shifts
  are checked on the shifted interval, not just the count:
  a left shift whose result would exceed 255 is retried
  ('y' `<<` 3 is 968 in C, 200 here), since the byte model
  agrees with C only inside the range.
- `char` storage only receives 0..127: C converts larger values
  into a `char` implementation-defined (signed-char
  sign-extension on the oracle platform), so the generator caps
  `char` assignments, initializers, and compound assignments,
  and only emits `(char)` casts of values below 128. Braced
  list elements for `char` arrays are literals, `char`
  variables, or `(char)` casts: C++ list-init rejects a
  non-constant `int` expression narrowing into `char`.
- Loops are bounded and terminating: counted `for` loops, and
  `while`/`do...while` loops with a counter that only the loop
  template increments. `continue` is generated only in `for`
  bodies (it would skip a `while` increment). Loop bodies never
  change persistent state — intervals across iterations would
  need a fixpoint, so bodies only print or control flow — and
  loop counters leave scope with their loop.
- Array indices are always in range (constant or bounded
  variable indices). `sizeof` is generated only for `char`
  storage (`sizeof(int)` is 4 in C, 1 here). `while` loops are
  never generated directly under a `switch` case label: the
  counter declaration there is a jump-over-initialization
  error in C++.
- `scanf` appears only as a prologue reading 2–3 numbers into
  fresh `int` variables, with well-formed tokens (decimal, hex,
  `0x`-prefixed, or octal) in 0..200, trailing newline included.
  No negative tokens, no `%s`, and no `%c` after a number: the
  documented `scanf` deviations (terminator consumed, `%i`
  `09` handling) would make the two models legitimately
  disagree there.
- Pointer-to-pointer bindings are fixed at declaration
  (`**pp = &p`) and never rebound or incremented: pointer
  arithmetic advances by one cell here and by `sizeof(int)`
  in C, so only dereference chains are generated. `**pp`
  reads and writes carry the ultimate target's interval
  (the whole-array join for array targets); `(*pp)[k]` and
  `pp[0][k]` carry the array interval with `k` in bounds.
  A dereferenced pointer value is never printed or used as
  a number: addresses differ between the models. About one
  program in ten declares a `**` binding, and a handful per
  300 seeds exercise a chain subscript.
- Multi-dimensional arrays are at most 2D and 3x3, at most
  one per program, filled by flat or nested lists with zero
  fill folded into the interval. Both indices are in range
  (constant or interval-bounded). A row subscript appears
  only as a pointer initializer (`*p = a[r]`), never as a
  printed value or a `%s` argument. `sizeof` of a row is
  generated only for `char` arrays, where element counts
  agree (see the `sizeof` rule above).
- Expressions are pure: no assignments, increments, or calls
  with side effects inside them, so evaluation order cannot
  change the result. Helper functions are pure too (one
  parameter, locals, one return expression).
- `printf` `%hhd` is emitted only with arguments capped at
  127: the conversion narrows to signed `char` in C, so
  larger values print negative there and unsigned here
  (length modifiers are ignored). No other length modifier
  narrows a byte value, so `%hd`, `%ld`, and friends take
  uncapped arguments.
- No declarations inside branches: all names are declared at
  function top level, so the generator's flat interval
  environment stays sound across merges.
- Nesting depth is hard-capped: at depth 0 a statement may
  only assign, increment, break/continue, or print — never
  nest another `if`, loop, or `switch`. Temporaries stay live
  across nesting levels, so unbounded nesting could exceed
  the 256 addressable cells (fuzz seed 5 nested `if`s five
  deep and failed with "address is outside the addressable
  range" even after the reader shrink). The cap is structural
  and sweep-validated, not a proof that every generated
  program fits: the 500-seed sweep is the check, and any new
  address-range failure beyond the skip cap is a
  generator-bound or allocation bug unless a minimal case
  shows a single construct exceeding the limit on its own.
  The underlying compiler limit is documented in
  `docs/semantics.md`.
- `switch` cases are flat: no loop at any depth inside a
  case body (a loop inside an `if` inside a case is barred
  too). A loopy case body plus the switch's match and guard
  cells peaked past 256 (fuzz seed 5 again, after the depth
  cap and after just-in-time matches). Loops elsewhere, and
  switches inside loops, are still generated; loops in
  cases remain covered by the hand-written `switch` fixture.
- Total statements per program are budgeted (`STMT_BUDGET`,
  20 `stmt()` calls including nested ones): nesting
  multiplies statements, and straight-line temporaries
  accumulate across a whole function body with no reclaim
  boundary between top-level statements (statement-level
  reclaim was measured and rejected in
  `docs/decisions/0009-statement-reclaim-rejected.md`), so
  an unbounded count eventually leaves the next free cell
  above 255 and a trailing string literal fails (fuzz seed
  6). Exhausted branches come out empty. Like the depth
  cap, the budget is empirical and sweep-validated, not a
  proof: seeds 0-4 use at most 15 calls, seeds 5-6 used
  29/25. The budget reduces the skip rate but does not
  eliminate it (24 of 500 seeds still exceed the limit and
  are reported as skips); the sweep's checks are zero
  mismatches and skips within cap.

Not covered (yet): `scanf %s`/`%c` input, negative `scanf`
tokens, recursion, function pointers, `goto`, structs, and
floats. Each is either unsupported or needs its own
agreement argument before the generator may emit it.
Three-dimensional arrays, `**` returns and parameters, and
typedef-composed dimensions are supported by the compiler
but not generated: the generator caps dimensions at 2D and
generates no helper signatures beyond one scalar parameter.

## Extending the generator

To add a construct: first write down the agreement argument
(when do C and the byte model provably produce the same
output?), encode it as a generation rule (interval bound,
token shape, or exclusion), add the emission, then run at
least 500 seeds locally or via `fuzz-sweep` before landing. If
a new rule turns out wrong, the fuzzer will report oracle
rejections first — fix the generator, not the oracle
expectation.
