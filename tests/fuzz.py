#!/usr/bin/env python3
"""Differential fuzzer: random C-subset programs vs the g++ oracle.

Each seed generates one program plus stdin, compiles it with g++
(the oracle, exactly as `tests/test` does) and with `c2bf`, runs
both, and compares stdout byte-for-byte. A seed that mismatches
fails loudly with the seed, the program, and the input, so the
case reproduces with `--seeds <seed>`.

A seed the compiler rejects only with the documented
addressable-cell limit ("address is outside the addressable
range") is outside the fuzzed subset: it is reported as a
skip, never hidden, and the run fails if skips exceed 10% of
seeds (any skip fails a single-seed run). Every other
rejection and every mismatch is a failure. Exit codes: 0
clean (skips within cap), 1 fuzz failure or skip cap
exceeded, 2 generator failure.

Usage (from `tests/`):

    python3 fuzz.py [--count N] [--seed-base N] [--seeds A,B,...]
                    [--keep-dir DIR] [--verbose]

    C2BF_BIN=../rs/target/release/c2bf python3 fuzz.py --count 50

Generator guarantees (see `docs/fuzzing.md`): every value stays
in 0..255, so C `int` arithmetic and the byte model agree on
every operation; divisors are nonzero; loops are bounded and
terminating; array indices are in range; scanf inputs are
well-formed tokens. Anything outside those guarantees is a
generator bug, not a c2bf bug: report it as such.
"""

import argparse
import difflib
import os
import random
import shutil
import subprocess
import sys
import tempfile

# Absolute at import: run_seed executes c2bf with cwd set to a temp
# dir, so a path relative to the invoking directory would resolve
# inside that temp dir instead. The default is anchored at this
# script (tests/../rs/...), so it works from any cwd; an explicit
# C2BF_BIN keeps the usual cwd-relative meaning.
_SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
_DEFAULT_BIN = os.path.normpath(os.path.join(
    _SCRIPT_DIR, "..", "rs", "target", "release", "c2bf"))
_RAW_BIN = os.environ.get("C2BF_BIN")
C2BF_BIN = os.path.abspath(_RAW_BIN) if _RAW_BIN else _DEFAULT_BIN
TIMEOUT = 15

# The compiler's documented addressable-cell limit (see
# docs/semantics.md): rejections carrying exactly this message
# are out-of-subset skips (see main()); anything else is a bug.
LIMIT_MSG = "address is outside the addressable range"
# Skips may not exceed this fraction of seeds (1 in 10); a
# single-seed run fails on any skip.
SKIP_CAP_FRAC = 10

# Total `stmt()` calls per program, including nested ones:
# nesting multiplies statements (one top-level `if` holds
# three more, one `switch` holds four case bodies), so an
# unbounded count lets straight-line temporaries accumulate
# past the 256 addressable cells (fuzz seeds 5 and 6 used 29
# and 25 calls). Empirical: see `docs/fuzzing.md`.
# Declarations and the trailing `printf`s are outside the
# budget; exhausted branches come out empty, which is still
# valid C.
STMT_BUDGET = 20


# --- intervals: every generated value stays in [0, 255] ------------

def fits(lo, hi):
    return 0 <= lo and hi <= 255


def iv_add(a, b):
    return (a[0] + b[0], a[1] + b[1])


def iv_sub(a, b):
    return (a[0] - b[1], a[1] - b[0])


def iv_mul(a, b):
    ps = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]]
    return (min(ps), max(ps))


def iv_div(a, b):
    # Caller guarantees b[0] >= 1 and a[0] >= 0.
    return (a[0] // b[1], a[1] // b[0])


def iv_mod(a, b):
    # Caller guarantees b[0] >= 1 and a[0] >= 0.
    return (0, min(b[1] - 1, a[1]))


def iv_shl(a, k):
    return (a[0] << k, a[1] << k)


def iv_shr(a, k):
    return (a[0] >> k, a[1] >> k)


def iv_bit(a, b):
    # Both sides bounded by 31: every bitwise result stays there.
    return (0, 31)


def iv_join(a, b):
    return (min(a[0], b[0]), max(a[1], b[1]))


BINOPS = ["+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^",
          "==", "!=", "<", "<=", ">", ">=", "&&", "||"]


class Gen:
    """One program. `env` maps scalar names to (lo, hi) intervals;
    `arrays` maps names to (length, (lo, hi), is_string). `mdims`
    maps two-dimensional array names to their [rows, cols];
    their `arrays` entry keeps the total length. `pptrs` maps
    pointer-to-pointer names to the pointer they are bound to;
    bindings are fixed at declaration, like `ptrs`."""

    def __init__(self, rng):
        self.rng = rng
        self.env = {}
        self.arrays = {}
        self.ptrs = {}
        self.mdims = {}
        self.pptrs = {}
        self.aliases = {}
        self.fnptrs = {}
        # Function-pointer arrays: name -> per-element binding
        # ("helper", None for null, or "?" for unknown), tracked
        # like `fnptrs` so only helper-bound elements are called.
        self.fnptr_arrays = {}
        self.chartypes = set()
        self.helpers = {}
        self.protected = set()
        self.counter = 0
        self.emitted = 0
        self.goto_used = False

    def fresh(self, stem):
        self.counter += 1
        return f"{stem}{self.counter}"

    # --- pure expressions: (text, lo, hi) ---------------------------

    def expr(self, depth, allow_call=True, allow_sizeof=True):
        rng = self.rng
        if depth <= 0 or rng.random() < 0.35:
            return self.leaf()
        roll = rng.random()
        if roll < 0.55:
            return self.binop(depth, allow_sizeof)
        if roll < 0.68:
            return self.ternary(depth, allow_sizeof)
        if roll < 0.78 and allow_call and self.helpers:
            return self.call(depth, allow_sizeof)
        if roll < 0.86:
            inner = self.expr(depth - 1, allow_call, allow_sizeof)
            return (f"({inner[0]})", inner[1], inner[2])
        return self.unop(depth, allow_sizeof)

    def leaf(self):
        rng = self.rng
        names = [n for n, iv in self.env.items() if fits(*iv)]
        if names and rng.random() < 0.6:
            name = rng.choice(names)
            return (name, *self.env[name])
        if rng.random() < 0.12:
            ch = rng.choice("abcdefghxyz0123456789")
            return (f"'{ch}'", ord(ch), ord(ch))
        v = rng.choice([0, 1, 2, 3, 4, 5, 7, 8, 9, 10, 12, 15, 16,
                        20, 24, 31, 32, 48, 64, 100, 127, 200, 255])
        return (str(v), v, v)

    def binop(self, depth, allow_sizeof=True):
        rng = self.rng
        op = rng.choice(BINOPS)
        # Function-pointer equality uses addresses in C and
        # callee IDs here: equal exactly when the bindings
        # name the same function, and ordering is never
        # generated (ID order and address order need not
        # agree). The result is always a boolean.
        if op in ("==", "!=") and (self.fnptrs or self.fnptr_arrays) \
                and rng.random() < 0.25:
            elems = [f"{n}[{k}]" for n, bs in self.fnptr_arrays.items()
                     for k in range(len(bs))]
            pool = [*self.fnptrs, "helper", "0", *elems]
            left = rng.choice(pool)
            right = rng.choice(pool)
            if left in self.fnptrs or right in self.fnptrs \
                    or left in elems or right in elems:
                return (f"({left} {op} {right})", 0, 1)
        for _ in range(6):
            l = self.expr(depth - 1, allow_sizeof=allow_sizeof)
            r = self.expr(depth - 1, allow_sizeof=allow_sizeof)
            if op in ("+", "-", "*"):
                iv = {"+": iv_add, "-": iv_sub, "*": iv_mul}[op](l[1:], r[1:])
            elif op in ("/", "%"):
                if r[1] < 1:
                    continue
                iv = iv_div(l[1:], r[1:]) if op == "/" else iv_mod(l[1:], r[1:])
            elif op == "<<":
                if r[2] > 3 or r[1] < 0:
                    continue
                k = rng.randint(0, 3)
                iv = iv_shl(l[1:], k)
                # The shift branches return early, bypassing the
                # fits() check below that the other operators
                # go through: a shifted interval can exceed 255
                # ('y' << 3 is 968 in C, 200 here), so check
                # the shifted interval itself, not just the count.
                if not fits(*iv):
                    continue
                return (f"({l[0]} << {k})", *iv)
            elif op == ">>":
                if r[2] > 4 or r[1] < 0:
                    continue
                k = rng.randint(0, 4)
                iv = iv_shr(l[1:], k)
                if not fits(*iv):
                    continue
                return (f"({l[0]} >> {k})", *iv)
            elif op in ("&", "|", "^"):
                if l[2] > 31 or r[2] > 31:
                    continue
                iv = iv_bit(l[1:], r[1:])
            elif op in ("==", "!=", "<", "<=", ">", ">="):
                iv = (0, 1)
            else:  # && ||
                iv = (0, 1)
            if not fits(*iv):
                continue
            return (f"({l[0]} {op} {r[0]})", *iv)
        return self.leaf()

    def ternary(self, depth, allow_sizeof=True):
        cond = self.expr(depth - 1, allow_sizeof=allow_sizeof)
        a = self.expr(depth - 1, allow_sizeof=allow_sizeof)
        b = self.expr(depth - 1, allow_sizeof=allow_sizeof)
        iv = iv_join(a[1:], b[1:])
        if not fits(*iv):
            return a if fits(*a[1:]) else self.leaf()
        return (f"({cond[0]} ? {a[0]} : {b[0]})", *iv)

    def call(self, depth, allow_sizeof=True):
        # Indirect calls through a pointer bound to the
        # helper: never through a null or unknown binding
        # (a null call crashes in C, and an unknown binding
        # might be null). The return interval is the
        # helper's, whatever the spelling.
        if self.fnptrs and "helper" in self.helpers \
                and not self.helpers["helper"][1] \
                and self.rng.random() < 0.35:
            callable = [n for n, t in self.fnptrs.items()
                        if t == "helper"]
            if callable:
                name = self.rng.choice(callable)
                ret_iv = self.helpers["helper"][0]
                arg = self.expr(depth - 1, allow_call=False,
                                allow_sizeof=allow_sizeof)
                form = self.rng.choice([f"{name}({arg[0]})",
                                        f"(*{name})({arg[0]})"])
                return (form, *ret_iv)
        # An indirect call through a helper-bound array
        # element, in either spelling: the return interval
        # is the helper's, whatever the spelling.
        if self.fnptr_arrays and "helper" in self.helpers \
                and not self.helpers["helper"][1] \
                and self.rng.random() < 0.1:
            elems = [(n, k) for n, bs in self.fnptr_arrays.items()
                     for k, b in enumerate(bs) if b == "helper"]
            if elems:
                name, k = self.rng.choice(elems)
                ret_iv = self.helpers["helper"][0]
                arg = self.expr(depth - 1, allow_call=False,
                                allow_sizeof=allow_sizeof)
                form = self.rng.choice([f"{name}[{k}]({arg[0]})",
                                        f"(*{name}[{k}])({arg[0]})"])
                return (form, *ret_iv)
        name, (ret_iv, is_pp, is_char) = self.rng.choice(
            list(self.helpers.items()))
        if is_pp:
            # A `**` parameter takes the address of a pointer
            # of the matching element type; the returned
            # interval is the wide (0..255) / (0..127) bound
            # for whatever that pointer ultimately names.
            # With no such pointer in scope, there is no
            # well-typed call: fall back to a leaf.
            eligible = [n for n, p in self.ptrs.items()
                        if p[2] == is_char]
            if not eligible:
                return self.leaf()
            pname = self.rng.choice(eligible)
            return (f"{name}(&{pname})", *ret_iv)
        arg = self.expr(depth - 1, allow_call=False,
                        allow_sizeof=allow_sizeof)
        return (f"{name}({arg[0]})", *ret_iv)

    def capped(self, depth, cap, allow_sizeof=True):
        """Expression whose interval fits in [0, cap] (for `char`
        targets: C converts larger values implementation-defined,
        so only 0..127 may flow into one). With
        `allow_sizeof=False`, no `sizeof` subexpression is
        emitted: `sizeof` has type `size_t`, and a `size_t`
        value in a braced list element is a narrowing error
        in C++, even when the value fits."""
        for _ in range(8):
            e = self.expr(depth, allow_sizeof=allow_sizeof)
            if e[2] <= cap:
                return e
        v = self.rng.randint(0, cap)
        return (str(v), v, v)

    def unop(self, depth, allow_sizeof=True):
        # `!fp` tests nullness: 0 is null in both models, and
        # any callee ID is nonzero in both. Array elements
        # test the same way.
        if self.fnptr_arrays and self.rng.random() < 0.06:
            name = self.rng.choice(list(self.fnptr_arrays))
            k = self.rng.randrange(len(self.fnptr_arrays[name]))
            return (f"(!{name}[{k}])", 0, 1)
        if self.fnptrs and self.rng.random() < 0.12:
            name = self.rng.choice(list(self.fnptrs))
            return (f"(!{name})", 0, 1)
        inner = self.expr(depth - 1, allow_sizeof=allow_sizeof)
        roll = self.rng.random()
        if roll < 0.45:
            return (f"(!{inner[0]})", 0, 1)
        if roll < 0.7 and inner[2] <= 127:
            # A cast to char is the identity only below 128;
            # above that C sign-extends and the models diverge.
            return (f"((char){inner[0]})", *inner[1:])
        # sizeof only for char-typed storage: sizeof(int) is 4
        # in C but 1 here, while sizeof(char) is 1 in both, and
        # sizeof(char[N]) is N in both. Skipped entirely for
        # braced-list elements (see capped()): the size_t
        # result narrows there.
        if allow_sizeof:
            cands = [n for n in self.chartypes]
            if cands and roll < 0.85:
                return (f"(sizeof({self.rng.choice(cands)}))", 1, 1)
            char_arrays = [n for n, a in self.arrays.items() if a[3]]
            if char_arrays:
                name = self.rng.choice(char_arrays)
                return (f"(sizeof({name}))", self.arrays[name][0], self.arrays[name][0])
            char2d = [n for n in self.mdims
                      if self.arrays[n][3]]
            if char2d and roll < 0.92:
                name = self.rng.choice(char2d)
                dims = self.mdims[name]
                # sizeof of one row: the product of the
                # remaining dimensions.
                size = 1
                for d in dims[1:]:
                    size *= d
                r = self.rng.randrange(dims[0])
                return (f"(sizeof({name}[{r}]))", size, size)
        return (f"(!{inner[0]})", 0, 1)

    def indexable(self):
        """(name, length) of a one-dimensional array usable for
        reads/writes; two-dimensional arrays are handled by
        `indexable2d` (a single subscript names a row there,
        whose value is an address the models disagree on)."""
        names = [n for n in self.arrays if n not in self.mdims]
        if not names:
            return None
        name = self.rng.choice(names)
        length = self.arrays[name][0]
        return (name, length)

    def indexable2d(self):
        """(name, dims) of a multi-dimensional array."""
        if not self.mdims:
            return None
        name = self.rng.choice(list(self.mdims))
        return (name, self.mdims[name])

    def subscript_text(self, name, depth):
        """A fully-subscripted access `name[i][j]...` with every
        index in range, or None: constant indices most of the
        time, interval-bounded variable indices otherwise."""
        rng = self.rng
        dims = self.mdims[name]
        if rng.random() < 0.6:
            idx = [str(rng.randrange(d)) for d in dims]
            return "".join(f"[{k}]" for k in idx)
        parts = []
        for d in dims:
            e = self.expr(depth - 1)
            if not (0 <= e[1] and e[2] < d):
                return None
            parts.append(e[0])
        return "".join(f"[{k}]" for k in parts)

    def ptr_target_iv(self, ppname):
        """Interval of the ultimate target of a
        pointer-to-pointer: the scalar interval, or the whole
        array interval (a join over all its elements)."""
        tgt = self.pptrs[ppname]
        kind, ultimate, _ = self.ptrs[tgt]
        if kind == "var":
            return self.env[ultimate]
        return self.arrays[ultimate][1]

    def ptr_target_len(self, ppname):
        """Subscript bound for `(*pp)[k]`: None for a scalar
        target, the row length for a row pointer, the total
        length for an array pointer."""
        tgt = self.pptrs[ppname]
        kind, ultimate, _ = self.ptrs[tgt]
        if kind == "var":
            return None
        if kind == "arrayrow":
            return self.mdims[ultimate][1]
        return self.arrays[ultimate][0]

    def readable_expr(self, depth):
        """Expression usable as an rvalue, including array reads;
        intervals still hold."""
        rng = self.rng
        if rng.random() < 0.1 and self.mdims:
            got = self.indexable2d()
            if got:
                name, _ = got
                iv = self.arrays[name][1]
                sub = self.subscript_text(name, depth)
                if sub is not None:
                    return (f"{name}{sub}", *iv)
        if rng.random() < 0.22:
            got = self.indexable()
            if got:
                name, length = got
                iv = self.arrays[name][1]
                if rng.random() < 0.6:
                    k = rng.randrange(length)
                    return (f"{name}[{k}]", *iv)
                idx = self.expr(depth - 1)
                if 0 <= idx[1] and idx[2] < length:
                    return (f"{name}[{idx[0]}]", *iv)
        if self.ptrs and rng.random() < 0.15:
            pname = rng.choice(list(self.ptrs))
            kind, target, is_char = self.ptrs[pname]
            if kind == "var":
                return (f"*{pname}", *self.env[target])
            if kind == "arrayrow":
                length = self.mdims[target][1]
            else:
                length = self.arrays[target][0]
            iv = self.arrays[target][1]
            k = rng.randrange(length)
            return (f"{pname}[{k}]", *iv)
        if self.pptrs and rng.random() < 0.25:
            ppname = rng.choice(list(self.pptrs))
            iv = self.ptr_target_iv(ppname)
            length = self.ptr_target_len(ppname)
            if length is not None and rng.random() < 0.65:
                if rng.random() < 0.6:
                    k = rng.randrange(length)
                    form = rng.choice([f"(*{ppname})[{k}]",
                                       f"{ppname}[0][{k}]"])
                    return (form, *iv)
                idx = self.expr(depth - 1)
                if 0 <= idx[1] and idx[2] < length:
                    form = rng.choice([f"(*{ppname})[{idx[0]}]",
                                       f"{ppname}[0][{idx[0]}]"])
                    return (form, *iv)
            # A dereferenced pointer value is an address and
            # must never be printed or used as a number; only
            # the double dereference carries a value interval.
            return (f"**{ppname}", *iv)
        return self.expr(depth)

    # --- declarations ---------------------------------------------

    def decl_scalar(self):
        rng = self.rng
        name = self.fresh("v")
        is_char = rng.random() < 0.3
        cap = 127 if is_char else 255
        e = self.capped(2, cap)
        self.env[name] = (e[1], e[2])
        if is_char:
            self.chartypes.add(name)
        return f"{'char' if is_char else 'int'} {name} = {e[0]};"

    def char_elem(self):
        """A char-typed braced-list element: C++ list-init rejects
        a non-constant `int` expression narrowing into `char`,
        even when the value fits, so elements are literals, char
        variables, or `(char)` casts."""
        rng = self.rng
        roll = rng.random()
        if roll < 0.3:
            ch = rng.choice("abcdefgxyz")
            return (f"'{ch}'", ord(ch), ord(ch))
        if roll < 0.5 and self.chartypes:
            name = rng.choice(list(self.chartypes))
            return (name, *self.env[name])
        if roll < 0.7:
            v = rng.randint(0, 127)
            return (str(v), v, v)
        e = self.capped(2, 127)
        return (f"((char){e[0]})", e[1], e[2])

    def decl_array(self):
        rng = self.rng
        name = self.fresh("a")
        is_char = rng.random() < 0.5
        typ = "char" if is_char else "int"
        cap = 127 if is_char else 255
        if is_char and rng.random() < 0.4:
            word = "".join(rng.choice("abcdefgxyz") for _ in range(rng.randint(1, 5)))
            length = len(word) + 1
            self.arrays[name] = (length, (0, max(ord(c) for c in word)), True, True)
            return f'char {name}[] = "{word}";'
        n = rng.randint(2, 5)
        if is_char:
            elems = [self.char_elem() for _ in range(rng.randint(1, n))]
        else:
            elems = [self.capped(2, cap, allow_sizeof=False)
                     for _ in range(rng.randint(1, n))]
        lo = min(e[1] for e in elems)
        hi = max(e[2] for e in elems)
        if len(elems) < n:
            # Unfilled elements are zero: the read interval must
            # include 0, or later reasoning under-approximates.
            lo = 0
        self.arrays[name] = (n, (lo, hi), False, is_char)
        inits = ", ".join(e[0] for e in elems)
        if rng.random() < 0.5:
            return f"{typ} {name}[{n}] = {{{inits}}};"
        self.arrays[name] = (len(elems), (lo, hi), False, is_char)
        return f"{typ} {name}[] = {{{inits}}};"

    def decl_pointer(self):
        rng = self.rng
        name = self.fresh("p")
        if self.arrays and rng.random() < 0.5:
            target = rng.choice(list(self.arrays))
            length, _, _, is_char = self.arrays[target]
            typ = "char" if is_char else "int"
            if len(self.mdims.get(target, [])) == 2 and rng.random() < 0.5:
                # Row decay: the pointer names one row; its
                # length is the row length, its interval the
                # whole-array join. (A row of a 3D array is
                # itself an array, not an element pointer, so
                # row decay only applies to 2D arrays.)
                r = rng.randrange(self.mdims[target][0])
                self.ptrs[name] = ("arrayrow", target, is_char)
                return f"{typ} *{name} = {target}[{r}];"
            if target in self.mdims:
                # A multi-dimensional array name decays to a
                # pointer to its first row, not to an element
                # pointer: decay through the first row (or
                # rows) to the first element.
                zeros = "[0]" * (len(self.mdims[target]) - 1)
                self.ptrs[name] = ("array", target, is_char)
                return f"{typ} *{name} = {target}{zeros};"
            self.ptrs[name] = ("array", target, is_char)
            return f"{typ} *{name} = {target};"
        names = [n for n in self.env if n not in self.protected]
        if not names:
            return None
        target = rng.choice(names)
        is_char = target in self.chartypes
        self.ptrs[name] = ("var", target, is_char)
        typ = "char" if is_char else "int"
        return f"{typ} *{name} = &{target};"

    def gen_typedef(self):
        """One file-scope typedef: pointer, array, or
        function-pointer alias. Uses are spelled through the
        alias in declarations; the alias name never collides
        with a variable stem."""
        rng = self.rng
        roll = rng.random()
        if roll < 0.4:
            is_char = rng.random() < 0.5
            typ = "char" if is_char else "int"
            self.aliases["TPA"] = ("ptr", 1, is_char)
            return f"typedef {typ} *TPA;"
        if roll < 0.7:
            is_char = rng.random() < 0.5
            typ = "char" if is_char else "int"
            dim = rng.randint(2, 3)
            self.aliases["TAA"] = ("array", [dim], is_char)
            return f"typedef {typ} TAA[{dim}];"
        self.aliases["TCB"] = ("fnptr", None, False)
        return "typedef int (*TCB)(int);"

    def decl_md_array(self):
        """One multi-dimensional array: 2D, or 3D with
        probability ~1/3, each dimension 2..3, filled by a
        flat or nested list with zero fill folded into the
        interval (a short list at any level leaves zeroes
        behind). With an array alias, the dimensions compose
        through it instead."""
        rng = self.rng
        name = self.fresh("m")
        alias = self.aliases.get("TAA")
        use_alias = alias is not None and rng.random() < 0.6
        if use_alias:
            is_char = alias[2]
            dims = [rng.randint(2, 3), alias[1][0]]
        else:
            is_char = rng.random() < 0.5
            ndim = 3 if rng.random() < 0.35 else 2
            dims = [rng.randint(2, 3) for _ in range(ndim)]
        typ = "char" if is_char else "int"
        cap = 127 if is_char else 255
        total = 1
        for d in dims:
            total *= d

        def elem():
            if is_char:
                return self.char_elem()
            return self.capped(2, cap, allow_sizeof=False)

        if rng.random() < 0.6:
            text, lo, hi = self.init_list_text(dims, is_char)
        else:
            n = rng.randint(1, total)
            elems = [elem() for _ in range(n)]
            lo = min(e[1] for e in elems)
            hi = max(e[2] for e in elems)
            if n < total:
                lo = 0
            text = "{" + ", ".join(e[0] for e in elems) + "}"
        self.arrays[name] = (total, (lo, hi), False, is_char)
        self.mdims[name] = dims
        if use_alias:
            return f"TAA {name}[{dims[0]}] = {text};"
        dims_text = "".join(f"[{d}]" for d in dims)
        return f"{typ} {name}{dims_text} = {text};"

    def init_list_text(self, dims, is_char):
        """(text, lo, hi) for a nested braced list filling
        dimensions `dims`, outermost first, with zero fill
        folded into the interval (a short list at any level
        leaves zeroes behind)."""
        rng = self.rng
        cap = 127 if is_char else 255

        def elem():
            if is_char:
                return self.char_elem()
            return self.capped(2, cap, allow_sizeof=False)

        def build(ds):
            if len(ds) == 1:
                n = rng.randint(1, ds[0])
                elems = [elem() for _ in range(n)]
                text = "{" + ", ".join(e[0] for e in elems) + "}"
                return (text, min(e[1] for e in elems),
                        max(e[2] for e in elems), n < ds[0])
            parts = []
            lo, hi, zf = 255, 0, False
            n = rng.randint(1, ds[0])
            for _ in range(n):
                text, l, h, z = build(ds[1:])
                parts.append(text)
                lo, hi, zf = min(lo, l), max(hi, h), zf or z
            if n < ds[0]:
                zf = True
            return ("{" + ", ".join(parts) + "}", lo, hi, zf)

        text, lo, hi, zf = build(dims)
        if zf:
            lo = 0
        return (text, lo, hi)

    def decl_fnptr(self):
        """A function-pointer variable bound to the helper or
        to null. Only the scalar helper's signature exists in
        generated programs: (int) -> int. The binding is the
        generator's record of what the variable holds: the
        helper's name, None for null, or "?" for unknown
        after a disagreeing merge."""
        rng = self.rng
        if "helper" not in self.helpers or self.helpers["helper"][1]:
            return None
        name = self.fresh("f")
        if rng.random() < 0.3:
            self.fnptrs[name] = None
            if "TCB" in self.aliases and rng.random() < 0.4:
                return f"TCB {name} = 0;"
            return f"int (*{name})(int) = 0;"
        self.fnptrs[name] = "helper"
        if "TCB" in self.aliases and rng.random() < 0.4:
            return f"TCB {name} = helper;"
        return f"int (*{name})(int) = helper;"

    def decl_fnptr_array(self):
        """A function-pointer array bound element-wise to the
        helper or to null, spelled directly or through the
        `TCB` alias when one exists. Per-element bindings
        are tracked so only helper-bound elements are
        called; the array is never rebound wholesale."""
        rng = self.rng
        if "helper" not in self.helpers or self.helpers["helper"][1]:
            return None
        name = self.fresh("fa")
        n = rng.randint(2, 3)
        bindings = []
        elems = []
        for _ in range(n):
            if rng.random() < 0.3:
                bindings.append(None)
                elems.append("0")
            else:
                bindings.append("helper")
                elems.append("helper")
        self.fnptr_arrays[name] = bindings
        inits = ", ".join(elems)
        if "TCB" in self.aliases and rng.random() < 0.4:
            return f"TCB {name}[{n}] = {{{inits}}};"
        return f"int (*{name}[{n}])(int) = {{{inits}}};"

    def decl_pp(self):
        """A pointer-to-pointer bound to an existing pointer.
        The binding never changes: rebinding or incrementing
        a pointer would advance by a cell here and by
        sizeof(int) in C, so only dereference chains are
        generated for it."""
        rng = self.rng
        if not self.ptrs:
            return None
        # A pointer alias plus `*` is depth 2: spell the
        # binding through it when one exists and the element
        # types match.
        if "TPA" in self.aliases and rng.random() < 0.7:
            is_char = self.aliases["TPA"][2]
            eligible = [n for n, p in self.ptrs.items()
                        if p[2] == is_char]
            if eligible:
                target = rng.choice(eligible)
                name = self.fresh("pp")
                self.pptrs[name] = target
                return f"TPA *{name} = &{target};"
        name = self.fresh("pp")
        # Bind mostly to array pointers: only those give a
        # chain form (`(*pp)[k]`), scalar targets only give
        # `**pp`.
        array_ptrs = [n for n, p in self.ptrs.items()
                      if p[0] in ("array", "arrayrow")]
        if array_ptrs and rng.random() < 0.7:
            target = rng.choice(array_ptrs)
        else:
            target = rng.choice(list(self.ptrs))
        is_char = self.ptrs[target][2]
        self.pptrs[name] = target
        typ = "char" if is_char else "int"
        return f"{typ} **{name} = &{target};"

    # --- statements -------------------------------------------------

    def snap(self):
        return (dict(self.env), {k: v for k, v in self.arrays.items()},
                set(self.chartypes), dict(self.fnptrs),
                {k: list(v) for k, v in self.fnptr_arrays.items()})

    def merge(self, snaps):
        envs, arrs = [s[0] for s in snaps], [s[1] for s in snaps]
        for name in self.env:
            ivs = [e[name] for e in envs if name in e]
            if ivs:
                self.env[name] = (min(i[0] for i in ivs), max(i[1] for i in ivs))
        for name in self.arrays:
            ivs = [a[name][1] for a in arrs if name in a]
            if ivs:
                old = self.arrays[name]
                self.arrays[name] = (old[0], (min(i[0] for i in ivs),
                                              max(i[1] for i in ivs)), old[2], old[3])
        # A function-pointer binding the branches disagree on
        # becomes unknown: comparable, but never callable
        # (it might be null, which must not be called).
        for name in self.fnptrs:
            vals = [s[3][name] for s in snaps if name in s[3]]
            if vals:
                self.fnptrs[name] = vals[0] if all(
                    v == vals[0] for v in vals) else "?"
        # Per-element bindings merge element-wise, like the
        # scalar bindings: a disagreement becomes unknown.
        for name in self.fnptr_arrays:
            for k in range(len(self.fnptr_arrays[name])):
                vals = [s[4][name][k] for s in snaps
                        if name in s[4] and k < len(s[4][name])]
                if vals:
                    self.fnptr_arrays[name][k] = vals[0] if all(
                        v == vals[0] for v in vals) else "?"

    def assignable(self):
        return [n for n in self.env if n not in self.protected]

    def stmt(self, depth, loop, lines, pure=False, allow_while=True,
             allow_loop=True, allow_goto=False):
        self.emitted += 1
        if self.emitted > STMT_BUDGET:
            # Budget exhausted: emit nothing. Callers tolerate
            # empty branches (an `if`/`for`/`switch` with no
            # body statements is still valid C), and the merged
            # interval environment is unchanged by a statement
            # that never happened.
            return
        rng = self.rng
        if allow_goto and not self.goto_used and self.assignable() \
                and rng.random() < 0.03:
            self.gen_goto_segment(lines, depth)
            return
        roll = rng.random()
        if pure:
            # Loop bodies never change persistent state: reasoning
            # about intervals across iterations would need a
            # fixpoint, so bodies only print or control flow.
            roll = 0.62 + roll * 0.38
        if depth <= 0 and 0.52 <= roll < 0.8:
            # Depth exhausted: no further nesting. The old clamp
            # only forced the print branch for roll >= 0.62, so
            # if/loop/switch still nested past depth 0 without
            # bound, and live temporaries could exceed the 256
            # addressable cells (fuzz seed 5 at depth 5). The
            # nesting band is [0.52, 0.8): assignments, increments,
            # break/continue, and prints outside it stay as-is.
            roll = 0.9
        if not allow_loop and 0.62 <= roll < 0.74:
            # Inside a switch case (at any depth): no loops. A
            # loopy case body plus the switch's match/guard cells
            # peaks past the 256 addressable cells (fuzz seed 5
            # after the depth cap); flat cases keep the peak
            # inside. Loops elsewhere, and switches inside
            # loops, are unaffected.
            roll = 0.9
        if roll < 0.3 and self.assignable():
            name = rng.choice(self.assignable())
            cap = 127 if name in self.chartypes else 255
            e = self.capped(depth, cap)
            if rng.random() < 0.25:
                op = rng.choice(["+=", "-=", "*="])
                if op == "+=":
                    iv = iv_add(self.env[name], e[1:])
                elif op == "-=":
                    iv = iv_sub(self.env[name], e[1:])
                else:
                    iv = iv_mul(self.env[name], e[1:])
                if fits(*iv) and iv[1] <= cap:
                    lines.append(f"{name} {op} {e[0]};")
                    self.env[name] = iv
                    return
            lines.append(f"{name} = {e[0]};")
            self.env[name] = (e[1], e[2])
            return
        if roll < 0.4:
            if self.mdims and rng.random() < 0.35:
                got = self.indexable2d()
                if got:
                    name, _ = got
                    old = self.arrays[name]
                    _, iv, _, is_char = old
                    cap = 127 if is_char else 255
                    e = self.capped(depth, cap)
                    sub = self.subscript_text(name, depth)
                    if sub is None:
                        return
                    lines.append(f"{name}{sub} = {e[0]};")
                    self.arrays[name] = (old[0], iv_join(iv, (e[1], e[2])),
                                         old[2], old[3])
                    return
            got = self.indexable()
            if got:
                name, length = got
                _, iv, _, is_char = self.arrays[name]
                cap = 127 if is_char else 255
                e = self.capped(depth, cap)
                if rng.random() < 0.6:
                    k = rng.randrange(length)
                    lines.append(f"{name}[{k}] = {e[0]};")
                else:
                    idx = self.expr(depth - 1)
                    if 0 <= idx[1] and idx[2] < length:
                        lines.append(f"{name}[{idx[0]}] = {e[0]};")
                    else:
                        return
                self.arrays[name] = (length, iv_join(iv, (e[1], e[2])),
                                     self.arrays[name][2], is_char)
                return
        if roll < 0.47 and self.fnptr_arrays and rng.random() < 0.3:
            # Rebind one array element to the helper or to
            # null, updating only that element's binding.
            name = rng.choice(list(self.fnptr_arrays))
            k = rng.randrange(len(self.fnptr_arrays[name]))
            if rng.random() < 0.6:
                lines.append(f"{name}[{k}] = helper;")
                self.fnptr_arrays[name][k] = "helper"
            else:
                lines.append(f"{name}[{k}] = 0;")
                self.fnptr_arrays[name][k] = None
            return
        if roll < 0.47 and self.fnptrs and rng.random() < 0.3:
            name = rng.choice(list(self.fnptrs))
            choice = rng.random()
            if choice < 0.45:
                lines.append(f"{name} = helper;")
                self.fnptrs[name] = "helper"
            elif choice < 0.7:
                lines.append(f"{name} = 0;")
                self.fnptrs[name] = None
            elif len(self.fnptrs) > 1:
                other = rng.choice(
                    [n for n in self.fnptrs if n != name])
                lines.append(f"{name} = {other};")
                self.fnptrs[name] = self.fnptrs[other]
            else:
                cond = self.expr(depth - 1)
                lines.append(f"{name} = ({cond[0]} ? helper : 0);")
                self.fnptrs[name] = "?"
            return
        if roll < 0.47 and self.pptrs and rng.random() < 0.6:
            ppname = rng.choice(list(self.pptrs))
            tgt = self.pptrs[ppname]
            kind, ultimate, is_char = self.ptrs[tgt]
            cap = 127 if is_char else 255
            e = self.capped(depth, cap)
            length = self.ptr_target_len(ppname)
            if length is not None and rng.random() < 0.65:
                form = None
                if rng.random() < 0.6:
                    k = rng.randrange(length)
                    form = rng.choice([f"(*{ppname})[{k}]",
                                       f"{ppname}[0][{k}]"])
                else:
                    idx = self.expr(depth - 1)
                    if 0 <= idx[1] and idx[2] < length:
                        form = rng.choice([f"(*{ppname})[{idx[0]}]",
                                           f"{ppname}[0][{idx[0]}]"])
                if form is not None:
                    lines.append(f"{form} = {e[0]};")
                    old = self.arrays[ultimate]
                    self.arrays[ultimate] = (
                        old[0], iv_join(old[1], (e[1], e[2])), old[2], old[3])
                    return
            if kind == "var" and rng.random() < 0.25:
                lo, hi = self.env[ultimate]
                if hi < cap:
                    lines.append(f"(**{ppname})++;")
                    self.env[ultimate] = (lo + 1, hi + 1)
                    return
            lines.append(f"**{ppname} = {e[0]};")
            if kind == "var":
                self.env[ultimate] = (e[1], e[2])
            else:
                old = self.arrays[ultimate]
                self.arrays[ultimate] = (
                    old[0], iv_join(old[1], (e[1], e[2])), old[2], old[3])
            return
        if roll < 0.47 and self.ptrs:
            pname = rng.choice(list(self.ptrs))
            kind, target, is_char = self.ptrs[pname]
            cap = 127 if is_char else 255
            e = self.capped(depth, cap)
            if kind == "var":
                lines.append(f"*{pname} = {e[0]};")
                self.env[target] = (e[1], e[2])
            else:
                if kind == "arrayrow":
                    length = self.mdims[target][1]
                else:
                    length = self.arrays[target][0]
                k = rng.randrange(length)
                lines.append(f"{pname}[{k}] = {e[0]};")
                old = self.arrays[target]
                self.arrays[target] = (old[0], iv_join(old[1], (e[1], e[2])),
                                        old[2], old[3])
            return
        if roll < 0.52 and self.assignable():
            name = rng.choice(self.assignable())
            cap = 127 if name in self.chartypes else 255
            lo, hi = self.env[name]
            if hi < cap:
                lines.append(f"{name}++;")
                self.env[name] = (lo + 1, hi + 1)
            elif lo > 0:
                lines.append(f"{name}--;")
                self.env[name] = (lo - 1, hi - 1)
            return
        if roll < 0.62:
            self.gen_if(depth, loop, lines, pure, allow_while, allow_loop)
            return
        if roll < 0.74:
            self.gen_loop(depth, lines, allow_while)
            return
        if roll < 0.8 and self.env:
            self.gen_switch(lines, loop, pure)
            return
        if roll < 0.84 and loop is not None:
            # `continue` in a while body would skip the generated
            # increment and hang; it is only safe in `for` loops,
            # whose update still runs.
            cond = self.expr(1)
            if loop == "for" and rng.random() < 0.5:
                lines.append(f"if ({cond[0]}) continue;")
            else:
                lines.append(f"if ({cond[0]}) break;")
            return
        self.gen_printf(lines)

    def gen_goto_segment(self, lines, depth):
        """One forward `goto` segment, as a direct child of
        `main`'s block: a conditional or unconditional jump over
        one or two `printf`s to a label carrying one scalar
        assignment. Only one segment per program, so at most one
        flag cell is added. The skipped statements are prints
        only, so no initialization is jumped over (C++ rejects
        that), the label is defined later in the same block, and
        the label assignment runs on both paths with the
        environment the prints leave unchanged — the interval
        update is unconditional and sound in both models."""
        rng = self.rng
        label = self.fresh("gl")
        self.goto_used = True
        if rng.random() < 0.5:
            cond = self.expr(1)
            if rng.random() < 0.5:
                lines.append(f"if ({cond[0]}) {{ goto {label}; }}")
            else:
                lines.append(f"if ({cond[0]}) goto {label};")
        else:
            lines.append(f"goto {label};")
        for _ in range(rng.randint(1, 2)):
            self.gen_printf(lines)
        name = rng.choice(self.assignable())
        cap = 127 if name in self.chartypes else 255
        e = self.capped(depth, cap)
        lines.append(f"{label}: {name} = {e[0]};")
        self.env[name] = (e[1], e[2])

    def gen_if(self, depth, loop, lines, pure=False, allow_while=True,
               allow_loop=True):
        snap = self.snap()
        cond = self.expr(1)
        then_lines = []
        # Branches are braced, so a while counter declared inside
        # one is scoped to it: always allowed there.
        self.stmts(depth - 1, loop, 2, then_lines, pure, allow_while=True,
                   allow_loop=allow_loop)
        then_snap = self.snap()
        # Copies: restoring the snapshot dict itself and then
        # mutating it would corrupt the snapshot the merge reads.
        self.env, self.arrays, self.chartypes, self.fnptrs, self.fnptr_arrays = dict(snap[0]), dict(snap[1]), set(snap[2]), dict(snap[3]), {k: list(v) for k, v in snap[4].items()}
        else_lines = []
        self.stmts(depth - 1, loop, 1, else_lines, pure, allow_while=True,
                   allow_loop=allow_loop)
        self.merge([snap, then_snap, self.snap()])
        lines.append(f"if ({cond[0]}) {{")
        lines.extend("  " + s for s in then_lines)
        lines.append("} else {")
        lines.extend("  " + s for s in else_lines)
        lines.append("}")

    def gen_loop(self, depth, lines, allow_while=True):
        rng = self.rng
        # A while counter is declared at statement level; directly
        # under a switch case label that is a jump over its
        # initialization, which C++ rejects. For loops scope
        # their counter and are fine there.
        if allow_while:
            kind = rng.choice(["for", "while", "dowhile"])
        else:
            kind = "for"
        bound = rng.randint(2, 6)
        if kind == "for":
            ivar = self.fresh("i")
            start = rng.randint(0, 2)
            self.protected.add(ivar)
            self.env[ivar] = (start, start + bound)
            body = []
            self.stmts(depth - 1, "for", 2, body, pure=True)
            del self.env[ivar]
            self.protected.discard(ivar)
            lines.append(f"for (int {ivar} = {start}; {ivar} < {start + bound}; {ivar}++) {{")
            lines.extend("  " + s for s in body)
            lines.append("}")
            return
        cvar = self.fresh("c")
        snap = self.snap()
        # The counter ranges 0..bound inside the body; reasoning
        # with (0, 0) would under-approximate reads of it.
        self.env[cvar] = (0, bound)
        self.protected.add(cvar)
        body = []
        self.stmts(depth - 1, "while", 2, body, pure=True)
        self.protected.discard(cvar)
        self.merge([snap, self.snap()])
        # The counter is declared with the loop and leaves scope
        # with it; later statements must not reference it.
        self.env.pop(cvar, None)
        if kind == "while":
            lines.append(f"int {cvar} = 0;")
            lines.append(f"while ({cvar} < {bound}) {{")
            lines.extend("  " + s for s in body)
            lines.append(f"  {cvar}++;")
            lines.append("}")
        else:
            lines.append(f"int {cvar} = 0;")
            lines.append("do {")
            lines.extend("  " + s for s in body)
            lines.append(f"  {cvar}++;")
            lines.append(f"}} while ({cvar} < {bound});")

    def gen_switch(self, lines, loop, pure=False):
        rng = self.rng
        names = self.assignable()
        if not names:
            return
        name = rng.choice(names)
        lo, hi = self.env[name]
        labels = sorted({rng.randint(lo, hi), rng.randint(lo, hi), lo})
        snap = self.snap()
        snaps = [snap]
        lines.append(f"switch ({name}) {{")
        for lab in labels:
            lines.append(f"case {lab}:")
            body = []
            self.stmts(1, loop, 1, body, pure, allow_while=False,
                       allow_loop=False)
            lines.extend("  " + s for s in body)
            lines.append("  break;")
            snaps.append(self.snap())
            self.env, self.arrays, self.chartypes, self.fnptrs, self.fnptr_arrays = dict(snap[0]), dict(snap[1]), set(snap[2]), dict(snap[3]), {k: list(v) for k, v in snap[4].items()}
        lines.append("default:")
        body = []
        self.stmts(1, loop, 1, body, pure, allow_while=False,
                   allow_loop=False)
        lines.extend("  " + s for s in body)
        lines.append("  break;")
        snaps.append(self.snap())
        lines.append("}")
        self.merge(snaps)

    def stmts(self, depth, loop, n, lines, pure=False, allow_while=True,
              allow_loop=True, allow_goto=False):
        for _ in range(n):
            self.stmt(depth, loop, lines, pure, allow_while, allow_loop,
                      allow_goto)

    def chain_read(self, depth):
        """A `(*pp)[k]` / `pp[0][k]` read, or None when no
        pointer-to-pointer has an array ultimate, or the index
        does not fit in bounds."""
        rng = self.rng
        eligible = [n for n in self.pptrs
                    if self.ptr_target_len(n) is not None]
        if not eligible:
            return None
        ppname = rng.choice(eligible)
        length = self.ptr_target_len(ppname)
        iv = self.ptr_target_iv(ppname)
        if rng.random() < 0.6:
            k = rng.randrange(length)
            form = rng.choice([f"(*{ppname})[{k}]",
                               f"{ppname}[0][{k}]"])
            return (form, *iv)
        idx = self.expr(depth - 1)
        if 0 <= idx[1] and idx[2] < length:
            form = rng.choice([f"(*{ppname})[{idx[0]}]",
                               f"{ppname}[0][{idx[0]}]"])
            return (form, *iv)
        return None

    def gen_printf(self, lines):
        rng = self.rng
        parts = []
        args = []
        for _ in range(rng.randint(1, 3)):
            if rng.random() < 0.15:
                got = self.chain_read(2)
                if got is not None:
                    parts.append("%d")
                    args.append(got[0])
                    continue
            roll = rng.random()
            if roll < 0.14:
                strings = [n for n, a in self.arrays.items() if a[2]]
                if strings and rng.random() < 0.7:
                    parts.append("%s")
                    args.append(rng.choice(strings))
                else:
                    word = "".join(rng.choice("abcdefgxyz") for _ in range(rng.randint(1, 4)))
                    parts.append("%s")
                    args.append(f'"{word}"')
                continue
            if roll < 0.22:
                parts.append("%c")
                args.append(f"'{rng.choice('abcdefgxyz .,!?')}'")
                continue
            if roll < 0.26:
                parts.append("%%")
                continue
            letter = rng.choice(["d", "x", "X", "o", "u"])
            prefix = rng.choice(["", "", "l", "hh", "ll", "h", "z", "j", "t"])
            if prefix == "hh" and letter == "d":
                # %hhd narrows to signed char in C, so values
                # above 127 print negative there and unsigned
                # here (length modifiers are ignored): cap the
                # argument. No other modifier narrows a byte value.
                e = self.capped(2, 127)
            else:
                e = self.readable_expr(2)
            parts.append(f"%{prefix}{letter}")
            args.append(e[0])
        fmt = ", ".join(parts)
        if args:
            lines.append(f'printf("{fmt}\\n", {", ".join(args)});')
        else:
            lines.append(f'printf("{fmt}\\n");')

    def gen_scanf(self):
        rng = self.rng
        decls = []
        specs = []
        args = []
        tokens = []
        for _ in range(rng.randint(2, 3)):
            name = self.fresh("s")
            decls.append(f"int {name} = 0;")
            self.env[name] = (0, 200)
            conv = rng.choice(["d", "x", "i", "o", "u"])
            value = rng.randint(0, 200)
            if conv in ("d", "u"):
                token = str(value)
            elif conv == "x":
                token = format(value, "x" if rng.random() < 0.7 else "X")
            elif conv == "o":
                token = format(value, "o")
            else:
                style = rng.random()
                if style < 0.4:
                    token = str(value)
                elif style < 0.7:
                    token = "0x" + format(value, "x")
                else:
                    token = "0" + format(value, "o")
            specs.append(f"%{conv}")
            args.append(f"&{name}")
            tokens.append(token)
        line = f'scanf("{" ".join(specs)}", {", ".join(args)});'
        return decls, line, " ".join(tokens)

    def gen_scanf_string(self):
        """One `%s` conversion into an existing one-dimensional
        char array, with a word short enough to fit: the token
        plus NUL fit in the array in both models, and the
        rest of the array keeps its previous contents in
        both. At most one scanf kind appears per program, so
        the terminator-consumed deviation can never affect
        a later `%c` conversion. Returns (line, token) or
        None when no suitable array exists."""
        rng = self.rng
        cands = [n for n, a in self.arrays.items()
                 if a[3] and n not in self.mdims and a[0] >= 2]
        if not cands:
            return None
        name = rng.choice(cands)
        length, iv, is_string, _ = self.arrays[name]
        word = "".join(rng.choice("abcdefgxyz")
                       for _ in range(rng.randint(1, length - 1)))
        ords = [ord(c) for c in word]
        new_iv = iv_join(iv_join(iv, (min(ords), max(ords))), (0, 0))
        self.arrays[name] = (length, new_iv, is_string, True)
        return f'scanf("%s", {name});', word

    def gen_scanf_char(self):
        """One `%c` conversion as the program's only scanf:
        it reads exactly one letter, whose value is the new
        variable's exact interval. Keeping it the only scanf
        kind avoids every documented `%c` deviation (no
        preceding number or string conversion whose
        terminator handling could differ)."""
        rng = self.rng
        name = self.fresh("s")
        ch = rng.choice("abcdefgxyz")
        self.env[name] = (ord(ch), ord(ch))
        self.chartypes.add(name)
        return [f"char {name} = 0;"], f'scanf("%c", &{name});', ch

    def gen_helper(self):
        rng = self.rng
        if rng.random() < 0.25:
            # A `**` helper: the body needs no generated
            # state, so no save/restore is involved. The
            # return interval is the wide bound for the
            # ultimate target; callers narrow it with fits()
            # and capped() retries like any wide interval.
            is_char = rng.random() < 0.5
            typ = "char" if is_char else "int"
            self.helpers["helper"] = ((0, 127) if is_char else (0, 255),
                                      True, is_char)
            return (f"{typ} helper({typ} **hp) {{\n"
                    f"  return **hp;\n}}")
        saved = (self.env, self.arrays, self.chartypes, self.ptrs,
                 self.mdims, self.pptrs, self.fnptrs, self.fnptr_arrays)
        self.env = {"hp": (0, 255)}
        self.arrays = {}
        self.chartypes = set()
        self.ptrs = {}
        self.mdims = {}
        self.pptrs = {}
        self.fnptrs = {}
        self.fnptr_arrays = {}
        lines = []
        if rng.random() < 0.6:
            lines.append(self.decl_scalar())
        ret = self.readable_expr(2)
        (self.env, self.arrays, self.chartypes, self.ptrs,
         self.mdims, self.pptrs, self.fnptrs, self.fnptr_arrays) = saved
        self.helpers["helper"] = ((ret[1], ret[2]), False, False)
        body = "\n".join("  " + l for l in lines)
        tail = f"  return {ret[0]};"
        return f"int helper(int hp) {{\n{body}\n{tail}\n}}"


def gen_program(seed):
    rng = random.Random(seed)
    g = Gen(rng)
    chunks = ["#include <stdio.h>", ""]
    if rng.random() < 0.35:
        chunks.append(g.gen_helper())
        chunks.append("")
    if rng.random() < 0.3:
        chunks.append(g.gen_typedef())
        chunks.append("")
    lines = []
    for _ in range(rng.randint(2, 4)):
        roll = rng.random()
        if roll < 0.42:
            lines.append(g.decl_scalar())
        elif roll < 0.66:
            lines.append(g.decl_array())
        elif roll < 0.78:
            decl = g.decl_pointer()
            lines.append(decl if decl else g.decl_scalar())
        elif roll < 0.86 and not g.mdims:
            lines.append(g.decl_md_array())
        elif roll < 0.86:
            lines.append(g.decl_array())
        elif roll < 0.88 and not g.fnptr_arrays:
            decl = g.decl_fnptr_array()
            lines.append(decl if decl else g.decl_scalar())
        else:
            decl = None
            if roll < 0.93 and len(g.fnptrs) < 2:
                decl = g.decl_fnptr()
            if decl is None:
                decl = g.decl_pp()
            lines.append(decl if decl else g.decl_scalar())
    # A function-pointer variable needs the helper to point
    # at; bind one afterwards when none was declared, so
    # indirect calls get steady coverage.
    if not g.fnptrs and rng.random() < 0.4:
        decl = g.decl_fnptr()
        if decl is not None:
            lines.append(decl)
    # A pointer-to-pointer declaration needs a pointer declared
    # before it, which the loop above only sometimes provides;
    # bind one afterwards half the time a pointer exists, so
    # dereference chains get steady coverage.
    if g.ptrs and not g.pptrs and rng.random() < 0.5:
        decl = g.decl_pp()
        if decl:
            lines.append(decl)
    stdin_parts = []
    scan_roll = rng.random()
    if scan_roll < 0.4:
        decls, line, tokens = g.gen_scanf()
        lines.extend(decls)
        lines.append(line)
        stdin_parts.append(tokens)
    elif scan_roll < 0.58:
        got = g.gen_scanf_string()
        if got is not None:
            line, tokens = got
            lines.append(line)
            stdin_parts.append(tokens)
    elif scan_roll < 0.7:
        decls, line, tokens = g.gen_scanf_char()
        lines.extend(decls)
        lines.append(line)
        stdin_parts.append(tokens)
    g.stmts(2, None, rng.randint(4, 8), lines, allow_goto=True)
    g.gen_printf(lines)
    if rng.random() < 0.5:
        g.gen_printf(lines)
    body = "\n".join("  " + line for line in lines)
    chunks.append(f"int main() {{\n{body}\n}}")
    src = "\n".join(chunks) + "\n"
    if stdin_parts:
        # About one program in five ends its input at EOF, so
        # number/string conversions must terminate there (zero
        # sentinel); the rest keep the trailing newline.
        stdin = "\n".join(stdin_parts) + ("\n" if rng.random() < 0.8 else "")
    else:
        stdin = ""
    return src, stdin


# --- driver ----------------------------------------------------------

class FuzzFailure(Exception):
    pass


class GeneratorFailure(Exception):
    pass


def run_capture(cmd, cwd, stdin_bytes):
    return subprocess.run(
        cmd, cwd=cwd, input=stdin_bytes,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=TIMEOUT,
    )


def run_seed(seed, keep_dir=None, verbose=False):
    src, stdin_text = gen_program(seed)
    work = tempfile.mkdtemp(prefix=f"c2bf-fuzz-{seed}-")
    try:
        c_path = os.path.join(work, "prog.c")
        in_path = os.path.join(work, "prog.in")
        with open(c_path, "w", encoding="ascii") as f:
            f.write(src)
        stdin_bytes = stdin_text.encode("ascii")
        with open(in_path, "wb") as f:
            f.write(stdin_bytes)
        got = run_capture(["g++", "-w", "prog.c", "-o", "prog.o"], work, b"")
        if got.returncode != 0:
            raise GeneratorFailure(
                f"seed {seed}: g++ rejected the generated program:\n"
                f"{got.stderr.decode('latin-1')}\n{src}")
        got = run_capture(["./prog.o"], work, stdin_bytes)
        if got.returncode != 0:
            raise GeneratorFailure(
                f"seed {seed}: oracle exited {got.returncode}:\n{src}")
        expected = got.stdout
        got = run_capture([C2BF_BIN, "compile", "-c", "prog.c", "-o", "prog.bf"],
                          work, b"")
        if got.returncode != 0:
            raise FuzzFailure(
                f"seed {seed}: c2bf rejected the program:\n"
                f"{got.stdout.decode('latin-1')}{got.stderr.decode('latin-1')}\n{src}")
        got = run_capture([C2BF_BIN, "run", "-c", "prog.bf", "-i", "prog.in"],
                          work, b"")
        if got.returncode != 0:
            raise FuzzFailure(
                f"seed {seed}: c2bf run exited {got.returncode}:\n{src}")
        if got.stdout != expected:
            exp = expected.decode("latin-1").splitlines()
            act = got.stdout.decode("latin-1").splitlines()
            diff = "\n".join(difflib.unified_diff(
                exp, act, "oracle", "c2bf", lineterm=""))
            raise FuzzFailure(
                f"seed {seed}: output mismatch\n--- program ---\n{src}"
                f"--- stdin ---\n{stdin_text}--- diff ---\n{diff}")
        if verbose:
            print(f"seed {seed}: ok")
        if keep_dir:
            shutil.copytree(work, os.path.join(keep_dir, f"seed-{seed}"),
                            dirs_exist_ok=True)
        return True
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main(argv=None):
    ap = argparse.ArgumentParser(description="Differential fuzzer (see docs/fuzzing.md).")
    ap.add_argument("--count", type=int, default=50)
    ap.add_argument("--seed-base", type=int, default=0)
    ap.add_argument("--seeds", default="",
                    help="Comma-separated seeds; overrides --count/--seed-base.")
    ap.add_argument("--keep-dir", default=None,
                    help="Keep every generated case under this directory.")
    ap.add_argument("--continue-on-failure", action="store_true")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args(argv)
    if args.seeds:
        seeds = [int(s) for s in args.seeds.split(",") if s.strip()]
    else:
        seeds = list(range(args.seed_base, args.seed_base + args.count))
    if args.keep_dir:
        os.makedirs(args.keep_dir, exist_ok=True)
    failures = []
    skipped = []
    ran = 0
    for seed in seeds:
        ran += 1
        try:
            run_seed(seed, args.keep_dir, args.verbose)
        except GeneratorFailure as e:
            print(f"GENERATOR BUG: {e}", file=sys.stderr)
            return 2
        except FuzzFailure as e:
            msg = str(e)
            # The rejection message is seed-prefixed
            # ("seed N: c2bf rejected the program:\n<stderr>").
            if "c2bf rejected the program" in msg and LIMIT_MSG in msg:
                # Documented limit (docs/semantics.md), not a
                # wrong output: report and keep going; the cap
                # below bounds how many a run may skip.
                print(f"FUZZ SKIP: seed {seed} (address limit)",
                      file=sys.stderr)
                skipped.append(seed)
                continue
            print(f"FUZZ FAILURE: {e}", file=sys.stderr)
            failures.append(seed)
            if not args.continue_on_failure:
                break
        except subprocess.TimeoutExpired as e:
            print(f"FUZZ FAILURE: seed {seed}: timeout in {e.cmd}",
                  file=sys.stderr)
            failures.append(seed)
            if not args.continue_on_failure:
                break
    if len(seeds) > 1:
        over_cap = len(skipped) > max(1, len(seeds) // SKIP_CAP_FRAC)
    else:
        over_cap = bool(skipped)
    print(f"fuzz: {ran - len(failures) - len(skipped)}/{ran} seeds passed"
          + (f"; {len(skipped)} skipped (address limit): {skipped}" if skipped else "")
          + (f"; failures: {failures}" if failures else ""))
    if over_cap:
        print(f"fuzz: skip cap exceeded ({len(skipped)} skipped)",
              file=sys.stderr)
    return 1 if failures or over_cap else 0


if __name__ == "__main__":
    sys.exit(main())
