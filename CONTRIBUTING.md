# Contributing to Gall

Thanks for your interest! Bug reports, ideas, docs and code are all welcome.

## Ground rules

1. **Clean-room.** Read [docs/00-clean-room-policy.md](docs/00-clean-room-policy.md). Never paste code,
   assets, strings or screenshots from proprietary file managers, and never decompile them.
2. **Core first.** Behaviour that can be expressed without UI goes into `crates/gall-core`
   with unit tests. Sorting and filter rules exist in Rust *and* `ui/src/core`; change both, with their tests.
3. **Small PRs** with a clear description. Prefer one topic per PR.

## Setup

```sh
npm install && npm install --prefix ui
npm run dev
```

## Before opening a PR

```sh
npm run lint              # cargo fmt --check, clippy -D warnings, TypeScript
npm test                  # Rust + UI unit tests
./scripts/selftest.sh     # UI changes: end-to-end self-test (KDE, needs a display)
cargo test -p gall-core --release --test undo_fuzz -- --ignored   # file operation or undo changes
```

The self-test needs a build with the bundled UI (`npm --prefix ui run build`, then
`cargo build -p gall --features tauri/custom-protocol` and pass that binary to the script):
a debug build that loads the Vite dev server runs the test twice at once (React StrictMode).
The randomized undo test uses the real trash and purges its own items afterwards.

Update `CHANGELOG.md` (section *Unreleased*) and, for features, `docs/04-compatibility-matrix.md`.
If you add a dependency, run `python3 scripts/third_party_notices.py`. Licences must pass `deny.toml`.

## Releases (maintainers)

Bump the version in `Cargo.toml`, `package.json`, `ui/package.json` and `src-tauri/tauri.conf.json`,
move *Unreleased* in `CHANGELOG.md` under the new version, then tag: `git tag v0.2.0 && git push --tags`.
The *Release* workflow builds installers and creates a draft release.
