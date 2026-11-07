# 0016 — Pointer-to-pointer and multi-dimensional arrays

Date: 2026-11-07. Status: accepted.

## Context

The subset has one level of indirection (`int *p`, one-dimensional
arrays) and rejects the next level everywhere: the parser errors
on a second `*` and on a second bracket, and code generation
errors on a subscript whose base is itself a subscript. The
recommendation adopted for this batch was pointer-to-pointer plus
multi-dimensional arrays, with fuzzer coverage for both, because
both build on machinery that already exists: dereference is
already recursive (`*` loads through the 256-arm dispatch over
whatever address its operand evaluates to), subscripts already
distinguish static, range, and pointer paths, and pointer
arithmetic is already ordinary byte arithmetic. Nothing here
needs a new helper in the emitter.

## Decision: depth at most 2, dimensions as lists

Pointer depth and array dimensions become lists in the AST,
generalized from the current booleans:

- Declarators, parameters, typedef aliases, and function
  returns carry a pointer depth (`0`, `1`, `2`). A third `*`
  is still an error. Depth lives only in the AST and in
  signature comparison: code generation needs no depth, because
  a pointer variable of any depth is one cell holding an
  address byte, and `is_pointer` stays a boolean set.
- Array declarators carry a dimension list. The emitter keeps
  `(base, total length)` per array and gains a dimension table
  beside it; strides are products of the trailing dimensions.
  Multi-dimensional parameters carry their dimensions in the
  signature too, since the inner dimensions are the stride
  (the outermost parameter dimension is discarded, as in C).

The parse-dump format extends in the obvious way, with no
change for existing shapes: one `(pointer)` marker per `*`,
one size/`unsized` marker per dimension, in order. `SPEC.md`
is updated with the parser change.

## Semantics

Row-major layout, byte addresses, one byte per element, as
for one-dimensional arrays:

- `a[i][j]` addresses `base + i * stride + j`. Constant
  indices are bounds-checked per dimension and resolve to a
  static cell; variable indices compute a linear offset in
  byte arithmetic and load/store through the range dispatch
  over the array's cells, so an out-of-range variable index
  loads zero and stores nowhere, exactly as for one
  dimension. An array living past cell 255 keeps working
  through the range path.
- A short subscript chain names a row: `a[i]` decays to the
  address of `a[i][0]`, usable as a pointer value, a
  subscript base (`a[i][j]`), and an assignment source, but
  not as an assignment or increment target (it names an
  array). Subscripting past the last dimension (e.g.
  `a[0][1]` on a one-dimensional array) is an error.
- Subscript chains rooted at a pointer value step through
  loads: `pp[i][j]` loads the pointer at `pp + i`, adds `j`,
  and loads through it; `(*pp)[j]` and `pp[i]` compose the
  same way. A chain rooted at a one-dimensional pointer
  parameter keeps the existing pointer path, byte-identical.
- `sizeof(a)` is the total element count; `sizeof(a[i])`
  is the product of the remaining dimensions;
  `sizeof(a[i][j])`, `sizeof(*pp)`, and `sizeof(pp)` are 1.
- Initializers fill row-major. A flat list fills the whole
  array in order with zero fill; a list of lists fills row by
  row with zero fill per row; mixing flat and nested lists in
  one initializer is rejected, as are string initializers
  for multi-dimensional arrays (including lists of strings).
  Only the outermost dimension may be unsized, sized by the
  initializer. Deeper nesting than the dimension list (a
  nested list in a scalar or one-dimensional initializer)
  stays a code-generation error, as before.
- Typedef composition follows the declarator: a pointer
  alias plus `*` is depth 2, an array alias plus `[N]`
  prepends a dimension. `for` declarations stay scalar-only,
  as before. `***p`, function pointers, and structs are
  untouched.

## Fuzzer agreement

The generator gains both constructs under the existing
interval discipline, with these additional rules:

- Pointer-to-pointer bindings are fixed at declaration
  (`**pp = &p`), never rebound or incremented: pointer
  arithmetic would advance by one cell here and by
  `sizeof(int)` in C, so only dereference chains are
  generated. `**pp` reads and writes carry the ultimate
  target's interval; `(*pp)[k]` carries the array interval.
  A dereferenced pointer value is never printed or used as
  a number: addresses differ between the models.
- Multi-dimensional arrays are at most 2D and 3x3, with
  one per program. Both indices are in range (constant or
  interval-bounded). Row decay appears only as a pointer
  initializer (`*p = a[r]`), never as a printed value or a
  `%s` argument. Flat and nested initializers both appear;
  zero fill is folded into the array interval.

## Validation

The oracle fixtures `ptrptr` and `multidim` agree with g++;
one-dimensional emission is byte-identical for every prior
fixture. The 500-seed differential sweep after the generator
extensions passed 460 seeds, skipped 40 (8.0%, within the
10% cap), and mismatched none. The skip rate rose from
4.8% (24 skips) before this batch: the new constructs cost
cells, and the cap headroom is now 10 seeds. The sweep
also found two generator bugs, fixed in the generator, not
the compiler: whole-array decay for a two-dimensional
array must go through row 0 (`int *p = a;` is a pointer
to a row, not an element pointer), and `sizeof` results
must not appear in braced-list elements (a `size_t` value
narrows there in C++). One real compiler bug surfaced on
the way: the offset planner asserted equal stride and
index counts, which row chains violate; it now allows a
prefix. The one `sizeof` divergence the sweep exposed was
in the fixture, not the compiler: `sizeof(int[2][3])` is
24 in C and 6 in the byte model, so the fixture measures
`sizeof` on a `char` array.

## Rejected

- Recording depth in `Memory`: no code-generation decision
  needs it, and the boolean set already answers every
  question the emitter asks.
- Rewriting the subscript paths: the existing static /
  range / pointer split is kept, with a chain-flattening
  classification in front of it, so one-dimensional
  emission stays byte-identical.
- String-row initializers and mixed flat/nested lists:
  real C shapes, but each needs its own fill rule; rejected
  with a clear error and documented, rather than half
  supported.
- Three-dimensional arrays in the fuzzer: code generation
  is dimension-general, but the generator caps at 2D to
  keep programs inside the addressable cells.
