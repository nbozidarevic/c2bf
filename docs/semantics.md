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
- An assignment expression yields the value stored, captured
  in a fresh cell: two assignments to the *same* variable
  inside one operator's operands each contribute the value
  they stored, as in C.
- Compound assignment (`op=`) desugars to `x = x op y` with the
  right-hand side evaluated first, as for the other compound
  forms; `&= |= ^= <<= >>=` use the bitwise/shift helpers.
- Arrays: `char a[N]` reserves `N` consecutive cells, named by
  the array and cleared at the declaration, so elements start at
  zero (C leaves them indeterminate). The size must be a positive
  integer literal, multi-dimensional arrays are rejected, and an
  array must be declared before use.
  A braced initializer list fills an array in order
  (`char a[3] = {1, 2}`, zero fill after the last element; more
  elements than the size is a compile error), and sizes an
  unsized array by the element count (`char a[] = {1, 2}` has 2
  elements and no NUL, unlike a string initializer). Elements
  are arbitrary expressions evaluated left to right; a list
  holding exactly one string literal is a string initializer
  (`char a[] = {"hi"}`). Empty, nested, and designated
  initializer lists are parse errors. An `int` expression into a
  `char` array narrows, which the Clang C++ oracle rejects in a
  list, so fixtures avoid that shape; the byte model itself
  truncates, as unit tests pin. A scalar declarator may
  take a one-element braced list (`int x = {5};`, including a
  `for` declarator); any other scalar list is a compile error.
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
  Multi-dimensional arrays (`int a[2][3]`) lay out row-major
  in consecutive cells. A subscript chain addresses `base +
  i * stride + j`, bounds-checked per dimension for constant
  indices; variable indices compute the linear offset in byte
  arithmetic and load/store through the range dispatch, with
  the same out-of-range fallback as one dimension. A short
  chain names a row: `a[i]` decays to the address of
  `a[i][0]` and may be used as a pointer value or a
  subscript base, but not as an assignment or increment
  target, and dereferencing a multi-dimensional array name
  (`*a`) likewise yields the base address. Subscripting
  past the last dimension is a compile error. `sizeof(a)`
  is the total element count, `sizeof(a[i])` the product
  of the remaining dimensions. A flat list fills row-major
  and a list of lists fills row by row, both zero-filling;
  mixing flat and nested lists in one initializer, string
  initializers, and unsized dimensions after the first are
  compile errors. Only the outermost dimension may be
  unsized, sized by the initializer.
- Pointers: there are no pointer types; a pointer is a byte
  holding a tape index. `&x` is the index of `x`'s cell, `*p`
  loads or stores the cell at the index `p` holds (a dispatch
  over all 256 byte addresses), a bare array name decays to
  `&a[0]`, and pointer arithmetic and comparison are ordinary
  byte arithmetic (wrapping). A pointer to a pointer (`int
  **pp`) holds the address of a pointer cell: `**pp` loads
  through both levels, `pp[i][j]` and `(*pp)[j]` step
  through the pointer each level loads, and `*pp` is an
  address-valued `scanf` destination. A third `*` is an
  error. Only the first 256 tape cells are
  addressable: taking the address of storage at cell 256 or
  above is a compile error, while static-index array access
  works anywhere. Deeply nested programs can exhaust the
  addressable cells even with few variables, because
  temporaries stay live across nesting levels and string
  literals reserve storage permanently, so such programs fail
  with the same error; this is a known limitation, not a bug
  in any one construct, and the fuzzer caps nesting depth to
  stay inside it. A string literal whose storage (including its
  NUL terminator) would cross cell 255 is likewise a compile
  error, rather than printing past the tape. The null pointer
  is address `0` (cell `0`).
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
  before. A number ending exactly at end of input reads its
  accumulated value: the reader detects EOF by a zero
  sentinel, as `%s` does, and a NUL byte in the input reads
  the same way. Pinned by the `io` oracle fixture (wrap-back
  form), the `scanf_eof` fixture, and unit tests.
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
- `printf` supports `%d`, `%u`, `%o`, `%x`, `%X`, `%c`, `%s`,
  and `%%`; `scanf` supports `%d`, `%u`, `%o`, `%x`, `%i`,
  `%c`, and `%s`. Length modifiers (`hh`, `h`, `ll`, `l`, `j`,
  `z`, `t`) are accepted and ignored in both — every value is
  one byte. Flags, field widths, and precisions are rejected,
  as are float conversions and `%p`/`%n`. `scanf` also supports
  assignment suppression (`%*d`, `%*s`, ...): the conversion is
  read and discarded and consumes no argument. `%i` detects its
  base: a `0x`/`0X` prefix reads hex, a leading `0` reads octal,
  otherwise decimal; `%x` accepts an optional `0x` prefix.
  `%o` reads octal digits only. `%i` on `09` yields `0` as in C,
  but the `9` is consumed here (C leaves it unread), so fixtures
  avoid that shape. Numeric `%o`/`%x`/`%i` conversions end at
  EOF the same way `%d` does (zero sentinel). Pinned by the
  `formats` oracle fixture and unit tests.

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
in "Tape and cells" above: division/modulo by zero, `scanf` sign,
base-prefix, and `%s` handling, wrapping single-byte arithmetic,
jumping over an initializer, and the `sizeof` exceptions.

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
- Function pointers: `int (*fp)(int)` holds a callee ID
  (assigned in translation-unit order, `0` for null). A
  function designator (`f`, `&f`) yields its ID, and
  dereferencing a function pointer (`*fp`, `**fp`) yields
  the same ID again. An indirect call (`fp(args)`,
  `(*fp)(args)`) evaluates its arguments once, then
  dispatches on the ID over the defined functions whose
  signature matches the pointer's declared signature;
  only the pointed-to body runs. Calling a null pointer
  runs no body and yields zero, defined here (a crash in
  C). Recursion through a pointer is rejected exactly
  like direct recursion. Assignment checks signature
  compatibility (void-ness, return depth, parameter
  depths/dimensions, recursively); only designators,
  function pointers of a compatible signature, and the
  null constant `0` may initialize or assign one.
  Arithmetic, compound assignment, and increment on
  function pointers are compile errors, as are stores
  through a dereferenced function pointer. Comparison
  (`==`, `!=`, `!`) tests identity/nullness.
- Typedef function-pointer types: `typedef int (*cb)(int);`
  aliases the signature, spelled through in variable,
  parameter, and return declarators (`cb fp`, `cb arr[3]`,
  `cb f(int)`), including return-pointer spellings and
  composition where the aliased type itself returns a
  function pointer. A `*` on top of such a typedef, or a
  parenthesized declarator over one, is a compile error
  (a pointer to a function pointer, or a nested return).
- Functions may return function pointers, spelled through
  a typedef (`cb f(int)`) or directly
  (`int (*f(int))(int)`): `return` compiles its operand
  as a function-pointer value against the returned
  signature, a bare `return;` yields null, and calls
  through the result (`f()(x)`, `(*f())(x)`) dispatch
  like any indirect call. A function pointer that itself
  returns one, and a pointer-to-function-pointer return,
  are compile errors.
- Arrays of function pointers are one-dimensional only:
  `int (*arr[3])(int)`, `cb arr[3]`, and typedef array
  spellings hold one callee ID per element, initialized
  from a braced list (unsized only with one), subscripted
  and called by element (`arr[i](x)`, `(*arr[i])(x)`),
  including through a one-dimensional array parameter
  (which decays to an address, like every array
  parameter). Subscript assignment validates the
  signature; `*arr[i]` collapses to the ID like `*fp`.
  Multi-dimensional arrays of function pointers are
  compile errors, as are string initializers for them.
  `sizeof` of such an array is its element count, as for
  `char` arrays in this byte model.
  Still unsupported, each a parse or compile error:
  pointers to function pointers, nested function-pointer
  parameters, nested function-pointer returns,
  multi-dimensional function-pointer arrays,
  function-pointer variables in `for` declarators, and
  parenthesized pointer-to-array declarators.
- Parameters are values, except pointer (`*`) and array (`[]`/`[N]`)
  parameters, which receive an address. Array parameters decay to
  pointers, and the size is discarded. Arguments evaluate
  left-to-right into temporaries before any parameter is bound.
  A function-pointer parameter receives a callee ID like a
  variable of the same type.
  A multi-dimensional array parameter decays to a pointer to
  its first row: the outermost size is discarded and the inner
  dimensions are the stride, so `m[i][j]` addresses through
  them. Pointer-to-pointer parameters (`int **pp`) receive an
  address like other pointers. A prototype must agree with its
  definition in pointer depth and in the inner dimensions of
  array parameters, as well as in the existing signature
  rules.
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
- `goto` jumps forward only, to a label defined later in the
  same function, one flag cell per label: statements between
  the jump and the label are skipped at runtime, including
  loop re-entry and the `for` update after a jump out of a
  loop, and declarations jumped over keep zero (their
  initializer is skipped). Duplicate or undefined labels,
  backward jumps, jumps into a loop, an `if` arm, or a
  `switch`, and jumps inside a `switch` are compile errors.
  Labels are function-scoped; a label with no preceding
  `goto` is a no-op marker. Decision record:
  `docs/decisions/0020-goto-spike.md`. Pinned by the `goto_*`
  oracle fixtures and unit tests.
- Prototypes (forward declarations) are file-scope only, with
  optional parameter names: `int f(int);` declares `f` without a
  body and emits nothing. A prototype must agree with its
  definition and with any earlier prototype: same void-ness,
  same pointer-return depth, same parameter count, same pointer
  depth and post-first dimensions per parameter, and same
  nested function-pointer signatures; parameter names
  and type spellings may differ (all spellings denote one byte).
  Repeated compatible prototypes are allowed; a conflicting one
  is a compile error. Calling a function with only a prototype
  is a compile error (there is no body to expand); an uncalled
  prototype compiles. A pointer return may be spelled
  directly (`char *f()`, `int **f()`) or through a typedef
  alias (see Typedefs); both spellings carry the same
  signature. A third `*` is an error.
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
  unknown word pair already does. A typedef name is hidden in
  any scope where a variable, parameter, or `for` declarator
  of the same name is declared: using it as a specifier there
  is a parse error, and it is visible again when that scope
  ends, matching C scoping (a name used as a value is
  unaffected; only specifier position hides the alias).
- One declarator per declaration, no initializer, no comma list.
  The declarator may add `*` or `[]` to a scalar alias, and
  the additions compose with the aliased flags: a pointer
  alias plus `*` is depth 2, and an array alias plus `[N]`
  prepends a dimension (`typedef char A[3]; A b[2];` is a
  2x3 array). Adding `*` to a depth-2 alias is an error
  (a third level is not supported). Chained aliases
  (`typedef MyInt Alias;`) expand transitively. Qualifiers
  may precede or follow the alias name (`const T`,
  `T const`); any other specifier word beside an alias name
  is an error.
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
specifier position. At file scope `static` is accepted and
ignored: `static int x = 1;` behaves like a plain global
definition and a `static` function behaves like a plain
function. An `extern` declarator *with* an initializer is a
definition; one *without* an initializer is a non-defining
declaration — it emits nothing, so a later definition of the
same name registers normally. Using a name that is declared
only as `extern` and never defined is a compile error when
the use is compiled (an undefined reference, like a linker
error); a `sizeof` operand does not count as a use, since it
is unevaluated, and neither does a use inside a function
that is never called, since its body is never expanded.

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
