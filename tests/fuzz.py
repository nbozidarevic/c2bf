#!/usr/bin/env python3
"""Differential fuzzer: random C-subset programs vs the g++ oracle.

Each seed generates one program plus stdin, compiles it with g++
(the oracle, exactly as `tests/test` does) and with `c2bf`, runs
both, and compares stdout byte-for-byte. A seed that mismatches
fails loudly with the seed, the program, and the input, so the
case reproduces with `--seeds <seed>`.

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

C2BF_BIN = os.environ.get("C2BF_BIN", "../rs/target/release/c2bf")
TIMEOUT = 15


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
    `arrays` maps names to (length, (lo, hi), is_string)."""

    def __init__(self, rng):
        self.rng = rng
        self.env = {}
        self.arrays = {}
        self.ptrs = {}
        self.chartypes = set()
        self.helpers = {}
        self.protected = set()
        self.counter = 0

    def fresh(self, stem):
        self.counter += 1
        return f"{stem}{self.counter}"

    # --- pure expressions: (text, lo, hi) ---------------------------

    def expr(self, depth, allow_call=True):
        rng = self.rng
        if depth <= 0 or rng.random() < 0.35:
            return self.leaf()
        roll = rng.random()
        if roll < 0.55:
            return self.binop(depth)
        if roll < 0.68:
            return self.ternary(depth)
        if roll < 0.78 and allow_call and self.helpers:
            return self.call(depth)
        if roll < 0.86:
            inner = self.expr(depth - 1, allow_call)
            return (f"({inner[0]})", inner[1], inner[2])
        return self.unop(depth)

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

    def binop(self, depth):
        rng = self.rng
        op = rng.choice(BINOPS)
        for _ in range(6):
            l = self.expr(depth - 1)
            r = self.expr(depth - 1)
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
                return (f"({l[0]} << {k})", *iv)
            elif op == ">>":
                if r[2] > 4 or r[1] < 0:
                    continue
                k = rng.randint(0, 4)
                iv = iv_shr(l[1:], k)
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

    def ternary(self, depth):
        cond = self.expr(depth - 1)
        a = self.expr(depth - 1)
        b = self.expr(depth - 1)
        iv = iv_join(a[1:], b[1:])
        if not fits(*iv):
            return a if fits(*a[1:]) else self.leaf()
        return (f"({cond[0]} ? {a[0]} : {b[0]})", *iv)

    def call(self, depth):
        name, ret_iv = self.rng.choice(list(self.helpers.items()))
        arg = self.expr(depth - 1, allow_call=False)
        return (f"{name}({arg[0]})", *ret_iv)

    def capped(self, depth, cap):
        """Expression whose interval fits in [0, cap] (for `char`
        targets: C converts larger values implementation-defined,
        so only 0..127 may flow into one)."""
        for _ in range(8):
            e = self.expr(depth)
            if e[2] <= cap:
                return e
        v = self.rng.randint(0, cap)
        return (str(v), v, v)

    def unop(self, depth):
        inner = self.expr(depth - 1)
        roll = self.rng.random()
        if roll < 0.45:
            return (f"(!{inner[0]})", 0, 1)
        if roll < 0.7 and inner[2] <= 127:
            # A cast to char is the identity only below 128;
            # above that C sign-extends and the models diverge.
            return (f"((char){inner[0]})", *inner[1:])
        # sizeof only for char-typed storage: sizeof(int) is 4
        # in C but 1 here, while sizeof(char) is 1 in both, and
        # sizeof(char[N]) is N in both.
        cands = [n for n in self.chartypes]
        if cands and roll < 0.85:
            return (f"(sizeof({self.rng.choice(cands)}))", 1, 1)
        char_arrays = [n for n, a in self.arrays.items() if a[3]]
        if char_arrays:
            name = self.rng.choice(char_arrays)
            return (f"(sizeof({name}))", self.arrays[name][0], self.arrays[name][0])
        return (f"(!{inner[0]})", 0, 1)

    def indexable(self):
        """(name, length) of an array usable for reads/writes."""
        if not self.arrays:
            return None
        name = self.rng.choice(list(self.arrays))
        length = self.arrays[name][0]
        return (name, length)

    def readable_expr(self, depth):
        """Expression usable as an rvalue, including array reads;
        intervals still hold."""
        rng = self.rng
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
            length = self.arrays[target][0]
            iv = self.arrays[target][1]
            k = rng.randrange(length)
            return (f"{pname}[{k}]", *iv)
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
            elems = [self.capped(2, cap) for _ in range(rng.randint(1, n))]
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
            self.ptrs[name] = ("array", target, is_char)
            typ = "char" if is_char else "int"
            return f"{typ} *{name} = {target};"
        names = [n for n in self.env if n not in self.protected]
        if not names:
            return None
        target = rng.choice(names)
        is_char = target in self.chartypes
        self.ptrs[name] = ("var", target, is_char)
        typ = "char" if is_char else "int"
        return f"{typ} *{name} = &{target};"

    # --- statements -------------------------------------------------

    def snap(self):
        return (dict(self.env), {k: v for k, v in self.arrays.items()},
                set(self.chartypes))

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

    def assignable(self):
        return [n for n in self.env if n not in self.protected]

    def stmt(self, depth, loop, lines, pure=False, allow_while=True):
        rng = self.rng
        roll = rng.random()
        if pure:
            # Loop bodies never change persistent state: reasoning
            # about intervals across iterations would need a
            # fixpoint, so bodies only print or control flow.
            roll = 0.62 + roll * 0.38
        if depth <= 0 and roll >= 0.62:
            # Depth exhausted: no further nesting (if/loop/switch
            # would recurse without bound, since stmts() clamps
            # nothing here); fall through to the print branch.
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
        if roll < 0.47 and self.ptrs:
            pname = rng.choice(list(self.ptrs))
            kind, target, is_char = self.ptrs[pname]
            cap = 127 if is_char else 255
            e = self.capped(depth, cap)
            if kind == "var":
                lines.append(f"*{pname} = {e[0]};")
                self.env[target] = (e[1], e[2])
            else:
                length = self.arrays[target][0]
                k = rng.randrange(length)
                lines.append(f"{pname}[{k}] = {e[0]};")
                old = self.arrays[target]
                self.arrays[target] = (length, iv_join(old[1], (e[1], e[2])),
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
            self.gen_if(depth, loop, lines, pure, allow_while)
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

    def gen_if(self, depth, loop, lines, pure=False, allow_while=True):
        snap = self.snap()
        cond = self.expr(1)
        then_lines = []
        # Branches are braced, so a while counter declared inside
        # one is scoped to it: always allowed there.
        self.stmts(depth - 1, loop, 2, then_lines, pure, allow_while=True)
        then_snap = self.snap()
        # Copies: restoring the snapshot dict itself and then
        # mutating it would corrupt the snapshot the merge reads.
        self.env, self.arrays, self.chartypes = dict(snap[0]), dict(snap[1]), set(snap[2])
        else_lines = []
        self.stmts(depth - 1, loop, 1, else_lines, pure, allow_while=True)
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
            self.stmts(1, loop, 1, body, pure, allow_while=False)
            lines.extend("  " + s for s in body)
            lines.append("  break;")
            snaps.append(self.snap())
            self.env, self.arrays, self.chartypes = dict(snap[0]), dict(snap[1]), set(snap[2])
        lines.append("default:")
        body = []
        self.stmts(1, loop, 1, body, pure, allow_while=False)
        lines.extend("  " + s for s in body)
        lines.append("  break;")
        snaps.append(self.snap())
        lines.append("}")
        self.merge(snaps)

    def stmts(self, depth, loop, n, lines, pure=False, allow_while=True):
        for _ in range(n):
            self.stmt(depth, loop, lines, pure, allow_while)

    def gen_printf(self, lines):
        rng = self.rng
        parts = []
        args = []
        for _ in range(rng.randint(1, 3)):
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

    def gen_helper(self):
        rng = self.rng
        saved = (self.env, self.arrays, self.chartypes, self.ptrs)
        self.env = {"hp": (0, 255)}
        self.arrays = {}
        self.chartypes = set()
        self.ptrs = {}
        lines = []
        if rng.random() < 0.6:
            lines.append(self.decl_scalar())
        ret = self.readable_expr(2)
        self.env, self.arrays, self.chartypes, self.ptrs = saved
        self.helpers["helper"] = (ret[1], ret[2])
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
    lines = []
    for _ in range(rng.randint(2, 4)):
        roll = rng.random()
        if roll < 0.5:
            lines.append(g.decl_scalar())
        elif roll < 0.8:
            lines.append(g.decl_array())
        else:
            decl = g.decl_pointer()
            lines.append(decl if decl else g.decl_scalar())
    stdin_parts = []
    if rng.random() < 0.4:
        decls, line, tokens = g.gen_scanf()
        lines.extend(decls)
        lines.append(line)
        stdin_parts.append(tokens)
    g.stmts(2, None, rng.randint(4, 8), lines)
    g.gen_printf(lines)
    if rng.random() < 0.5:
        g.gen_printf(lines)
    body = "\n".join("  " + line for line in lines)
    chunks.append(f"int main() {{\n{body}\n}}")
    src = "\n".join(chunks) + "\n"
    stdin = ("\n".join(stdin_parts) + "\n") if stdin_parts else ""
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
    for seed in seeds:
        try:
            run_seed(seed, args.keep_dir, args.verbose)
        except GeneratorFailure as e:
            print(f"GENERATOR BUG: {e}", file=sys.stderr)
            return 2
        except FuzzFailure as e:
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
    ran = len(seeds) if not failures else seeds.index(failures[0]) + 1
    print(f"fuzz: {ran - len(failures)}/{ran} seeds passed"
          + (f"; failures: {failures}" if failures else ""))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
