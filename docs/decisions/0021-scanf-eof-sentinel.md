# 0021 — `scanf` numbers ending at EOF use the zero sentinel

Date: 2026-11-09. Status: accepted.

## Context

The decimal and based `scanf` readers looped while the input
cell held a digit (or a valid digit for the base), and each
loop ended with a bare `,` read. The runner leaves the cell
unchanged at EOF, so a number ending exactly at end of input
— no trailing newline or other terminator — re-read its last
digit forever and the program hung. Every existing fixture
dodged this with a trailing newline, and the `goto` spike
(0020) tripped over it in `goto_cleanup`/`goto_dowhile` before
the inputs were newline-terminated. `%s` never hung: its
reader presets the cell to zero before each read, so a zero
byte means EOF. `%c` never hung either: it reads once, and at
EOF the destination keeps its previous value, as in C.

## Decision: the `%s` sentinel, in the number readers

Apply the `%s` reader's pattern to `read_as_number`
(`%d`/`%u`) and `read_as_number_base` (`%o`/`%x`/`%i`):
every read presets the cell to zero with `read_char_into`,
the decimal skip loop additionally gates on the cell being
nonzero, and the based reader's hand-rolled skip loop clears
its gate cell when the preset read stays zero. The digit
loops need no extra gate: zero is not a digit in any base.
The runner's EOF semantics do not change (the cell stays
unchanged at EOF, pinned by `tests/run/eof.bf`), and `%c`
is untouched. A NUL byte in the input reads as end of input,
exactly as `%s` already documented; that is the sentinel's
known limit, shared by all four readers now.

## Scope

Fixed: `%d`, `%u`, `%o`, `%x`, `%i`, including assignment
suppression and through-pointer destinations (they share the
readers). A number with no digits at EOF reads zero, matching
the terminated-input behavior. Pinned by the `scanf_eof`
oracle fixture (numbers ending at EOF, then a `%s` at EOF)
and unit tests for each conversion, bare EOF, leading
junk, and a second conversion ending at EOF. The fuzzer's
stdin now sometimes omits its trailing newline, so sweeps
exercise EOF-terminated conversions. Deferred: none — the
sentinel covers every looping reader; only `%c`'s
single-read shape remains, and it cannot hang.
