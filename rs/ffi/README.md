# C ABI adapter (`c2bf-ffi`)

A dependency-free C ABI over `c2bf-core`, kept in its own crate so
the core stays free of FFI surface (pollution veto, see
`docs/decisions/0006-distribution-adapters.md`). The adapter adds no
behavior: it forwards to `compile`, `runner::run`, and `format`.

## Build

```
cargo build --release --manifest-path rs/ffi/Cargo.toml
```

The shared library lands at `rs/target/release/libc2bf.so` (Linux)
or `libc2bf.dylib` (macOS). The header is [c2bf.h](c2bf.h);
the worked example is [example.c](example.c):

```
cc -std=c11 -Wall -Wextra -Werror rs/ffi/example.c -Irs/ffi \
  -o /tmp/c2bf_example -Lrs/target/release -lc2bf
LD_LIBRARY_PATH=rs/target/release /tmp/c2bf_example       # Linux
DYLD_LIBRARY_PATH=rs/target/release /tmp/c2bf_example   # macOS
```

## Contract

See [c2bf.h](c2bf.h) for the authoritative wording.
In short: inputs are `(pointer, length)` byte slices; outputs come
back through `(*out, *out_len)` and are freed with `c2bf_free`
passing the exact length; return codes are `0` ok, `1` operation
error (`c2bf_last_error()`), `2` invalid argument; no panic crosses
the boundary.

## Tests

```
cargo test --manifest-path rs/ffi/Cargo.toml
```

The Rust tests exercise the ABI itself (compile/run round-trip,
error and argument validation, format). The C example is built and
run by the `c-abi` CI job on Ubuntu and macOS.

## Notes

- Symbol names (`c2bf_*`) follow the crate name. The contract shape
  does not change with renames.
- Kotlin/Swift (UniFFI) and Java (JNI) bridges are named follow-ups
  in `docs/decisions/0006-distribution-adapters.md`; both build on
  this ABI. The web/WASM adapter lives separately under `rs/wasm/`.
