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

## Generator guarantees

The generator only emits programs on which C `int` arithmetic
and the byte model provably agree, so any mismatch is a real
bug. The rules, enforced with interval tracking (every value
keeps a `[lo, hi]` bound):

- Every value stays in 0..255. Operations that would leave the
  range are retried with different operands or fall back to a
  literal. Division and modulo divisors are nonzero.
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
- Expressions are pure: no assignments, increments, or calls
  with side effects inside them, so evaluation order cannot
  change the result. Helper functions are pure too (one
  parameter, locals, one return expression).
- No declarations inside branches: all names are declared at
  function top level, so the generator's flat interval
  environment stays sound across merges.

Not covered (yet): `scanf %s`/`%c` input, negative `scanf`
tokens, recursion, function pointers, `goto`, structs,
multi-dimensional arrays, pointer-to-pointer, and floats. Each
is either unsupported or needs its own agreement argument
before the generator may emit it.

## Extending the generator

To add a construct: first write down the agreement argument
(when do C and the byte model provably produce the same
output?), encode it as a generation rule (interval bound,
token shape, or exclusion), add the emission, then run at
least 500 seeds locally or via `fuzz-sweep` before landing. If
a new rule turns out wrong, the fuzzer will report oracle
rejections first — fix the generator, not the oracle
expectation.
