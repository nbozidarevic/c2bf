# Semantics

Ground-truth behavior for the transpiler, taken from the Master's thesis
(`Varijacije`, `Okruženje`, `Sintaksa`, `Testiranje interpretera` chapters).
Where the implementation deviates from the thesis, the deviation is listed as
fix-or-pin work for Phase 1.

## Tape and cells

- Cells are 1 byte, with wrapping arithmetic ("obmotavanje"): `255 + 1 → 0`,
  `0 − 1 → 255`.
- Unary `-` and `~` wrap like all arithmetic: `-5` is `251`, `~5` is
  `250`. Printed with `%d` these differ from C `int` values (`-5`,
  `-6`); only wrap-back uses (e.g. `-5 + 5`) agree with the g++
  oracle, as pinned by the `unary` differential fixture.
- Casts are no-ops: `(int)`, `(char)`, and multi-word spellings do
  not change the value, since every value is already a byte
  (so `(char)300` is `44`, from the literal's own truncation).
  `(char)` of a value at or above 128 still differs from g++ on
  signed-`char` platforms (`(char)250` prints `-6` there, `250`
  here); see the signedness deviation below.
- `sizeof` is always `1`, since every type is one byte here. This
  differs from C for `int` (4) and for expressions of `int` type;
  only `char`-typed uses agree with the g++ oracle, as pinned by
  the `sizeof` fixture. The operand is unevaluated, as in C.
- Bitwise `& | ^` operate on bytes bit by bit. `<<` doubles and
  `>>` halves (logical, unsigned), once per unit of the count;
  counts of eight or more give zero. Shift counts at or above the
  C width are undefined behavior there and are defined here as
  zero, so oracle fixtures keep counts below eight. A shift whose
  value grows past 255 wraps like all arithmetic, so `%d` of it
  differs from C `int` arithmetic (`128 << 1` prints `256` there,
  `0` here); the fixture uses wrap-back forms for those cases.
  Operands evaluate right-to-left, like other non-arithmetic
  operators; the `bitwise` fixture's double-side-effect lines
  pin this against g++.
- An assignment expression yields its target cell, not a value
  copy, so two assignments to the *same* variable inside one
  operator's operands are both read after both assignments
  (unlike C, which captures each assigned value). The fixture
  avoids that shape; distinct variables in the operands are
  unaffected.
- Compound assignment (`op=`) desugars to `x = x op y` with the
  right-hand side evaluated first, as for the other compound
  forms; `&= |= ^= <<= >>=` use the bitwise/shift helpers.
- Arrays: `char a[N]` reserves `N` consecutive cells, named by
  the array and cleared at the declaration, so elements start at
  zero (C leaves them indeterminate). The size must be a positive
  integer literal, initializers and multi-dimensional arrays are
  rejected, and an array must be declared before use. Only
  constant indices are supported so far; a constant index outside
  `0..N-1` is a compile error, and a variable index is rejected.
  `sizeof` an array is its element count (unlike the always-`1`
  rule for other operands). Variable indices are also supported:
  `a[i]` loads through a dispatch over the array's range (offsets
  `0..255` only), and an out-of-range variable index loads zero
  and stores nowhere — defined here, undefined in C, so the
  fallback is pinned by unit tests, not by the oracle.
- Pointers: there are no pointer types; a pointer is a byte
  holding a tape index. `&x` is the index of `x`'s cell, `*p`
  loads or stores the cell at the index `p` holds (a dispatch
  over all 256 byte addresses), a bare array name decays to
  `&a[0]`, and pointer arithmetic and comparison are ordinary
  byte arithmetic (wrapping). Only the first 256 tape cells are
  addressable: taking the address of storage at cell 256 or
  above is a compile error, while static-index array access
  works anywhere. The null pointer is address `0` (cell `0`).
  `scanf` destinations may be variables, `&`-expressions, or
  bare pointers, and store through the address. For `=`, a
  subscript's index expression is evaluated before the
  right-hand side; for compound assignment it is evaluated
  after it, as for the other compound forms.
- Only `char` variables are supported; anything wider needs multi-cell
  representation (deferred to language expansion).
- Integer literals may be decimal, octal, hex, or binary, with `u`/`l`
  suffixes; values truncate mod 256 like all arithmetic, and 8/9 in an
  octal literal is an error, as in g++.
- Type specifiers may be multi-word (`unsigned int`, `const char`,
  `long`, `signed`, `long long`); every spelling denotes one byte.
  Qualifier words stay identifiers outside specifier position, so a
  variable may still be named `long`.
- The thesis specifies addressable memory for any integer index (its reference
  interpreter uses `map<int, char>`), i.e. negative indices are legal.
- The original Brainfuck spec defines a minimum of 30,000 cells; transpiled
  programs need many temporaries, so in practice far more are required.

## Execution

- All cells start at `0`; the pointer starts at cell `0`.
- `[` skips forward to the matching `]` when the current cell is `0`;
  `]` jumps back to the matching `[` (unconditionally — the cell is re-tested
  at `[`, so loop-exit means "current cell is `0`", not "`> 0`").
- `,` reads one character as its ASCII value; `.` writes the current cell as
  the character with that ASCII value. All other characters in BF source are
  ignored (comments must avoid `.` and `,`).

## Known implementation deviations (Phase 1 work)

1. `BrainfuckRunner` uses a fixed `char memory[65536]` with `pointer = 0`:
   negative indices are out-of-bounds (spec: legal), indices past 65535 are
   out-of-bounds (spec: legal).
2. `char` signedness is platform-defined; wrap behavior at 127/128 therefore
   differs from the specified 0–255 wrapping on platforms with signed `char`.
3. `LOOP_END` resumes the loop while `memory[pointer] > 0`; the spec means
   `!= 0`, which differs once cells can hold negative values.
4. End-of-input (`, ` at EOF) and division-by-zero behavior are unspecified;
   Phase 1 pins and tests them against oracle-defined expectations.

## Port notes (retired C++ behaviors)

Deliberate deviations from the original C++ implementation (retired in
slice 5f; documented, not mirrored):

1. A zero-cell `[` with no matching `]` spins forever in C++ (it keeps
   reading `NONE` past EOF). The Rust runner reports
   `Unbalanced Brainfuck code: unclosed '['` instead — a hang has no
   observable output to match.
2. A missing/unopenable code file spins forever in C++ (`BrainfuckReader`
   only breaks on `eof()`, which a failed-open stream never reports).
   The Rust CLIs treat it as an empty program instead.

## Source language

See `decisions/0001-source-language-identity.md` for what the accepted source
language is. Only constructs in that definition are in scope for hardening.

## Preprocessor

Object-like `#define` (and `#undef`) are expanded at tokenize time, in
source order: uses before a definition do not expand, redefinition
replaces the old value, chained macros expand, and a macro expanding
to itself yields one plain identifier. Function-like macros are an
error. Every other `#` line (including `#include`) is skipped, as
before; there is no conditional compilation or file inclusion.
