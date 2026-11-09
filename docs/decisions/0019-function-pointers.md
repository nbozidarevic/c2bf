# 0019 — Function pointers by ID dispatch

Date: 2026-11-07. Status: accepted.

## Context

Calls are identifier-only: a call node names a function, and
the body expands inline at the call site. There is no call
stack, no return address, and no code address to point at,
so a function pointer cannot hold a tape index. What it can
hold is a small integer ID, assigned to each defined
function in translation-unit order, with `0` for null.

## Decision: IDs, signature-checked dispatch, no arithmetic

A function-pointer variable holds one byte: the callee ID.
A direct function designator (`f`, `&f`) yields its ID,
and `*fp` in value or callee position collapses to the ID
of the function `fp` names, as in C where dereferencing a
function designator yields a designator again.

An indirect call evaluates its arguments once, then
dispatches on the ID: for each defined function whose
signature matches the pointer's declared signature, a
guard expands that function's body, sharing one result
cell and one returned flag with the same reclaim rules as
a direct expansion. Only a matching arm can run. A null
call (ID `0`, or a signature with no defined function)
runs no arm and yields `0`, defined here as for a `void`
call used as a value. Recursion through a pointer is
rejected exactly like direct recursion: the expansion
stack already names the function being expanded.

Signatures are part of the pointer's type. Assigning a
function designator or another function pointer checks
compatibility with the declared signature (void-ness,
return depth, parameter depth/dimensions, recursively for
nested pointers) using the existing `FuncSig::matches`.
A function-pointer parameter receives an ID the same way;
a `**` parameter is its address. Arithmetic, compound
assignment, and increment on function pointers are
compile errors: IDs have no address arithmetic. So are
stores through a dereferenced function pointer
(`*fp = g`), which in C assigns to a function. Comparison
(`==`, `!=`, `!`) and assignment of designators, other
function pointers, and the null constant `0` are the only
other operations; any other value in a function-pointer
position is an incompatible-type error.

## Scope

Supported: direct declarators (`int (*fp)(int)`), including
the return-pointer spelling (`int *(*fp)(int)`), parameters
(`int f(int (*cb)(int))`, named or not), assignment from
designators and other function pointers, indirect calls in
both spellings (`fp(args)`, `(*fp)(args)`), and `&f`.

Deferred, each a compile or parse error with its own
message: function-pointer returns, typedef function-pointer
types, arrays of function pointers, pointers to function
pointers, nested function-pointer parameters, and
parenthesized pointer-to-array declarators. The pointer
declaration check (`*` count, third level) is unchanged;
a second `*` inside the parentheses is what makes the
declarator a pointer to a function pointer and is
rejected.

## Fuzzer agreement

The generator gains one helper-shaped binding at a time:
a function-pointer variable bound to the (single) helper
or to null, never rebound through an unknown target, so
every indirect call's return interval is the helper's.
Calls through a null-bound pointer are never generated
(they are a no-op here but a crash in C). Comparisons use
only `==`/`!=`/`!`, never ordering: ID order and address
order need not agree. Assignments copy the binding, so
the generator's interval merges track the target exactly;
a merge that disagrees makes the binding unknown, and an
unknown binding is compared but never called.

## Rejected

- Tape addresses for functions: no code address exists
  in inline expansion, and inventing one (a trampoline
  per function) would be a new runtime, not a dispatch.
- Multi-byte IDs: at most 255 defined functions can be
  named, which no corpus program approaches; the cap is
  a compile error only if ever reached.
- Stride-style dispatch through data cells: function
  bodies are code, not data, and only the guarded
  expansion above shares the direct call's reclaim and
  return protocol.
