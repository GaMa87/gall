<div align="center">

<img src="assets/icon.png" width="96" alt="Gall icon">

# Gall

**A fast, keyboard-first file manager: instant search over the whole computer, a compare tool for
text, spreadsheets and images, and Vim keys.**
Open source (MIT) · Linux, Windows and macOS · Built with Rust, Tauri and React.

[![CI](https://github.com/GaMa87/gall/actions/workflows/ci.yml/badge.svg)](https://github.com/GaMa87/gall/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/GaMa87/gall?include_prereleases&sort=semver)](https://github.com/GaMa87/gall/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS%20(experimental)-informational)](#install)
[![Buy me a coffee](https://img.shields.io/badge/%E2%98%95%20buy%20me%20a%20coffee-revolut.me%2Fgama87-5b7f60)](https://revolut.me/gama87)

**[Download](#install)** · [Features](#highlights) · [Shortcuts](#usage) · [Build from source](#build-from-source) · [Roadmap](#roadmap)

<img src="docs/screenshots/main.png" alt="Gall with two browsers, Miller columns and file lists" width="900">

</div>

Gall is a file manager for people who live on the keyboard. It pairs macOS-Finder-style
**Miller columns** with an orthodox **dual-pane** layout (like Total Commander or Midnight Commander),
and it adds instant search across the whole computer, a built-in compare tool and Vim keys.

**Why try it?**

- **Find any file in milliseconds:** fuzzy search over every file name on the computer, as you type.
- **Find a word, variable or macro in every file:** ripgrep-powered search inside files, in the current folder, your home or the whole drive (`/` or `C:\`), with results streaming in and a preview of each match.
- **Compare anything without another app:** code and text, Excel/ODS/CSV cell by cell, photos and images (swipe, onion skin, pixel diff) and whole folders.
- **Git built in:** status letters next to every file, the branch with ahead/behind, and a diff of any file against `HEAD`.
- **Previews that help:** source code with syntax highlighting, Markdown, PDF, images, video, audio and archives, right next to the file list.
- **Undo for every file operation:** copy, move, rename, batch rename and delete to trash are undone with `Ctrl+Z`.
- **Keyboard all the way:** Vim keys, a which-key menu on `Space`, every shortcut rebindable. The mouse works too, with drag & drop that says what it will do before you let go.
- **Machine monitor in the sidebar:** CPU, GPU and memory with temperatures, network and disk throughput, free space on every drive.
- **Light on your machine:** a native Rust core in a small Tauri app (no bundled Chromium), with a portable Windows build that runs from a USB stick.

> [!NOTE]
> **New on GitHub, not a new project.** Gall was developed and tested locally for a long time
> before this repository went public. It has been the maintainer's everyday file manager on Linux
> (KDE Plasma, Wayland) and on Windows, and both systems were tested just as thoroughly.
> The history here starts with one initial commit because the private development history was squashed for the public release.
> The [changelog](CHANGELOG.md) shows what went in, and the code ships with over 200 Rust unit
> tests, 100+ UI tests and an end-to-end UI self-test.

It is an independent, clean-room project whose workflows are inspired by the publicly
documented behaviour of OneCommander. It contains no code, assets or text from that
product and is not affiliated with it (see [clean-room policy](docs/00-clean-room-policy.md)).

## Highlights

### ⚡ Find any file on your computer as you type

`Ctrl+P` (or `Space Space`) searches file names in the open folder and below; `Tab` widens it to
every file name on the machine. Gall builds the index
in the background with a parallel walk, so a query over a few hundred thousand paths returns
in tens of milliseconds. Matching is fuzzy and uses fzf-style syntax (`ext:jpg`, `'exact`, `!not`),
and the selected file shows a live preview. Type `>` to switch to the command palette.

Build output stays out of the results: `node_modules`, `target`, `dist`, `build`, folders tagged as
caches (`CACHEDIR.TAG`) and everything `.gitignore` leaves out are skipped. For anything else in a
project, right-click it and choose **Hide from search**, or list it in a `.gallignore` file (gitignore
syntax) in the project folder. A `.gallignore` works without git too, Quick Open, find in files and
`F3` all honour it, and hidden items are dimmed in the file list. When you do need the build output,
`Alt+I` searches ignored files too.

<p align="center"><img src="docs/screenshots/quick-open.gif" alt="Quick Open finds a photo among 360,000 indexed paths in 33 ms" width="860"></p>

### 🔎 Find text inside files

You know a word is written somewhere, but not in which file. `Ctrl+F` searches the contents of
the current folder and `Ctrl+Shift+F` searches the whole system. The engine is ripgrep's, so it
skips binaries and respects `.gitignore`. Results stream in grouped by file, with a context preview
of each match. Case, whole-word, regex and include/exclude filters are available.

<p align="center"><img src="docs/screenshots/find-in-files.gif" alt="Find in files: searching thousands of header files for a word" width="860"></p>

### ↔️ Compare anything: text, spreadsheets, images, folders

Select two files and press `Ctrl+Shift+D` (or `=`), or drop one file onto another.

- **Text and code:** side by side or unified, word-level highlights, syntax colouring, moved
  blocks detected. Edit, merge changes across and save.
- **Excel, ODS and CSV:** cell by cell. Rows are matched by a detected key column, and
  added or removed rows and columns are listed.
- **Images:** side by side, swipe, onion skin, or a pixel difference with the share of pixels changed.
- **Folders:** compare two folders and sync them. Select many files to get a similarity matrix
  that finds the odd one out. You can also compare a file with the clipboard or with its Git `HEAD` version.

<table>
<tr>
<td><img src="docs/screenshots/compare-text.png" alt="Text compare with word-level changes" width="440"></td>
<td><img src="docs/screenshots/compare-excel.png" alt="Excel compare, cell by cell" width="440"></td>
</tr>
<tr><td align="center">Text, with word-level changes</td><td align="center">Two Excel files, cell by cell</td></tr>
<tr>
<td colspan="2" align="center"><img src="docs/screenshots/compare-image.png" alt="Image compare in swipe mode" width="600"><br>Images: drag the divider to swipe between versions</td>
</tr>
</table>

### ⌨️ Vim keys, and `Space` shows you the rest

`j` `k` move, `h` `l` go up and into folders, `gg` `G` jump, `yy` `dd` `p` copy, cut and paste,
`/` filters. Press `Space` and a which-key menu lists every command, grouped LazyVim-style
(`Space s g` greps, `Space d d` compares, `Space t n` opens a tab). Press `?` for a searchable
sheet of every shortcut. Prefer plain shortcuts? Turn Vim keys off and tap `Ctrl` twice for the same menu.

<table>
<tr>
<td><img src="docs/screenshots/vim.gif" alt="Moving through folders with j, k, l and h" width="440"></td>
<td><img src="docs/screenshots/which-key.gif" alt="Space opens the which-key menu" width="440"></td>
</tr>
<tr><td align="center"><code>j</code> <code>k</code> <code>l</code> <code>h</code> through folders</td><td align="center"><code>Space</code> opens the command menu</td></tr>
<tr>
<td colspan="2" align="center"><img src="docs/screenshots/key-sheet.png" alt="The ? key sheet lists every shortcut" width="600"><br><code>?</code> lists every shortcut, and you can rebind any of them</td>
</tr>
</table>

### 🤓 A look made for the terminal crowd

Monospace type, a dark theme by default, and file icons from Nerd Fonts and Catppuccin.
If no Nerd Font is installed, Gall falls back to SVG icons. Colour-coded file age, folder colours,
a light theme and an accent colour are built in, and [themes](docs/themes/README.md) restyle the rest: Catppuccin and Gruvbox are included.

### 📊 A small system monitor in the sidebar

<img src="docs/screenshots/system-monitor.png" alt="System monitor: CPU, GPU and memory rings with network, disk and temperature details" width="230" align="right">

CPU, GPU and memory at a glance, with temperatures. Expand it for network and disk throughput.
Every drive in the sidebar shows a usage bar and free space. Hide the monitor with one click if you don't want it.

<br clear="right">

### 🖥 Linux, Windows and macOS

One codebase in Rust and Tauri, packaged for every major desktop: AppImage, `.deb` and `.rpm`
for Linux, an installer or a **portable** zip for Windows, and a `.dmg` for macOS. The core
is unit-tested on all three systems in CI.

## Everything else

| | |
|---|---|
| 🏛 **Miller columns** | Every folder of the path is a column. Ancestor columns compress; hovering expands one. Toggle with breadcrumbs using `` ` ``. |
| 🪟 **Dual pane + 3 layouts** | Standard (side by side), Columns (stacked, current folder as last column) and Classic (single pane). Drag a divider past the edge to collapse a pane. |
| 🗂 **Tabs** | Per pane. Clone, reopen closed (`Ctrl+Shift+T`), aliases, drag between panes, close stale. Restored on start. |
| 📄 **File list** | Natural sort remembered per folder, folders on top, colour-coded age, type-to-filter (`.pdf` filters by extension), rubber-band selection, 5 views inherited by subfolders. |
| 👁 **Previews** | Images (zoom to cursor, pan), PDF, video/audio (frame-step), text/code, zip archives, notes. A large preview is one key away. |
| 📏 **Folder sizes** | `Ctrl+Alt+S` (or automatically) counts everything inside each folder and shows it in the Size column, so sorting by size finds what fills the disk. Running totals while it counts; it stays on one drive and counts hard links once. |
| 🌿 **Git status** | Letters next to names in a repository (`M` modified, `S` staged, `U` new, `!` conflict; folders show what is inside), ignored files dimmed, and the branch with ↑ ahead / ↓ behind in the status bar. Safe in downloaded repositories: none of their configured commands run. |
| ✏️ **Batch rename** | `F2` on several items: a name template (`Trip {n:2}`, `{date}`, `{folder}`), find & replace with regular expressions, case changes, or edit all names as text. A live preview flags clashes; one `Ctrl+Z` undoes it all. |
| ↩️ **Undo everything** | Copy, move, rename, create, delete-to-trash: `Ctrl+Z` (until Gall is closed). Background task queue with progress, time left, cancel and conflict handling. |
| 🖱 **Honest drag & drop** | A label shows *"Move 3 items to Photos"* or *"Copy …"*. Hold `Ctrl` = copy, `Shift` = move, `Alt` = symlink. The default (move / copy / by drive / ask) is configurable. |
| 🏷 **Tags, notes, templates** | Colour tags (`Ctrl+Alt+T`), `.2do` notes with a done checkbox, new files from your own templates (`Ctrl+N`). |
| ☁️ **Sidebar** | Drives with usage bars, cloud folders (OneDrive, Dropbox, Google Drive, iCloud), favourites groups, history. |
| 🎨 **Make it yours** | Searchable settings, every shortcut rebindable, dark/light/system theme, accent colour, custom CSS themes. |

<table>
<tr>
<td><img src="docs/screenshots/columns.png" alt="Columns layout" width="440"></td>
<td><img src="docs/screenshots/light-grid.png" alt="Light theme with thumbnail grid" width="440"></td>
</tr>
<tr><td align="center">Columns layout</td><td align="center">Light theme, thumbnail grid</td></tr>
<tr>
<td colspan="2" align="center"><img src="docs/screenshots/drag.png" alt="Drag label showing the action" width="600"><br>Drag & drop always says what it will do</td>
</tr>
</table>

Full feature status: [docs/04-compatibility-matrix.md](docs/04-compatibility-matrix.md).

## Install

Download a package for your system from **[Releases](https://github.com/GaMa87/gall/releases)**.
Linux and Windows are equally well tested; macOS builds are experimental.

| System | Package | Install |
|---|---|---|
| Any Linux | `Gall_<version>_amd64.AppImage` | `chmod +x Gall_*.AppImage && ./Gall_*.AppImage` |
| Fedora, openSUSE | `Gall-<version>-1.x86_64.rpm` | `sudo dnf install ./Gall-*.rpm` |
| Debian, Ubuntu, Mint | `Gall_<version>_amd64.deb` | `sudo apt install ./Gall_*.deb` |
| Windows 10/11 | `Gall_<version>_x64-setup.exe` or `.msi` | run the installer |
| Windows, no install | `Gall-<version>-windows-x64-portable.zip` | unzip and run `gall.exe` |
| macOS (Apple silicon / Intel) | `Gall_<version>_aarch64.dmg` / `_x64.dmg` | open and drag to Applications |

The macOS build is not signed or notarised yet: if macOS refuses to open it, allow it in
*System Settings › Privacy & Security › Open Anyway*.
Flathub, AUR, Scoop, winget and Homebrew packages are planned; see [packaging](packaging/README.md)
if you would like to help.

**Portable (Windows):** `Gall-<version>-windows-x64-portable.zip` needs no installation. Unzip it
anywhere (a USB drive works) and run `gall.exe`; everything stays in `UserData` next to it.
Build it yourself with `./scripts/portable.ps1` on Windows or `./scripts/portable.sh` on Linux
(cross-compiled with [cargo-xwin](https://github.com/rust-cross/cargo-xwin)); the output is in `dist/`.

Video preview on Linux uses system GStreamer codecs (Fedora: `gstreamer1-plugins-good`,
`gstreamer1-libav`; Debian/Ubuntu: `gstreamer1.0-plugins-good`, `gstreamer1.0-libav`).

## Build from source

Requirements: Rust ≥ 1.95, Node ≥ 20, and the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/)
(Fedora: `webkit2gtk4.1-devel libsoup3-devel gtk3-devel openssl-devel librsvg2-devel`;
Debian/Ubuntu: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev librsvg2-dev`).

```sh
git clone https://github.com/GaMa87/gall.git && cd gall
npm install && npm install --prefix ui
npm run dev          # run with hot reload
npm run build        # release build + installers in target/release/bundle/
npm test             # Rust + UI unit tests
npm run lint         # fmt, clippy, typecheck
./scripts/selftest.sh   # end-to-end UI test of a debug build (KDE; opens a window for ~1 min)
```

## Usage

```
gall [PATH]             open PATH in the left pane
gall -p2 PATH           open PATH in the right pane
gall -o PATH --newtab   send PATH to the running instance, in a new tab
```

Some shortcuts: `Ctrl+P` find a file · `Ctrl+F` / `Ctrl+Shift+F` find text · `Ctrl+Shift+D` compare ·
`?` all shortcuts · `F2` rename · `Alt+C`/`Alt+M` copy/move to the other pane · `Ctrl+Shift+N` new folder ·
`F3` search · `F10` terminal here · `Alt+P` preview pane · `Alt+S` sidebar · `Ctrl+H` hidden files ·
`Ctrl+,` settings. All are listed and editable in *Settings › Shortcuts*.

### New in 0.1.3: search scope and exclusions

Open a folder and press `Ctrl+P`: Quick Open now searches **that folder and its subfolders**
first. Press `Tab` to switch to **everywhere**, and again to return. In WSL, on a network
share or outside the indexed folders, Gall reads the open folder on demand; you can type
while it says "Reading…". "Everywhere" searches the system index, not every unindexed
share or WSL distribution. Configure permanent roots in *Settings › Search › Indexed folders*.
Inside an archive, Quick Open searches the system index; folder scope and `Alt+I` are unavailable.

To keep generated or noisy items out of results, right-click them and choose **Hide from search**.
This writes a `.gallignore` next to the items; it does not delete them or change `.gitignore`.
They remain visible in the file list, dimmed and italic. Right-click **Show in search again**
to remove the rule or add an exception; a hidden folder's header also lets you show it again.
Hide from search offers Undo.

You can edit `.gallignore` yourself. For example, put this in your project folder:

```gitignore
# Skip generated output and logs
/generated/
*.log
# Keep this log searchable
!important.log
```

Rules use gitignore syntax and apply to that folder and everything below it, even outside
a Git repository and when a search starts in a subfolder. Quick Open, find in files and
the name search (`F3`) all respect them. Edits are picked up without manually rebuilding
the index. Global exclusions live in *Settings › Search › Never index or search*;
folders with a valid `CACHEDIR.TAG` are skipped too.

**Need an ignored item just this time?** In find in files or `F3`, `Alt+I` toggles inclusion
of ignored files, including `.gallignore`, `.gitignore`, excluded folders and caches.
In Quick Open, `Alt+I` instead opens the name search in the current folder with ignored
files included; it does not add them to the index. That search uses plain names and
wildcards (`*`, `?`), rather than Quick Open's fuzzy syntax. `Ctrl+H` only changes the
file list's hidden-file visibility; it does not remove search exclusions.

### Windows: browse WSL distributions

Installed WSL distributions appear in the sidebar's **Drives** group. Expand **Not mounted**
and click a stopped distribution to start and open it; running ones appear among the drives.
You can also type `\\wsl.localhost\Ubuntu\home\yourname` in the path bar (replace the names
with your own); Gall also supports `\\wsl$\Ubuntu\…` on older Windows installations.
Typing `/etc` opens it in the current WSL distribution, or the default one when outside WSL.
Docker Desktop's internal distributions are intentionally hidden; this is not Docker container browsing.

Browse, preview, rename, copy and move as usual, including between Windows and WSL or between
distributions. **Terminal here** (`F10`) opens a shell in that distribution's current folder.
In find in files, **Home** refers to the Linux home when opened from WSL. Use
**Copy path › Linux path (WSL)** for `/home/…` or `/mnt/c/…`; the default format is configurable
in Settings.

Delete inside WSL normally uses the distribution's Linux trash; `Ctrl+Z` restores the items.
Gall's Trash window shows only the Windows Recycle Bin, so use undo or a Linux file manager
for WSL trash. If trash is unavailable, Gall asks before deleting permanently; that cannot
be undone. Copying names Windows cannot store into Windows is refused with an explanation.
For an individual name ending in a dot or space, rename it in a WSL terminal first;
copying a normally named folder within WSL can preserve such names inside it.

The eject button **Stop … (WSL)** terminates the whole distribution, not just Gall's view.
Save work in its other programs before confirming. If external changes do not appear,
press `F5` to refresh.

More details and a manual verification checklist: [WSL support](docs/wsl-manual-test.md).
The [0.1.3 changelog](CHANGELOG.md#013--2026-11-05) lists the accompanying fixes.

### Settings, data and themes

Settings are in `~/.config/gall/` and data in `~/.local/share/gall/`. A file named
`portable` next to the executable switches storage to `./UserData` (settings, data, logs and the
webview's cache).

**Themes:** Catppuccin and Gruvbox are included; pick them in *Settings › Appearance › Custom theme*.
For other themes, put `mytheme.css` into `~/.config/gall/themes/` and pick it from the same menu;
`Ctrl+Shift+F5` reloads it while you edit. [docs/themes](docs/themes/README.md) has a guide and a
Dracula example.

## Architecture

```
React + TypeScript UI  ──typed IPC──▶  Tauri shell  ──▶  gall-core (pure Rust)
                                                        fs · nav · view · preview · storage
```

The core has no UI dependency and is unit-tested on Linux, Windows and macOS. Details are in
[docs/03-architecture.md](docs/03-architecture.md).

## Roadmap

- Image conversion
- Multiple and named windows
- Thumbnail cache in Rust
- Drag files *out* to other applications, "Open with…" chooser
- Packages for Flathub, AUR, Scoop, winget and Homebrew ([plan and status](packaging/README.md); help welcome)

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) and the
[clean-room policy](docs/00-clean-room-policy.md). Report bugs with the issue templates and
security problems via [SECURITY.md](SECURITY.md).

## Support

Gall is free and built in spare time. If it saves you time, you can buy me a coffee:
**[revolut.me/gama87](https://revolut.me/gama87)** (Revolut, revtag `@gama87`), or scan the code.
Stars, bug reports and pull requests help just as much. Thank you!

<a href="https://revolut.me/gama87"><img src="docs/support/revtag.svg" width="220" alt="Revolut @gama87: QR code for revolut.me/gama87"></a>

## License

[MIT](LICENSE) © 2026 Gall contributors.
Third-party components and their licences are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
All are permissive or weak file-level copyleft (MPL-2.0, unmodified). This is enforced in CI with [`cargo-deny`](deny.toml).

<sub>Keywords: file manager, file explorer, Miller columns, column view, dual pane, orthodox file manager, fuzzy file search, find in files, ripgrep, diff, file compare, Excel compare, image diff, vim, which-key, Nerd Font, Tauri, Rust, Linux, Windows, macOS, KDE, GNOME, Wayland.</sub>
