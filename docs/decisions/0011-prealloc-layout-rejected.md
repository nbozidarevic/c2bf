# 0011 — Pre-allocating variable cells rejected

Date: 2026-11-04. Status: rejected (spike measured, not landed).

## Context

The bump allocator hands out variable, array, and temporary
cells in emission order, so a variable declared late sits far
up the tape among late temporaries. The spike
(`spike/cell-layout`, branch-scoped measurement workflow)
reserved every variable/array cell up front — a walk over each
renamed function body and the file-scope declarations, calling
`var_cell` / `alloc_array` before any code was emitted — so
storage clustered at low indices and temporaries started above
all of it. Allocation is memoized, so emission reuses the
reserved cells.

## Decision

Rejected. The spike passed all unit tests and the differential
/ g++ oracle suite, but grew total emitted code from 10,969,857
bytes (post-`perform_if` baseline) to 11,296,885 (+327,028,
+3.0%). Only a handful of fixtures held or shrank; the grows
were broad (`bitwise` +90,538, `continue` +53,960, `assign`
+16,584, `switch` +7,450), with allocated cells essentially
unchanged. The current interleaving is locality-optimal for
straight-line code: a variable sits near the temporaries of
the statements around its declaration, and moving all storage
low lengthens the average travel to it. An early spike version
also flipped the array/variable kind-confusion diagnostic
(pre-allocation recorded both kinds before emission saw either);
the fix — skip a declarator whose name is already recorded in
the opposite kind — is recorded here as a constraint on any
future pre-pass over declarations.

## Consequences

- Nothing landed on `master`; the spike branch was deleted with
  its measurement workflow. No golden, perf, or size-history
  refresh was needed.
- Cell layout is not pursued further in this batch: the
  allocator's emission-order layout stays, and layout-sensitive
  changes must bring a mechanism (as in 0009) plus
  fixture-by-fixture measurement with `strings`, `scanfstr`,
  and `prototypes` as canaries.
