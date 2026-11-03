#!/usr/bin/env python3
"""Regenerate docs/size-history.md from committed golden BF sizes.

Usage (from anywhere inside the checkout):

    python3 tools/size-history.py [--output PATH] [--check]

Method: for each milestone commit below, read the byte sizes of
``tests/golden/bf/*.bf`` with ``git ls-tree -r -l``. No code is
checked out and nothing is built, so the history is reproducible
from the repository alone. Sizes are golden-blob sizes: the exact
bytes the transpiler emitted for each fixture at that commit.

Milestones are curated, not auto-detected: add a new entry (full
SHA, short label, note) at the end of MILESTONES and rerun. Use a
commit whose tree holds the goldens you want to record (usually a
snapshot commit); the script records the sizes as committed there,
including any fixture edits in that same commit.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

# (full SHA, label, note) in chronological order.
MILESTONES = [
    (
        "a2e3653f0b2ee77edc5639158d9b5bfacfa416ef",
        "Rust baseline",
        "First golden snapshot (Phase 2, slice 5f-ii). "
        "Rust output byte-identical to the retired C++ oracle.",
    ),
    (
        "61def423a38bee8bc2c38ee098ae579743fbedaf",
        "do...while",
        "Phase 6a. Adds the dowhile fixture; existing output unchanged.",
    ),
    (
        "5edd2304e8fd1df33d269d466c8dcadfb8f683f2",
        "break",
        "Phase 6b. Adds the break fixture; break-free loops emit "
        "byte-identical code.",
    ),
    (
        "b6f643554933f88d3a4d92069f1f354121fd4368",
        "ternary",
        "Phase 6c. Adds the ternary fixture; existing output unchanged.",
    ),
    (
        "db7db25e3f1c563d485248bcafb767b7b69f2853",
        "continue + decimal printing rewrite",
        "Phase 6d. Adds the continue and printnum fixtures; the "
        "print_as_number place-value rewrite changes %d output "
        "sizes across 16 fixtures.",
    ),
    (
        "52b87b43a5ce88bc4c000ba59a872a88c7da978e",
        "Rename to c2bf (v0.1.0)",
        "Phase 8 rename. BF output byte-identical to the previous "
        "milestone.",
    ),
    (
        "c37dc8aada86873189b5fd889ee0e5f956bbb5b8",
        "Loop-based constants",
        "set_value builds large constants as a*b+r loops. "
        "10 fixtures shrink; hello_world halves (1209 -> 597 BF bytes).",
    ),
    (
        "42c58dfdeb8d34677f2260050be88b9ffdbc4763",
        "Unary -, +, ~",
        "Adds the unary fixture; existing output unchanged.",
    ),
    (
        "069bafea733f230a0578f23d547379775cac85df",
        "printf %%, %u, %x",
        "Adds the printfmt fixture; existing output unchanged.",
    ),
    (
        "e13e51c17fea2ef38b29c930261548fa3a8dc72a",
        "Hex/octal/binary literals",
        "Adds the literals fixture; existing output unchanged.",
    ),
    (
        "27982357e04eee331f85dddb3c5421aa649802b6",
        "Multiple declarators",
        "Adds the multidecl fixture; existing output unchanged.",
    ),
    (
        "45b9cfc39988db68e7d7a1b992c5b98d364e4dcd",
        "Multi-word type specifiers",
        "Adds the typespec fixture; existing output unchanged.",
    ),
    (
        "fec53d9802a25ea0e308102376816072cee1082c",
        "Casts",
        "Adds the casts fixture; existing output unchanged.",
    ),
    (
        "f2122c703843b205db399c359a5e2edba6f93119",
        "sizeof",
        "Adds the sizeof fixture; existing output unchanged.",
    ),
    (
        "4627a7b5cdf2bd1a2756d1825df2a99d1f46869a",
        "Object-like #define",
        "Adds the define fixture; existing output unchanged.",
    ),
    (
        "d4418ab44a8a7b646e9b9ec39afdddc7acd4a2d9",
        "Bitwise ops and shifts",
        "Adds the bitwise fixture and two order probes; existing "
        "output unchanged.",
    ),
    (
        "2f840714e914859d300c928a7fe8a8f003e8757a",
        "Bitwise/shift compound assignment",
        "Adds the bitassign fixture; existing output unchanged.",
    ),
    (
        "5a3bffa18976270df956e1d11425db2d8fcfa96b",
        "Bitwise optimization",
        "Fused divmod, in-place double/halve, early fold exit. Only "
        "bitwise, bitassign, and the order probes change.",
    ),
    (
        "3c4ab19d43831796410636b00c591107552ed864",
        "Raw combine/scale",
        "Raw bit-combine loops and conditional place accumulation. "
        "Only bitwise, bitassign, and the order_bitwise probe change.",
    ),
    (
        "e0e858f0607892f1551eef50fc090f6b36616d7b",
        "Scoped temp reclaim",
        "perform_if_else/perform_while hand out again the temporaries "
        "allocated inside (never below the largest variable cell). "
        "BF bytes shrink corpus-wide from shorter pointer motion; "
        "hello_world byte-identical.",
    ),
    (
        "412a09b663d85ddc12c49274c2ac9e58e8debabc",
        "Arrays and pointers",
        "Constant-index arrays resolve to static cells; variable "
        "indices and dereference load/store through a subtractive "
        "dispatch (one arm per cell). Pointers add ~2.95M BF bytes; "
        "static code is unchanged.",
    ),
    (
        "6486d4566aac92e03d0b8fea046ef4f2ca20ba8b",
        "Strings",
        "String-literal array initializers, value decay to "
        "anonymous storage, and printf %s over the dispatch. "
        "Strings add ~3.04M BF bytes (eight %s sites); static "
        "code is unchanged.",
    ),
    (
        "634c3f44f03e0c7c8def3d58706560ff07059829",
        "Functions",
        "User-defined functions by inline expansion, with return "
        "guards and expansion reclaim. The functions fixture adds "
        "~0.82M BF bytes on 275 cells; static code is unchanged.",
    ),
    (
        "f3d007ec94218bc9f5cae2902b1dcfa7b7b27c85",
        "Division and scanf sign",
        "Division by zero defined (guarded quotient, remainder "
        "keeps the dividend) and scanf honoring a leading sign, "
        "with the print helpers on unchecked division. The eight "
        "affected fixtures grow by ~69K BF bytes in total; static "
        "code is unchanged.",
    ),
]

GOLDEN_DIR = "tests/golden/bf"


def run_git(root, args):
    """Run git and return stdout, or exit with the diagnosis."""
    try:
        proc = subprocess.run(
            ["git", *args], cwd=root, capture_output=True, text=True, check=False
        )
    except FileNotFoundError:
        print("error: git not found on PATH", file=sys.stderr)
        sys.exit(2)
    if proc.returncode != 0:
        print(f"error: git {' '.join(args)} failed:", file=sys.stderr)
        print(proc.stderr.strip(), file=sys.stderr)
        sys.exit(2)
    return proc.stdout


def repo_root():
    """Repository root for the current checkout."""
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, check=False,
        )
    except FileNotFoundError:
        proc = None
    if proc is not None and proc.returncode == 0:
        return Path(proc.stdout.strip())
    return Path(__file__).resolve().parent.parent


def sizes_at(root, sha):
    """Map fixture name -> BF bytes at a commit, via git ls-tree."""
    out = run_git(root, ["ls-tree", "-r", "-l", sha, "--", GOLDEN_DIR])
    sizes = {}
    for line in out.splitlines():
        meta, path = line.split("\t")
        parts = meta.split()
        if len(parts) != 4 or parts[1] != "blob":
            print(f"error: unexpected ls-tree line for {sha}: {line!r}",
                  file=sys.stderr)
            sys.exit(2)
        name = Path(path).name
        if not name.endswith(".bf"):
            continue
        sizes[name[:-3]] = int(parts[3])
    if not sizes:
        print(f"error: no golden BF files at {sha} (bad SHA or pre-golden "
              "commit?)", file=sys.stderr)
        sys.exit(2)
    return sizes


def fmt_delta(delta):
    """Signed delta, or an em dash when there is no previous value."""
    return "—" if delta is None else f"{delta:+d}"


def load_cell_history(script_dir):
    """Load tools/cell-history.json, or None (with a warning) if absent.

    Malformed data is an error: a stale or half-written memory
    section is worse than none.
    """
    path = script_dir / "cell-history.json"
    if not path.exists():
        print(
            f"warning: {path} not found; rendering memory sections "
            "as unrecorded",
            file=sys.stderr,
        )
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        print(f"error: {path} is not valid JSON: {e}", file=sys.stderr)
        sys.exit(2)
    if not isinstance(data, dict) or "allocated" not in data or "runtime" not in data:
        print(
            f"error: {path} lacks 'allocated'/'runtime' sections",
            file=sys.stderr,
        )
        sys.exit(2)
    return data


def short_sha(sha):
    """Short SHA prefix used as a key in the cell-history data."""
    return sha[:7]


def render(rows, cell_data):
    """Render the size-history document.

    `rows` is [(sha, label, note, sizes)]; `cell_data` is the parsed
    tools/cell-history.json, or None when it has not been recorded.
    """
    lines = []
    lines.append("# BF size history")
    lines.append("")
    lines.append(
        "> Generated by `tools/size-history.py`. Do not edit by hand; "
        "add a milestone to the script and rerun it:"
    )
    lines.append(">")
    lines.append("> ```")
    lines.append("> python3 tools/size-history.py")
    lines.append("> ```")
    lines.append("")
    lines.append(
        "How the emitted Brainfuck size (bytes) for each golden fixture "
        "changed across notable commits. A row records the transpiler "
        "output as committed in that milestone's tree."
    )
    lines.append("")
    lines.append("## Method and caveats")
    lines.append("")
    lines.append(
        "- Sizes are the byte sizes of committed "
        "`tests/golden/bf/*.bf` blobs, read with "
        "`git ls-tree -r -l` at each milestone commit. No code is "
        "checked out and nothing is built, so the table is "
        "reproducible from the repository alone."
    )
    lines.append(
        "- A fixture absent at a milestone did not exist yet (shown "
        "as `—`). Totals therefore grow as fixtures are added and "
        "are not comparable across milestones with different "
        "fixture sets. The like-for-like change compares only "
        "fixtures present at both milestones."
    )
    lines.append(
        "- Sizes reflect both the code generator and the fixture "
        "source at that commit. A milestone that edits a fixture "
        "changes sizes for reasons other than a single "
        "optimization; the milestone notes say so where it applies."
    )
    lines.append(
        "- Before the first milestone there are no committed golden "
        "outputs, so the C++ era is not reconstructed here."
    )
    lines.append(
        "- Memory usage is tracked in the sections below: cells "
        "allocated by the compiler (recorded from the last "
        "milestone onward; allocation counts were never recorded "
        "before) and the runtime footprint backfilled from the "
        "golden programs themselves."
    )
    lines.append(
        "- To record a new point, append its commit (full SHA, "
        "label, note) to `MILESTONES` in the script, rerun it, and "
        "commit the regenerated document with the change."
    )
    lines.append("")
    lines.append("## Summary")
    lines.append("")
    lines.append(
        "| Milestone | Commit | Fixtures | Total bytes | "
        "Like-for-like change | Note |"
    )
    lines.append("|---|---|---|---|---|---|")
    prev = None
    for sha, label, note, sizes in rows:
        total = sum(sizes.values())
        if prev is None:
            delta = None
        else:
            common = set(sizes) & set(prev)
            delta = sum(sizes[f] - prev[f] for f in common)
        lines.append(
            f"| {label} | `{sha[:7]}` | {len(sizes)} | {total} | "
            f"{fmt_delta(delta)} | {note} |"
        )
        prev = sizes
    lines.append("")
    lines.append("## Sizes by fixture")
    lines.append("")
    prev = None
    for sha, label, note, sizes in rows:
        lines.append(f"### {label} (`{sha[:7]}`)")
        lines.append("")
        lines.append(note)
        lines.append("")
        lines.append("| Fixture | Bytes | Change vs previous |")
        lines.append("|---|---|---|")
        for fixture in sorted(sizes):
            if prev is None or fixture not in prev:
                delta = None
            else:
                delta = sizes[fixture] - prev[fixture]
            lines.append(
                f"| `{fixture}` | {sizes[fixture]} | {fmt_delta(delta)} |"
            )
        lines.append("")
        prev = sizes
    render_memory_sections(lines, rows, cell_data)
    return "\n".join(lines) + "\n"


def render_memory_sections(lines, rows, cell_data):
    """Append the allocated-cells and runtime-footprint sections."""
    lines.append("## Cells allocated (compiler)")
    lines.append("")
    lines.append(
        "Tape cells the compiler's bump allocator reserved for each "
        "program (`Memory::allocated_cells` after compilation). "
        "Allocation never reuses cells, so this is the static tape "
        "footprint: fewer allocated cells means less tape reserved, "
        "whether or not execution ever visits them all."
    )
    lines.append("")
    if cell_data is None:
        lines.append("No allocated-cell data recorded yet.")
        lines.append("")
    else:
        allocated = cell_data["allocated"]
        measured = [r for r in rows if short_sha(r[0]) in allocated]
        for sha, label, _note, _sizes in measured:
            cells = allocated[short_sha(sha)]
            lines.append(f"### {label} (`{sha[:7]}`)")
            lines.append("")
            lines.append("| Fixture | Cells alloc |")
            lines.append("|---|---|")
            for fixture in sorted(cells):
                lines.append(f"| `{fixture}` | {cells[fixture]} |")
            lines.append("")
        unmeasured = [label for sha, label, _n, _s in rows
                      if short_sha(sha) not in allocated]
        if unmeasured:
            lines.append(
                "Allocated cells were not recorded at: "
                + ", ".join(unmeasured)
                + ". Allocation counts were never recorded before "
                "the first measured milestone, and golden BF alone does not "
                "record them, so they are not backfilled."
            )
            lines.append("")
    lines.append("## Runtime footprint (from golden BF)")
    lines.append("")
    lines.append(
        "Tape cells actually used when each milestone's committed "
        "golden program runs with its committed fixture input "
        "(empty input where a golden has no input file): cells "
        "touched counts distinct pointer positions whose cell was "
        "accessed; peak cell is the largest pointer value held; "
        "low cell is the smallest (0 for every transpiled program "
        "here). Backfilled by running the golden programs under "
        "the current runner; see `tools/cell-history.json` for "
        "provenance. A gap between cells allocated and cells "
        "touched is reserved-but-never-visited tape: the main "
        "allocation-optimization signal."
    )
    lines.append("")
    if cell_data is None:
        lines.append("No runtime footprint data recorded yet.")
        lines.append("")
        return
    runtime = cell_data["runtime"]
    for sha, label, _note, _sizes in rows:
        entries = runtime.get(short_sha(sha))
        if entries is None:
            lines.append(f"### {label} (`{sha[:7]}`)")
            lines.append("")
            lines.append("No runtime footprint recorded for this milestone.")
            lines.append("")
            continue
        lines.append(f"### {label} (`{sha[:7]}`)")
        lines.append("")
        lines.append("| Fixture | Cells touched | Peak cell | Low cell |")
        lines.append("|---|---|---|---|")
        for fixture in sorted(entries):
            e = entries[fixture]
            lines.append(
                f"| `{fixture}` | {e['cells_touched']} | "
                f"{e['high_water']} | {e['low_water']} |"
            )
        lines.append("")


def main(argv=None):
    """Generate (or --check) docs/size-history.md."""
    parser = argparse.ArgumentParser(
        description="Regenerate docs/size-history.md from git history."
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="output path (default: <repo root>/docs/size-history.md)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if the on-disk document differs from generated output",
    )
    args = parser.parse_args(argv)
    root = repo_root()
    output = args.output or root / "docs" / "size-history.md"
    rows = [
        (sha, label, note, sizes_at(root, sha))
        for sha, label, note in MILESTONES
    ]
    cell_data = load_cell_history(Path(__file__).resolve().parent)
    text = render(rows, cell_data)
    if args.check:
        if not output.exists() or output.read_text(encoding="utf-8") != text:
            print(f"error: {output} is stale; rerun tools/size-history.py",
                  file=sys.stderr)
            return 1
        print(f"ok: {output} is up to date")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(text, encoding="utf-8")
    print(
        f"wrote {output} ({len(rows)} milestones, "
        f"{len(set().union(*[set(r[3]) for r in rows]))} fixtures)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

