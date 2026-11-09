# Parse-dump format (v1)

`parsedump FILE` writes the parse tree to stdout as one single-line
S-expression plus `\n`, and exits 0. This format was the structural
contract between the frozen C++ reference (retired in slice 5f) and
the Rust core; it remains the parser contract, pinned by goldens.

## Serialization

- `(kind)` — node with empty text and no children, e.g. `(empty)`.
- `(kind text)` — leaf with text, e.g. `(ident x)`, `(int 42)`.
- `(kind text? kid kid...)` — single spaces, no trailing space, e.g.
  `(binop + (ident x) (int 42))`, `(compound (decl int ...))`.
- `text` uses the tokenizer dump escaping: `\` → `\\`, newline → `\n`,
  tab → `\t`, CR → `\r`, NUL → `\0`.
- No positions, no token stream: structure only. (Positions are covered
  by the tokenizer differential.)

## CLI conventions (same as `tokdump`)

- Tokenize/parse error: stderr `FILE:LINE:COL: error: MSG`, exit 1.
- Usage/open errors: exit 2. Negative fixtures only assert exit ≠ 0;
  stderr message parity is a SHOULD, verified by review and Rust unit
  tests, not by the differential.

## Node shapes (frozen at C++ retirement)

Phase 6 addition: `dowhile` — `(dowhile body cond)`, body first in
source order, e.g.
`(dowhile (compound ...) (binop < (ident i) (int 3)))`.

- `tu`: top-level `decl`/`funcdef` nodes in order (stray `;` skipped).
- `funcdef(name)`: `[params?, compound body]`. The return type is
  consumed, not shown; only `void`-ness is retained (invisibly) for
  code generation. `params` is `[param]*`, present only when the
  parameter list is non-empty (`()` and `(void)` both give no child).
  `param(name)`: `[pointer*, array*, fnsig?]` — one `pointer`
  per `*` (depth at most 2), one `array` per bracket for
  `name[]` or `name[N]` (the sizes are discarded; the inner
  sizes are the stride), and one `fnsig` for a
  function-pointer parameter (`int (*cb)(int)`).
  `fnsig`: `[params?, (void)?, pointer*]` — the pointed
  function's parameter list (omitted when empty), a `void`
  child for a void return, and one `pointer` per `*` of
  return pointer depth.
- `decl(spec)`: `[initdecl]*` (`int;` has no children; declarators
  are comma-separated, in order). `spec` is one or more words
  (`unsigned int`, `const char`, `long long`), space-joined.
- `initdecl(name)`: `[pointer*, (size | unsized)*, fnsig?, init?]` —
  one `pointer` (no children) per `*` (depth at most 2; a third
  is a parse error), one `size`/`unsized` per dimension in order,
  one `fnsig` for a function-pointer declarator
  (`int (*fp)(int)`),
  (`size` for `name[N]`, the size expression; `unsized` for
  `name[]`, sized by a string or braced initializer), then the
  initializer. A string literal may initialize an array, as may a
  braced initializer list (an `initlist` node: element
  expressions in order; empty and designated lists are parse
  errors, nested lists parse but are rejected in code generation
  for scalar and one-dimensional declarators).
- `compound`: block items in order. `exprstmt`: `[expr]`. `empty`: none.
- `if`: `[cond, then]`; `ifelse`: `[cond, then, else]`.
- `switch`: `[expr, body]`; `while`: `[cond, body]`.
- `for`: `[init, cond?, incr?, body]` — 2 to 4 children. `init` is
  `fordecl`, `empty-init`, or an expression; `cond`/`incr` appear only
  when non-empty (`for(;;)` → `[empty-init, body]`).
- `fordecl("spec name"|"spec")`: `[init]?` (`spec` may be
  multi-word, as in `decl`).
- `return`: `[expr]?`; `break`, `continue`: none (kind is the keyword).
- `case`: `[label, body]` (label is an assignment-expression);
  `default`: `[body]`; `goto-label(name)`: `[body]`.
- `comma`: `[first, rest...]` (2+; only built when a comma is present).
- `assign(op)`: `[lhs, rhs]`, right-associative; `op` is one of
  `= += -= *= /= %= &= |= ^= <<= >>=`. Non-lvalue `lhs` (other than
  parenthesized lvalues) is a parse error.
- `ternary`: `[cond, then, else]`, right-associative; sits between
  assignment and `||` — the condition is a logical-or expression, the
  middle is a full expression (commas allowed), and the else-branch is
  a conditional.
- `binop(op)`: `[l, r]`, left-associative. Levels low→high:
  `||`, `&&`, `|`, `^`, `&`, `== !=`, `< > <= >=`, `<< >>`,
  `+ -`, `* / %`.
- `unary(op)`: `[e]`; `op` is one of `! & * ++ -- - + ~` (no
  `sizeof` — accepted subset surface). Unary `-`/`~` wrap mod 256;
  `*` is dereference and `&` is address-of (see `docs/semantics.md`).
  Subscript and dereference forms are lvalues for assignment.
- `cast(spec)`: `[e]` — a cast is a no-op in the byte model;
  `spec` may be multi-word (`unsigned int`).
- `sizeof`: `[e]` (operand unevaluated, value always 1);
  `sizeof-type(spec)`: the type form, no children, `spec` may be
  multi-word.
- `postfix(op)`: `[e]` (`++`/`--`); `call`: `[callee, args...]`
  (callee is any postfix expression).
- `index`: `[base, index]` — subscript; chains nest (`a[i][j]`
  for multi-dimensional arrays, `pp[i][j]` stepping through the
  pointer each level loads). A chain longer than the array's
  dimensions, or subscripting a scalar element, is rejected in
  code generation.
- `paren`: `[e]` — parentheses are kept as nodes, not transparent.
- `int`: raw literal text — decimal, octal (`0` prefix; 8/9 there are a
  tokenizer error), hex (`0x`), binary (`0b`), with optional `u`/`l`
  suffixes; the value is computed at compile time and truncated to a
  byte. `char`: raw slice including quotes; `string`:
  concatenated cooked contents of adjacent literals (C string
  pasting); `ident`: the name.
- Trailing tokens after the translation unit are an error
  (`unexpected trailing ...`).
