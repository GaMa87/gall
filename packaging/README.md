# Packaging

Gall is released on [GitHub Releases](https://github.com/GaMa87/gall/releases) as AppImage, `.deb`,
`.rpm`, a Windows installer (`.exe`/`.msi`), a portable Windows zip and macOS `.dmg` files.
Every release also has a `SHA256SUMS` file. This page lists the package managers and app stores
Gall should reach, in order of priority, and what each one needs.

**Want to help?** Packaging is one of the easiest ways to contribute. Open an issue to say which
channel you are working on, so work is not done twice.

## Status

| Channel | Platform | Status | Files here |
|---|---|---|---|
| GitHub Releases | all | ✅ available | `.github/workflows/release.yml` |
| Scoop (own bucket, then Extras) | Windows | 📝 manifest drafted | [`scoop/gall.json`](scoop/gall.json) |
| winget | Windows | 🔜 planned | (generated with `wingetcreate`) |
| AUR `gall-bin` | Arch, Manjaro, EndeavourOS | 📝 PKGBUILD drafted | [`aur/gall-bin/PKGBUILD`](aur/gall-bin/PKGBUILD) |
| Flathub | all Linux | 🔜 planned | [`linux/org.gall.app.metainfo.xml`](linux/org.gall.app.metainfo.xml) |
| Fedora COPR | Fedora, RHEL clones | 🔜 planned | |
| AppImageHub | all Linux | 🔜 planned | |
| openSUSE Build Service | openSUSE, Fedora, Debian, Ubuntu | 💡 idea | |
| Homebrew (own tap) | macOS | 💡 idea | |
| Snap Store, Chocolatey, Microsoft Store, nixpkgs | | 💡 idea | |

## Windows

### Scoop

Scoop suits Gall well because the portable zip keeps all data in `UserData` next to `gall.exe`,
so Scoop's `persist` keeps settings across updates.

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
- **Code changes:** run external commands (terminal, editor, `git`, "Open with") through
  `flatpak-spawn --host` when `/.flatpak-info` exists, and use the trash portal or host trash.
- **Submit:** PR to [flathub/flathub](https://github.com/flathub/flathub) on the `new-pr` branch. After
  approval, claim **verification** for `io.github.gama87` so the store shows a checkmark.

### Fedora COPR

`sudo dnf copr enable gama87/gall && sudo dnf install gall`, with updates through `dnf upgrade`.
Simplest start: a `.spec` that repackages the released `.rpm` (or builds from a source tarball with
vendored crates and node modules). COPR can rebuild on its own via a GitHub webhook.

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
