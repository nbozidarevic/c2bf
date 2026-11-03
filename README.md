# c2bf
`c2bf` transpiles a C subset to Brainfuck — and, as helpers around
that, runs Brainfuck programs and pretty-prints Brainfuck code.

## Quick start

```
> cargo build --release --manifest-path rs/Cargo.toml
> ./rs/target/release/c2bf program.c -o program.bf
```

`c2bf program.c` (no command) compiles. Without `-o`, Brainfuck goes
to stdout; without a code file, source is read from stdin.

## Commands

```
compile [file] [-o file]        Transpile C source to Brainfuck (default)
run [file] [-i file] [-o file]  Run a Brainfuck program
format [file] [-o file] [-m]    Pretty-print Brainfuck (-m minifies)
help                            Show help
```

The code file may also be given as `-c file`. `run` requires a code
file; `compile` and `format` fall back to stdin. Errors go to stderr
with usage; exit code is 1.

Run a transpiled program, feeding it input:

```
> ./rs/target/release/c2bf run -c program.bf -i input.txt
```

## Embedding

The core is a dependency-free Rust library. Adapters live outside it:

- C/C++: C ABI in [rs/ffi/](rs/ffi) (`c2bf.h`, `c2bf_compile`/`c2bf_run`/`c2bf_format`).
- Web: WebAssembly adapter in [rs/wasm/](rs/wasm) and the demo in [demo/](demo). Pages deploys automatically on pushes touching `demo/`, `rs/`, or the Pages workflow (manual dispatch retained); see [rs/wasm/README.md](rs/wasm/README.md). No npm package will be published.

## Tests

```
> cd tests && python3 test          # oracle + golden suite (needs the build above)
```

Byte outputs are pinned under `tests/golden/`; regenerate them (with
review) after intentional changes:

```
> ./tests/golden/generate.sh
```

## What it accepts

Only a C subset: `int`/`char` variables, arithmetic, comparisons,
logic, bitwise/shift, `if`/`else`, `while`/`for`/`do...while`, `switch`, `break`/`continue`,
`printf` with `%d`/`%c`/`%u`/`%x`/`%%`, `scanf` with `%d`/`%c`, and more — see `docs/semantics.md`
for the ground-truth
behavior and `tests/compile/` for examples. The original C++/ANTLR
implementation was retired after the Rust port matched it byte-for-byte
(see `docs/decisions/`).
