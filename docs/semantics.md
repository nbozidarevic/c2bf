# Semantics

Ground-truth behavior for the transpiler, taken from the Master's thesis
(`Varijacije`, `Okruženje`, `Sintaksa`, `Testiranje interpretera` chapters).
Where the implementation deliberately deviates from the thesis or from C,
the deviation is stated here as defined behavior, pinned by tests.

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
- `sizeof` is `1` for scalar operands, since every type is one byte
  here. This differs from C for `int` (4) and for expressions of
  `int` type; only `char`-typed uses agree with the g++ oracle, as
  pinned by the `sizeof` fixture. The exceptions are arrays
  (element count, below) and string literals (bytes plus the NUL).
  The operand is unevaluated, as in C.
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
  integer literal, braced initializers and multi-dimensional
  arrays are rejected, and an array must be declared before use.
  A string literal may initialize an array (`char a[N] = "hi"`,
  bytes plus zero fill, a NUL when it fits; `char a[] = "hi"`
  sizes itself to the bytes plus the NUL), and a string literal
  in value position decays to the address of its anonymous,
  NUL-terminated storage. `printf` `%s` prints the
  NUL-terminated string at the argument's address; it must be
  NUL-terminated (an unterminated array reads on into following
  cells, as in C). `scanf` `%s` reads a whitespace-delimited
  string; into an array it truncates (see below), through a
  pointer it writes unbounded, as in C.
  Constant indices resolve at compile time: a constant index
  outside `0..N-1` is a compile error. Variable indices are also
  supported: `a[i]` loads through a dispatch over the array's
  range (offsets `0..255` only), and an out-of-range variable
  index loads zero and stores nowhere — defined here, undefined
  in C, so the fallback is pinned by unit tests, not by the
  oracle. `sizeof` an array is its element count (unlike the `1`
  rule for scalar operands).
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
  bare pointers, and store through the address. `scanf` `%d`
  writes one byte here; in C it writes `sizeof(int)` bytes, so
  `%d` into a `char` or an array element may clobber neighbors
  there — fixtures keep `%d` targets from overlapping. For `=`,
  a subscript's index expression is evaluated before the
  right-hand side; for compound assignment it is evaluated
  after it, as for the other compound forms.
- Every value is a single byte; there is no wider or aggregate
  representation yet (multi-byte integers and structs are deferred
  to language expansion).
- Integer literals may be decimal, octal, hex, or binary, with `u`/`l`
  suffixes; values truncate mod 256 like all arithmetic, and 8/9 in an
  octal literal is an error, as in g++.
- Type specifiers may be multi-word (`unsigned int`, `const char`,
  `long`, `signed`, `long long`); every spelling denotes one byte.
  Qualifier words stay identifiers outside specifier position, so a
  variable may still be named `long`.
- Division and modulo by zero are defined: `a / 0` is `0` and
  `a % 0` is `a`, keeping `a == (a / b) * b + (a % b)`. C leaves
  both undefined and the g++ oracle traps, so the behavior is
  pinned by unit tests. (Previously `a / 0` spun forever.)
- `scanf` `%d` honors a leading `-`: the value is negated with
  wrapping (`-5` reads as 251, and `-5 + 5` prints `0`); `+` and
  other skipped characters before the digits are ignored, as
  before. Pinned by the `io` oracle fixture (wrap-back form) and
  unit tests.
- `scanf` `%s` skips leading whitespace (bytes 9–13 and 32),
  then stores the longest run of non-whitespace bytes plus a NUL
  terminator. Into an array (including a constant element
  address `&a[i]`, bounded by the remaining length) at most
  `len - 1` characters are stored and the rest of the token is
  read and discarded, so over-long input cannot write past the
  array; arrays longer than 256 cells keep only their first
  256 reachable, as for variable-index stores. Through a pointer
  or a variable element address the bound is not known at
  compile time, so the write is unbounded like C and the caller
  must provide room; writing stops at the end of the
  addressable tape (cell 255), and a string filling the tape is
  left unterminated. End of input before any token character
  stores nothing (the destination keeps its contents, with no
  NUL written); end of input after token characters still
  terminates the string. A NUL byte in the input reads as end of
  input (the reader detects EOF by a zero sentinel). The
  terminating whitespace is consumed, not pushed back: `%c`
  after `%s` reads the character after the delimiter, unlike
  C. Pinned by the `scanfstr` oracle fixture and unit tests.

## Execution

- All cells start at `0`; the pointer starts at cell `0`. The tape
  grows as needed in both directions (the thesis reference
  interpreter addresses any integer index via `map<int, char>`,
  including negative ones). The tape starts at 1024 cells, so the
  30,000-cell minimum in the original Brainfuck spec never binds.
- `[` skips forward to the matching `]` when the current cell is `0`;
  `]` jumps back to the matching `[` when the current cell is not
  `0` (the cell is re-tested at `[`, so loop-exit means "current
  cell is `0`", not "`> 0`"). Unmatched brackets are runtime
  errors, reported in execution order, with partial output kept.
- `,` reads one character as its ASCII value; at end of input the
  cell is left unchanged. `.` writes the current cell as the
  character with that ASCII value. All other characters in BF
  source are ignored (comments must avoid `.` and `,`).

## Known implementation deviations

None remain at the runner level: the Rust runner matches the spec
on tape growth (unbounded both ways), end-of-input (cell
unchanged), and the loop test (`!= 0`). The retired C++ runner
differed on all three (fixed 65536-cell tape, `> 0` loop test);
those deviations were not mirrored in the port. The deviations
that remain are source-language ones, stated as defined behavior
in "Tape and cells" above: division/modulo by zero, `scanf` sign
and `%s` handling, wrapping single-byte arithmetic, and the
`sizeof` exceptions.

## Port notes (retired C++ behaviors)

Deliberate deviations from the original C++ implementation (retired in
slice 5f; documented, not mirrored):

1. A zero-cell `[` with no matching `]` spins forever in C++ (it keeps
   reading `NONE` past EOF). The Rust runner reports
   `Unbalanced Brainfuck code: unclosed '['` instead — a hang has no
   observable output to match.
2. A missing/unopenable code file spins forever in C++ (`BrainfuckReader`
   only breaks on `eof()`, which a failed-open stream never reports).
   The Rust CLI reports `Cannot open code file '...'.` instead.

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

## Functions

User-defined functions are supported by inline expansion: there is
no call stack and no return address, and the body is emitted at each
call site.

- Definitions may appear in any order; calls may refer to functions
  defined later in the file. Duplicate definitions and duplicate
  parameter names are compile errors. A definition without `main`
  is valid but emits nothing unless called. `printf` and `scanf`
  keep their built-in meanings and cannot be redefined.
- Parameters are values, except pointer (`*`) and array (`[]`/`[N]`)
  parameters, which receive an address. Array parameters decay to
  pointers, and the size is discarded. Arguments evaluate
  left-to-right into temporaries before any parameter is bound.
  A wrong number of arguments is a compile error.
- Recursion, direct or mutual, is a compile error: a function on
  the expansion stack may not be called again. (An uncalled
  recursive function is never expanded, so it compiles.)
- `return` sets the result and skips the rest of the callee at
  runtime, including loop re-entry, the `for` update, and later
  `switch` cases. Falling off the end returns the last returned
  value, or zero if `return` never ran. A bare `return;` sets no
  value; a value returned from a `void` function is ignored, and
  a `void` call used as a value yields zero. `return` in `main`
  stays a no-op, as before: the operand is not compiled.
- Prototypes (forward declarations) are file-scope only, with
  optional parameter names: `int f(int);` declares `f` without a
  body and emits nothing. A prototype must agree with its
  definition and with any earlier prototype: same void-ness,
  same pointer return, same parameter count, same pointer/array
  flags per parameter; parameter names and type spellings may
  differ (all spellings denote one byte). Repeated compatible
  prototypes are allowed; a conflicting one is a compile error.
  Calling a function with only a prototype is a compile error
  (there is no body to expand); an uncalled prototype compiles.
  No function pointers. A direct pointer return declarator
  (`char *f()`) does not parse yet; a pointer return spelled
  through a typedef alias does (see Typedefs).
- Declarations are lexically scoped: a declaration in a nested
  block shadows an outer one (each gets its own cells). A
  declaration at the top level of a function body may not reuse
  a parameter's name: parameters share the body's outermost scope,
  and both compilers reject it (verified against g++ and gcc
  15.2 on Ubuntu, and Apple Clang 21: g++ reports "shadows a
  parameter"). Shadowing a parameter inside a nested block or a
  `for` initializer is legal, as is shadowing a global at the top
  level.
- Redeclaring a name in the same block is a compile error
  ("redefinition"), in every scalar/pointer combination (verified
  against the same compilers). Array redeclarations are the
  exception: they keep the emitter's behavior — same-size
  redeclaration re-clears the elements, a size mismatch is an
  error, and variable/array confusion is an error. The same
  rule applies at file scope: a duplicate initialized definition
  is a redefinition, unless an array is involved (then the
  emitter decides). A bare declarator without an initializer is
  not a definition here — it is rejected as unsupported — so the
  tentative-definition question (legal in C, an error in C++) does
  not arise: every duplicate this compiler can express is
  initialized, and both compilers reject those.

## Typedefs

`typedef` aliases a type spelling for later uses. Aliases are
resolved at parse time: a use expands to the aliased specifier
plus the alias's pointer/array flags, and the alias itself emits
nothing.

- Declarations are file-scope only; a `typedef` inside a function
  body is a parse error (like prototypes). An alias is visible
  from its definition onward, in source order; uses before the
  definition treat the name as an opaque specifier word, as any
  unknown word pair already does. A typedef name does not hide
  behind a variable of the same name: the alias always wins in
  specifier position, a documented deviation from C scoping.
- One declarator per declaration, no initializer, no comma list.
  The declarator may add `*` or `[]` to a scalar alias; adding
  `*` to a pointer alias or `[]` to an array alias is an error
  (pointer-to-pointer and multi-dimensional arrays are
  unsupported). Chained aliases (`typedef MyInt Alias;`) expand
  transitively. Qualifiers may precede or follow the alias name
  (`const T`, `T const`); any other specifier word beside an
  alias name is an error.
- Array aliases carry an integer-literal size, checked at the
  definition (`typedef char A[3];`), or are unsized (`typedef
  char S[];`, sized by a string initializer at each use).
  Aliases work in every specifier position that takes a plain
  specifier: variable declarations, function parameters
  (including unnamed ones), function return types (including
  pointer returns), and prototypes. They are not expanded in
  casts or `sizeof` type operands, which only recognize built-in
  specifier words.
- Redefining an alias to an identical expansion is allowed;
  redefining it to a different expansion is a parse error.
  Pointer and array aliases are not supported as `for`
  declarations.

## Enums

`enum` defines named integer constants. Definitions are
file-scope only (like typedefs and prototypes); `enum Tag` as a
specifier word works anywhere, including in blocks.

- `enum Color { RED, GREEN = 5, BLUE };` defines `RED` as 0,
  `GREEN` as 5, and `BLUE` as 6 (each uninitialized enumerator
  takes the previous value plus one, starting at 0). The tag is
  optional (`enum { A, B };`), a trailing comma is allowed, and
  a definition with no enumerators is an error. An `enum`
  declaration may also declare variables (`enum Color c =
  BLUE;`), use an enum type as a function return type, or stand
  alone as a forward tag declaration (`enum Color;`, which
  defines nothing).
- Enumerator values are constant expressions: integer and
  character literals, earlier enumerators, parentheses, casts,
  and arithmetic/bitwise/comparison operators over constants.
  Anything else (variables, calls, division by zero) is a parse
  error. A duplicate enumerator name — in one list or across
  lists — is a parse error.
- Uses substitute the value at parse time, so an enumerator
  needs no storage: `case RED:` labels, initializers, and
  ordinary expressions all see the constant. Values are bytes
  like every value here: a value outside 0..255 wraps (a
  negative value wraps to 255), unlike the g++ oracle, so
  out-of-range values are pinned by unit tests, not fixtures.
- A declared name shadows an enumerator in its scope: a local
  variable, parameter, or `for` declarator of the same name is
  used as a variable while in scope, and the enumerator is
  visible again after the scope ends. Uses before any
  definition treat the name as an opaque specifier word, as any
  unknown word pair already does.

## Storage classes

`static`, `extern`, `auto`, and `register` are recognized in
specifier position. At file scope `static` and `extern` are
accepted and ignored: `static int x = 1;` behaves like a plain
global definition and a `static` function behaves like a plain
function. An `extern` declarator *with* an initializer is a
definition; one *without* an initializer is a non-defining
declaration — it is dropped, so a later definition of the same
name registers normally. An `extern`-only name that is never
defined still allocates a zero cell when used, which is a
deviation from C (a real linker would reject it).

`static` and `extern` are rejected in block scope: `static`
locals do not keep their values across calls (there is no
persistence beyond the function's own cells), and block-scope
`extern` is not supported. Storage classes are also rejected
in parameter lists and with `typedef`, except that `register`
is ignored in parameter lists. `auto` and `register` are
ignored everywhere. A `for`-loop declaration rejects `static`
and `extern` the same way a block declaration does. Storage
words are recognized only in specifier position, so the same
words can still name variables (`int static = 1;`). `auto int`
is accepted here but rejected by the g++ oracle (a C++ rule),
so it is pinned by unit tests only, not by the `storage`
fixture.
