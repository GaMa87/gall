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

Run only the updater integration scenarios with `./scripts/selftest.sh --updates` after
building the bundled debug application. This exercises the real Settings UI, backend
installation eligibility and public update endpoint, and writes a report and screenshots
to the temporary path printed by the runner. It never installs an update. Available-update
and signed-installation coverage depends on publishing a configured signed release; a
missing manifest is tested as an actual endpoint error.

The self-test needs a build with the bundled UI (`npm --prefix ui run build`, then
`cargo build -p gall --features tauri/custom-protocol` and pass that binary to the script):
a debug build that loads the Vite dev server runs the test twice at once (React StrictMode).
The randomized undo test uses the real trash and purges its own items afterwards.

Update `CHANGELOG.md` (section *Unreleased*) and, for features, `docs/04-compatibility-matrix.md`.
If you add a dependency, run `python3 scripts/third_party_notices.py`. Licences must pass `deny.toml`.

## Releases (maintainers)

Bump the version in `Cargo.toml`, `package.json` and `ui/package.json` (Tauri inherits the Rust package version),
move *Unreleased* in `CHANGELOG.md` under the new version, then tag: `git tag v0.2.0 && git push --tags`.
The *Release* workflow builds installers and creates a draft release.


### Updater signing setup (one time)

The public `GaMa87/gall` repository builds signed updates. The development
repository does not run this release workflow. Before the first updater release:

1. Run `npm run tauri signer generate -- -w ~/.tauri/gall.key` locally.
   Keep the private key and password backed up outside the repository. Never
   commit the private key or paste it into an issue, log or chat.
2. In the public repository's **Settings → Secrets and variables → Actions**, add
   the private key's contents as secret `TAURI_SIGNING_PRIVATE_KEY`, and its
   password as secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (omit it if empty).
3. Add the public key's contents as repository variable `GALL_UPDATER_PUBLIC_KEY`.
   This public value is embedded in the application to verify downloaded updates.
   Keep using the same key for later releases so existing installations trust them.

The workflow enables `createUpdaterArtifacts` only for release builds and attaches
signed update packages, signatures and `latest.json`. Ordinary local builds do not
need a signing key. To test a configured local build, export
`GALL_UPDATER_PUBLIC_KEY` before building; an unconfigured build cannot install updates.

### Publishing and checking an update

Use tags such as `v0.2.0` for stable releases and `v0.2.0-beta.1` for prereleases.
Manual workflow runs must select an existing version tag. The workflow keeps the
release as a draft; after every platform build and the checksum job succeeds,
inspect its assets, add meaningful release notes and publish it. Verify that
`latest.json` contains Windows x86_64, Linux x86_64 and both macOS architectures,
with signatures and download URLs for the published version. Prereleases are not
served through the application's stable `releases/latest` endpoint.

Existing users must install the first updater-enabled version manually. Test the
next signed stable version from an older installed version: check both startup
notification and **Settings → Updates → Check for updates**, confirm installation,
and verify the new version and preserved settings after restart. Also check no
update, unavailable network, failed download, invalid signature and active file
operation cases. Never publish test releases as the latest stable release for
ordinary users. Windows portable ZIPs and system packages use manual upgrades;
see [packaging](packaging/README.md).

Updater signatures are separate from Windows code signing and Apple signing or
notarization. Cross-platform installation still needs testing on each target OS.

Run focused bug-report scenarios with `./scripts/selftest.sh --bug-report` after
building the bundled debug application. The runner intercepts GitHub opening and
checks compact report preparation, latest-error copying, optional saved-only
diagnostics, encoded-link budget feedback, Escape with Vim keys on/off, completed-draft
reset, non-overwriting ZIP export with finalized images and videos,
flattened screenshot redaction, native Gall capture, actual clip encoding/playback,
responsive review and previous-session recovery without publishing an issue.
Reports and screenshots use the temporary output directory printed by the runner.
On Windows, run `.\scripts\selftest-bug-report.ps1 -Binary .\target\debug\gall.exe`.
Add `-TextOnly` to check just text preparation, clipboard/GitHub handoff and
compact/detailed ZIP exports without repeating capture and recording scenarios.
This copies the bundled debug executable into an isolated temporary portable
installation, preserving your existing Gall session and data. A graphical desktop
and WebView2 are required. Open and inspect the captured PNG files; automated
layout assertions alone are not a visual review. Run both wide/narrow layouts and
dark/light themes when changing the reporter's design.

Run `.\scripts\selftest-preview-shortcuts.ps1 -Binary .\target\debug\gall.exe`
on Windows after building the bundled debug application to check PDF rendering,
selectable text, page/zoom keyboard controls, invalid-file recovery and `.lnk`
activation with Explorer and Vim keys. It creates disposable PDF and shortcut
fixtures in an isolated portable installation and prints the report/screenshot
directory. A graphical desktop and WebView2 are required.

Run focused keyboard and tab scenarios with `./scripts/selftest.sh --keyboard` after
building the bundled debug application. This checks basic/Vim help, standard typing,
Commander-style shortcuts, pinning and tab middle-click through the real UI and backend.
It uses isolated disposable folders, exercises move/trash undo, and captures wide/narrow
help and pin/tab states. It requires KDE Spectacle for screenshots, but not the full
runner's image/archive fixture tools.

Run `./scripts/selftest.sh --key-help` for help-only layout and notation scenarios:
all three profiles, dark/light themes, wide/narrow windows, filtered and empty searches,
and the next-key picker. This mode does not perform file operations.

Run `./scripts/selftest.sh --key-profiles` for welcome/settings profile selection,
persisted profile and custom-shortcut settings, Explorer function keys, Commander
preview/edit/copy/move/new-folder/trash with copy/move/trash undo, and Vim navigation
and pinning. Like `--keyboard`, it uses disposable data and requires Spectacle for
screenshots. To verify a second process reloads the selection, launch the same isolated
binary/configuration with `GALL_SELFTEST=key-profiles-reload` after this scenario;
it expects the Commander profile and custom F9 pin shortcut left by the first process.
