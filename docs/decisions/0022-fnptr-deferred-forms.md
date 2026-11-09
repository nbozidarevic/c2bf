# 0022 — Function-pointer deferred forms: typedef types, returns, arrays

Date: 2026-11-09. Status: accepted.

## Context

Record 0019 deferred three function-pointer forms, each a
parse or compile error with its own message: typedef
function-pointer types, function-pointer returns, and arrays
of function pointers. A callee ID is one byte and the
dispatch machinery (ID guards, shared result/returned cells,
signature-checked values) already exists, so all three are
representable without a new call model: a typedef carries a
signature, a return carries an ID into the caller's result
cell, and an array holds one ID per element cell.

## Decision: signatures flow through typedefs, returns, and elements

A `FuncSig` gains an optional returned function-pointer
signature (`returns_fn_sig`), compared recursively by
`FuncSig::matches`; a signature returning a function pointer
is never void and never has a return pointer depth. Typedefs
carry a signature (`TypedefDecl.fn_sig`), expanded into the
specifier like the existing pointer/array flags, so `cb fp`,
`cb arr[3]`, a `cb` parameter, and a `cb` return all mean the
same signature. Functions gain a `returns_fn_sig`: the
typedef spelling (`cb f(int)`) and the direct spelling
(`int (*f(int))(int)`, detected by a lookahead that
distinguishes it from a variable declarator) both parse
into it, and `return` compiles its operand as a
function-pointer value against that signature (a bare
`return;` yields null, as the result cell starts at zero).
Function-pointer arrays are one-dimensional only: elements
are callee IDs in consecutive cells, initialized from a
braced list through the same signature-checked value
compilation, subscripted through the existing array and
pointer chains, and callable by subscript
(`arr[i](x)`, `(*arr[i])(x)`), including through a
one-dimensional array parameter (which decays to an
address, like every array parameter). A subscript element
is a function-pointer lvalue: assignment validates the
signature, compound assignment and increment are errors,
and `*arr[i]` collapses to the ID like `*fp`.

## Scope

Supported: typedef function-pointer types (including
return-pointer spellings and typedef composition where the
aliased type itself returns a function pointer), typedef
and direct function-pointer returns, one-dimensional
function-pointer arrays (direct, typedef, and typedef-array
spellings; unsized only with a braced list), array
parameters of the same shape, calls through returned
function pointers (`f()(x)`, `(*f())(x)`), and `*arr[i]`
deref collapse. Deferred, still errors: pointers to
function pointers (`**`, `cb *p`, `**f()` returns),
nested function-pointer returns, multi-dimensional
function-pointer arrays (variables, typedefs, and
parameters), nested function-pointer parameters (unchanged
from 0019), function-pointer variables in `for`
declarators, and parenthesized pointer-to-array
declarators (unchanged). Addresses of function pointers
(`&fp`, `&arr[i]`) still slip through as for scalar
function pointers (a pre-existing gap, not widened here):
`&` on a scalar function pointer already compiled before
0019's rejection list, and dereferencing such an address
twice does not collapse to an ID. Stores through `*arr`
for a function-pointer array parameter are unchecked,
like deref stores through data pointers. `sizeof` of a
function-pointer array is its element count, as for
`char` arrays in this byte model.
