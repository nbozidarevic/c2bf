# Versioning

`c2bf` uses Semantic Versioning with a leading zero: versions are
`0.MINOR.PATCH`. Nothing is published to a registry (no crates.io,
no npm): a version exists as the three crate versions, the
`--version` / `c2bf_version` / WASM `version()` output, a
`CHANGELOG.md` section, and an annotated tag `vX.Y.Z` on the
release commit.

## One version, three crates

`rs/Cargo.toml` (`c2bf-core`), `rs/ffi/Cargo.toml` (`c2bf-ffi`),
and `rs/wasm/Cargo.toml` (`c2bf-wasm`) always carry the same
version, bumped in lockstep in the release commit. All three read
it from `CARGO_PKG_VERSION`, so one edit updates the CLI, the C
ABI, and the WASM export together. The `tests/golden/cli/version.out`
golden pins the CLI output and must be updated with the bump.

## When to bump

- **Minor (`0.x.0`)** — a new backwards-compatible capability:
  a new language construct, a new CLI command or flag, a new
  adapter export, or a new documented behavior. Most releases
  are minor releases. The emitted Brainfuck bytes are not API:
  a release that only shrinks or speeds up output, without
  changing what programs do, does not need a bump on its own;
  such improvements ride along with the next minor or patch.
- **Patch (`0.x.y`)** — a bug fix with no new capability: wrong
  output corrected, a crash or hang fixed, a diagnostic
  improved, an adapter fix. Behavior changes that fix a bug
  still count as a patch even if golden files change.
- **Major (`1.0.0`)** — reserved for the day the CLI commands,
  the `c2bf_*` C ABI signatures, and the WASM exports are
  declared stable. Until then the major version stays `0`, and
  breaking changes (a removed flag, a renamed export, a
  changed argument convention) ship in a minor release and say
  so in the changelog. There is no `1.0` date planned.

No bump for: documentation-only changes (including
`docs/perf.md` and `docs/size-history.md` refreshes, which
follow the code they describe), CI-only changes, rejected
spikes (no behavior changed), or golden re-snapshots that
record an already-shipped behavior. Unreleased work
accumulates under `## Unreleased` in `CHANGELOG.md`; do not
bump the crates mid-stream.

## How to release

One commit on `main`, made only after CI is green on its
parent:

1. Move the `## Unreleased` entries under a `## vX.Y.Z`
   section in `CHANGELOG.md`, completing any missing entries.
2. Bump the version in all three `Cargo.toml` files and in
   `tests/golden/cli/version.out`.
3. Push, wait for CI green, then create the annotated tag on
   that exact commit and push it:

   ```sh
   git tag -a vX.Y.Z -m "c2bf vX.Y.Z"
   git push origin vX.Y.Z
   ```

   Never move a tag, and never tag a commit that has not gone
   green. If a release commit needs a fix, the fix lands as a
   new commit and the tag moves to it only by deleting and
   recreating before anyone else has seen it — in practice,
   just cut a patch release instead.
