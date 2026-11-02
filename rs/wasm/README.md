# WebAssembly adapter (`c2bf-wasm`)

A `wasm-bindgen` adapter over `c2bf-core`, kept in its own crate so
the core never depends on `wasm-bindgen` (pollution veto, see
`docs/decisions/0006-distribution-adapters.md`). It exports
`compile`, `run`, `format_code`, and `version` for the browser demo
and local bundler builds. The adapter adds no behavior.

## Build

`wasm-pack` builds the crate for a chosen target. `demo/pkg/` is the
`web` build output directory (gitignored build artifact). Run this
from the repo root with an absolute out-dir: `wasm-pack` resolves a
relative one against the crate directory, so `demo/pkg` alone lands
in `rs/wasm/demo/pkg` and the demo page 404s on the bundle.

```
wasm-pack build rs/wasm --target web --out-dir "$PWD/demo/pkg"
```

For Node (smoke tests, bundlers):

```
wasm-pack build rs/wasm --target bundler --out-dir /tmp/c2bf-pkg
```

## Tests

Rust unit tests (plain functions, no browser needed):

```
cargo test --manifest-path rs/wasm/Cargo.toml
```

Node smoke test against a `--target nodejs` build:

```
wasm-pack build rs/wasm --target nodejs --out-dir /tmp/c2bf-node-pkg
node rs/wasm/smoke.cjs /tmp/c2bf-node-pkg
```

Both builds and the smoke test run in the `wasm` CI job.

## No npm publishing

No npm package will be published for `c2bf` (owner decision,
2026-11-01). Do not run `npm publish`. The `--target bundler`
output is for local bundler use only. CI validates that output's
shape with `npm pack --dry-run`, which contacts no registry and
publishes nothing.

## Demo site (owner-gated, not deployed by CI)

`demo/` is a static page using the `web` build. Deploying it to
GitHub Pages happens automatically on pushes touching `demo/`, `rs/`,
or the workflow itself, or by manual dispatch of
`.github/workflows/pages.yml`, after enabling Pages in the repo
settings (Settings → Pages → Source: GitHub Actions). The build it
uses is the same one CI verifies.
