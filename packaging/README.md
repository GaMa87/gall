# Packaging

Gall is released on [GitHub Releases](https://github.com/GaMa87/gall/releases) as AppImage, `.deb`,
`.rpm`, a Windows installer (`.exe`/`.msi`), a portable Windows zip and macOS `.dmg` files.
Every release also has a `SHA256SUMS` file. This page lists the package managers and app stores
Gall should reach, in order of priority, and what each one needs.

**Want to help?** Packaging is one of the easiest ways to contribute. Open an issue to say which
channel you are working on, so work is not done twice.

## In-app updates

Updater-enabled builds check the latest stable GitHub release at startup and from
**Settings → Updates → Check for updates**. By default, Gall downloads, verifies
(Minisign signature and signed version check), extracts, probes (`--gall-update-probe`),
and stages updates in the background, then prompts to **Restart to update** (toggleable
under **Settings → Updates → Download updates automatically**).

Gall uses a hybrid strategy so every non-store installation updates inside the app
without external installer prompts or privilege elevation:

- **In-place replacement:** writable installs (Windows portable ZIP with or without a
  `portable` marker, per-user NSIS, writable AppImage, writable local binary, and
  writable non-translocated macOS `Gall.app`) swap atomically beside the running binary.
- **Signed user-space overlay + startup bootstrapper:** read-only or package-managed
  installs (Linux `.rpm`/`.deb`/COPR/AUR `gall-bin` under `/usr`, Windows `.msi` in `Program Files`,
  per-machine NSIS, Scoop under `\scoop\apps\gall\`, read-only AppImage, and read-only
  or translocated macOS `Gall.app`) keep system files untouched for `rpm`/`dpkg`/MSI
  verification; the base binary re-verifies and hands off to `<updates>/versions/<ver>/`
  with automatic two-strike rollback and a **Revert to \<base\>** option in **Settings → Updates**.
- **Store-managed builds:** Flatpak (`/.flatpak-info`) and Snap (`$SNAP`) delegate updates
  to their store.
- **Source-built distro packages:** the Arch / Omarchy package `gall` (see
  [Arch Linux, Omarchy and aarch64](#arch-linux-omarchy-and-aarch64)) is built with
  `GALL_PACKAGE_MANAGED=1`, so Gall never downloads self-updates there: `pacman` or
  `omarchy update` own them.

Existing installations on `<= 0.2.2` need one manual upgrade to an updater-enabled build
with `*-bin` / `*-app` targets before in-app updates take over. Release signing setup,
publication, and the required real upgrade checks are in
[CONTRIBUTING.md](../CONTRIBUTING.md#updater-signing-setup-one-time). Installation
across all supported operating systems must be verified with signed release builds;
local unit tests alone do not establish that a published upgrade works.

## Status

| Channel | Platform | Status | Files here |
|---|---|---|---|
| GitHub Releases | all | ✅ available | `.github/workflows/release.yml` |
| Scoop (own bucket, then Extras) | Windows | 📝 manifest drafted | [`scoop/gall.json`](scoop/gall.json) |
| winget | Windows | 🔜 planned | (generated with `wingetcreate`) |
| Arch source package `gall` | Arch, Manjaro, EndeavourOS (`x86_64`, `aarch64`) | 📝 PKGBUILD ready (checksum set after tagging) | [`arch/gall/PKGBUILD`](arch/gall/PKGBUILD) |
| Omarchy package repository (`omacom/omarchy-pkgs`) | Omarchy (`x86_64`, `aarch64`) | 📝 template drafted | [`omarchy-pkgs/package.json`](omarchy-pkgs/package.json), [`omarchy/README.md`](omarchy/README.md) |
| AUR `gall-bin` | Arch, Manjaro, EndeavourOS (`x86_64`) | 📝 PKGBUILD drafted (repackages the v0.3.0 `.deb`) | [`aur/gall-bin/PKGBUILD`](aur/gall-bin/PKGBUILD) |
| Flathub | all Linux | 🔜 planned | [`linux/org.gall.app.metainfo.xml`](linux/org.gall.app.metainfo.xml) |
| Fedora COPR | Fedora, RHEL clones | ✅ available (`dnf copr enable gama87/gall`) | [`copr/gall.spec`](copr/gall.spec), [`scripts/copr-build.sh`](../scripts/copr-build.sh) |
| AppImageHub | all Linux | 🔜 planned | |
| openSUSE Build Service | openSUSE, Fedora, Debian, Ubuntu | 💡 idea | |
| Homebrew (own tap) | macOS | 💡 idea | |
| Snap Store, Chocolatey, Microsoft Store, nixpkgs | | 💡 idea | |

## Windows

### Scoop

Scoop suits Gall well: the portable zip needs no installer and keeps settings in the user profile
(`%APPDATA%\gall`), so they survive updates. `persist: UserData` only matters if a `portable` marker is added.

1. **Own bucket (do it right away):** create `GaMa87/scoop-bucket`, add `bucket/gall.json` from
   [`scoop/gall.json`](scoop/gall.json) and fill in the hash from `SHA256SUMS`. Users then run:
   ```powershell
   scoop bucket add gama87 https://github.com/GaMa87/scoop-bucket
   scoop install gama87/gall
   ```
   The [bucket template](https://github.com/ScoopInstaller/BucketTemplate) includes an Excavator
   workflow that picks up new releases on its own (`checkver` + `autoupdate` are in the manifest).
2. **Extras bucket (later):** [ScoopInstaller/Extras](https://github.com/ScoopInstaller/Extras) takes
   GUI apps but asks that they are reasonably well known. Open a PR once Gall has some stars
   and users. Then it is `scoop bucket add extras; scoop install gall`.

### winget

The quickest way to reach most Windows users (`winget install GaMa87.Gall`). No popularity rule applies.

1. After a release: `wingetcreate new https://github.com/GaMa87/gall/releases/download/v<version>/Gall_<version>_x64_en-US.msi`
   with `PackageIdentifier: GaMa87.Gall`, then submit the PR to `microsoft/winget-pkgs`.
2. Later releases: add the [winget-releaser](https://github.com/vedantmgoyal9/winget-releaser) action to
   `release.yml`. It opens the update PR when a release is published (needs a classic token with `public_repo`).

Installers that are not signed get SmartScreen warnings and are checked more closely in winget review.
Free code signing for open source: [SignPath Foundation](https://signpath.org/). Azure Trusted Signing
costs a few euros per month.

### Chocolatey and Microsoft Store (later)

- **Chocolatey:** community repository, moderated by hand (can take weeks). Worth it once winget and Scoop work.
- **Microsoft Store:** accepts ordinary `.msi`/`.exe` installers, and the developer account is free for
  individuals. The installer must be signed, so this comes after code signing.

## Linux

### AUR (`gall-bin`, later `gall` and `gall-git`)

No review, so it can go live the day of a release. Arch users find most of their desktop apps this way.

1. Create an AUR account and add an SSH key.
2. `git clone ssh://aur@aur.archlinux.org/gall-bin.git`, copy [`aur/gall-bin/PKGBUILD`](aur/gall-bin/PKGBUILD),
   run `updpkgsums && makepkg -si` to test, then `makepkg --printsrcinfo > .SRCINFO`, commit and push.
3. Later: a source package `gall` (cargo + npm build) and `gall-git`. Automation:
   [KSXGitHub/github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur) in `release.yml`.

### Flathub

The most important Linux store: GNOME Software, KDE Discover and many distributions show it by default.
It is also the most work:

- **App ID:** Flathub accepts only IDs for a domain or code host you control, so `org.gall.app` will not
  pass. Use `io.github.gama87.Gall` in the Flatpak manifest (the desktop file, icon and metainfo get
  that name in the Flatpak). Gall's Tauri identifier can stay as it is, because inside the sandbox it only names the data folders.
- **Offline build from source:** Flathub builds without network access. Generate `cargo-sources.json`
  with `flatpak-cargo-generator.py` and `node-sources.json` with `flatpak-node-generator` from
  [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools). Runtime `org.gnome.Platform`
  (it includes WebKitGTK 4.1 and GStreamer) with the `rust-stable` and `node` SDK extensions.
- **Permissions:** a file manager needs `--filesystem=host` (plus `--filesystem=xdg-run/gvfs`),
  `--talk-name=org.freedesktop.UDisks2` for mounting and ejecting, and `--talk-name=org.freedesktop.Flatpak`
  so "Open with", "Terminal here" and Git can run host programs via `flatpak-spawn --host`.
  Explain each in the submission PR; reviewers accept these for file managers (Dolphin on Flathub uses host file system access too).
- **Code changes (done):** inside a sandbox (`/.flatpak-info` exists) Gall starts the terminal, editors,
  "Open with" programs and `git` through `flatpak-spawn --host`, and looks programs up on the host
  (`crates/gall-core/src/util/hostspawn.rs`; outside Flatpak nothing changes). Not covered yet: the
  Git credential helpers for network operations and the interactive-rebase editor, which need the
  host Git to call back into the sandbox, and the trash (use the host trash or the trash portal).
- **Draft manifest:** [`flatpak/io.github.gama87.Gall.yml`](flatpak/io.github.gama87.Gall.yml). It was
  not run through `flatpak-builder` yet. To finish it:
  1. In a checkout of the release tag, generate the offline sources next to the manifest:
     `python3 flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json` and
     `flatpak-node-generator --output node-sources.json npm ui/package-lock.json`
     (both from [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools)).
  2. Set `runtime-version` to the current GNOME runtime, and `tag`/`commit` to the release.
  3. `flatpak-builder --user --install --force-clean build-dir io.github.gama87.Gall.yml`, then run
     `flatpak run io.github.gama87.Gall`, `flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.gama87.Gall.yml`
     and `appstreamcli validate` on the installed metainfo.
- **Submit:** PR to [flathub/flathub](https://github.com/flathub/flathub) on the `new-pr` branch. After
  approval, claim **verification** for `io.github.gama87` so the store shows a checkmark.

### File chooser portal (`xdg-desktop-portal-gall`)

Only the Arch `PKGBUILD` ships the FileChooser backend (`/usr/bin/xdg-desktop-portal-gall`,
`gall.portal` and the D-Bus service file from `packaging/linux/`). The `.deb`/`.rpm` do not, on purpose:

- Tauri's `bundle.linux.{deb,rpm}.files` can copy the two static files, but the backend is a separate
  binary (`cargo build --release -p gall-portal`) that the bundler does not build. A `files` entry for a
  binary that does not exist yet breaks every Linux bundle, so it has to arrive together with a
  `cargo build --release -p gall-portal` step in the Linux job of `.github/workflows/release.yml`
  (public repository), checked with `scripts/release-dryrun.sh` and a dry-run release.
- The backend is opt-in (a `portals.conf` must name `gall`), and the AppImage cannot carry it at all
  (a D-Bus service needs a fixed `Exec=` path).

To add it later: build `gall-portal` before `tauri build`, then list
`target/release/xdg-desktop-portal-gall` → `/usr/bin/xdg-desktop-portal-gall`,
`packaging/linux/gall.portal` → `/usr/share/xdg-desktop-portal/portals/gall.portal` and
`packaging/linux/org.freedesktop.impl.portal.desktop.gall.service` →
`/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.gall.service` in both `files` maps.

### Fedora COPR

`sudo dnf copr enable gama87/gall && sudo dnf install gall`, with updates through `dnf upgrade`.

[`copr/gall.spec`](copr/gall.spec) repackages the released `.rpm` (same approach as the AUR `gall-bin`),
and checks it against the SHA256 from `SHA256SUMS`. [`scripts/copr-build.sh`](../scripts/copr-build.sh)
downloads the release, builds a self-contained SRPM (COPR builds have no internet access) and uploads it.

One-time setup:

1. Log in at <https://copr.fedorainfracloud.org> with a Fedora Account (FAS) and accept the license terms.
2. **New Project**: name `gall`, chroots `fedora-rawhide`, `fedora-45`, `fedora-44` and `fedora-43` (add newer ones as they appear),
   architecture `x86_64`. Short description: *Keyboard-first file manager*. The project page needs a
   homepage (`https://github.com/GaMa87/gall`) and a license (`MIT`).
3. Create an API token at <https://copr.fedorainfracloud.org/api/> and save it as `~/.config/copr`.
4. `sudo dnf install copr-cli rpm-build`.

Releases go to COPR automatically: the [*COPR* workflow](../.github/workflows/copr.yml) runs when a GitHub
release is **published** (not when the draft is created, because draft assets are not public), builds the SRPM
from the release's own `.rpm` and `SHA256SUMS`, and submits it. Pre-releases are skipped. No spec change has to
be committed for this; `copr/gall.spec` is only the template.

One-time setup for the automation: store the API token as a secret, from the file (never paste it from the terminal):

```sh
gh secret set COPR_API_TOKEN --repo GaMa87/gall < ~/.config/copr
```

COPR API tokens expire after 180 days. When the workflow fails at *Write the COPR API token*, create a new token at
<https://copr.fedorainfracloud.org/api/> and run the command again.

Manual upload (for example a release that was published before the workflow existed, or a retry): *Actions → COPR →
Run workflow* with the version, or locally:

```sh
scripts/copr-build.sh 0.3.0 --rebuild   # builds the SRPM and test-builds it locally
scripts/copr-build.sh 0.3.0 --submit    # uploads the SRPM to gama87/gall (COPR_PROJECT overrides)
```

A local run with a new version also rewrites `copr/gall.spec`; commit that if you want the template to follow.

The `copr` job of the *Packaging* workflow runs the same build on Fedora, installs the package and checks
that it starts. The `.rpm` from Releases is built on Ubuntu, so it needs no more than the Fedora system
libraries (`webkit2gtk4.1`, `gtk3`); if a release ever fails to start on Fedora, build from source in COPR instead.

### AppImageHub

Add Gall to the [AppImage catalog](https://github.com/AppImage/appimage.github.io) with a PR (one data
file with the download link). Embedding update information (`gh-releases-zsync|GaMa87|gall|latest|Gall_*_amd64.AppImage.zsync`)
lets AppImageUpdate and Gear Lever update it.

### openSUSE Build Service, Snap, nixpkgs (ideas)

- **OBS** builds `.rpm` and `.deb` packages for openSUSE, Fedora, Debian and Ubuntu from one spec and
  hosts the repositories: one place for Linux repositories with automatic updates.
- **Snap Store:** a file manager needs *classic* confinement, which Canonical grants only after a manual
  request. Lower priority, since Ubuntu users can use the `.deb` or Flathub.
- **nixpkgs:** `rustPlatform.buildRustPackage` + `fetchNpmDeps`; usually done by a Nix user who likes the app.

## macOS

**Own Homebrew tap:** `GaMa87/homebrew-tap` with a cask (`brew install --cask gama87/tap/gall`).
The official `homebrew/cask` requires notable projects (GitHub stars/forks thresholds) and
apps that pass Gatekeeper, which needs an Apple Developer ID ($99/year) for signing and notarisation.
Until then, the own tap and the `.dmg` from Releases are the options.

## Before the first store submission

- [ ] Release with `SHA256SUMS` (done by `release.yml`).
- [ ] AppStream metadata valid: `appstreamcli validate packaging/linux/org.gall.app.metainfo.xml`
      (also installed by the `.deb`/`.rpm`, so Discover and GNOME Software show screenshots and a description).
- [ ] Screenshots in `docs/screenshots/` up to date (`./scripts/screenshots.sh`).
- [ ] Add a `<release>` entry to the metainfo with every version.

## Arch Linux, Omarchy and aarch64

- `packaging/arch/gall/PKGBUILD`: source package for `x86_64` and `aarch64` (builds from the release
  tarball, `GALL_PACKAGE_MANAGED=1`, installs the desktop file, D-Bus service, `actions.d`, icons and
  the `package-managed` marker). `gall-bin` under `aur/` stays the binary alternative.
- `packaging/omarchy-pkgs/package.json`: template for `omacom/omarchy-pkgs` (verify the schema there).
- `packaging/omarchy/README.md`: the file-by-file pull request for making Gall Omarchy's file manager.
- `release.yml` builds Linux on `ubuntu-22.04` and, natively, on `ubuntu-24.04-arm`; the `aarch64`
  self-update archive is added to `latest.json` as `linux-aarch64-bin` when it was uploaded.
