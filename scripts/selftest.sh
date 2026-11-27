#!/usr/bin/env bash
# UI self-test: drives a debug build of Gall against disposable test data
# (own D-Bus session, config and data dirs) and writes a report + screenshots.
#   ./scripts/selftest.sh [binary]      default: target/debug/gall
#   ./scripts/selftest.sh --themed-select [binary] themed dropdowns only
#   ./scripts/selftest.sh --updates [binary]  updater scenarios only
#   ./scripts/selftest.sh --notifications [binary] notification and undo feedback only
#   ./scripts/selftest.sh --linux-live [binary] Linux Trash, volume and GPU readings
#   ./scripts/selftest.sh --settings-sections [binary] Settings section switching and shell stability
#   ./scripts/selftest.sh --window-overlays [binary] image preview, dialog and pane entrance
#   ./scripts/selftest.sh --titlebar [binary] title-bar click and window-state regression
#   ./scripts/selftest.sh --keyboard [binary] keyboard and tab scenarios only
#   ./scripts/selftest.sh --popup-placement [binary] menus, submenus and dropdowns opened by a key (GALL_SELFTEST_SCALE=1.5 for another scale)
#   ./scripts/selftest.sh --key-help [binary] help notation and layout only
#   ./scripts/selftest.sh --key-profiles [binary] keyboard profiles and function keys
#   ./scripts/selftest.sh --quick-open [binary] deleted Quick Open results only
#   ./scripts/selftest.sh --search-hidden [binary] hidden-filter search UX (Quick Open, Find in files, F3)
#   ./scripts/selftest.sh --standalone-merge [binary] standalone and windowed Git merges
#   ./scripts/selftest.sh --merge-ai [binary] offline merge proposal scenarios with compare_ai_fake
#   ./scripts/selftest.sh --patch-import [binary] the Apply a patch dialog: three formats, problems, a race, a failed write (GALL_SELFTEST_SCALE=1.5 for another scale)
#   ./scripts/selftest.sh --compare-tabs [binary] comparison tabs: switching, saved position, earlier checks and relink, exports with cancel (GALL_SELFTEST_SCALE=1.5 for another scale)
#   ./scripts/selftest.sh --git-scrubber [binary] the revision scrubber of the commit details: parents, merge choice, root commit, rename, keys
#   ./scripts/selftest.sh --rich-formats [binary] image adapter parity, 4 GiB sparse hex & 50k CSV 60 Hz budgets
#   ./scripts/selftest.sh --doc-compare [binary] positioned PDF & DOCX block/layout comparison
#   ./scripts/selftest.sh --git-rebase-submodules [binary] rebase with submodules only
#   ./scripts/selftest.sh --compare-scale [binary] large text comparison budgets
#   ./scripts/selftest.sh --structural-diff [binary] structural diff in the compare and Git views
#   ./scripts/selftest.sh --i18n [binary] language switching (Slovenian, Omarchy language, right to left)
#   ./scripts/selftest.sh --rendering [binary] sharpness of icons, borders and rounded corners at several scales
#   ./scripts/selftest.sh --omarchy [binary] Omarchy dark/light theme, rounding 0 vs 8, Glassy
#   ./scripts/selftest.sh --adaptive-layout [binary] 2D adaptive layout: wide, tall half tile, compact quarter tile
#   GALL_SELFTEST_REBASE_SOURCE=/path/to/ah800 clones its staged nested-repository case
# Headless modes for tests/e2e (run by scripts/e2e-suite.sh with GALL_SELFTEST=<mode>, no flag needed here):
#   --e2e-cli-state
#   --e2e-dbus-reveal
#   --e2e-clip-copy
#   --e2e-clip-cut-paste
#   --e2e-clip-paste
#   --e2e-editor-omarchy
#   --e2e-editor-tui
#   --e2e-openwith-desktop
#   --e2e-terminal-script
#   --e2e-gvfs-connect
#   --e2e-gvfs-drives
#   --e2e-gvfs-network
#   --e2e-gvfs-slow
#   --e2e-gvfs-unmount
#   --e2e-omarchy-dark
#   --e2e-omarchy-font
#   --e2e-omarchy-glassy
#   --e2e-omarchy-inactive
#   --e2e-omarchy-rounding
#   --e2e-omarchy-swap
#   --e2e-adaptive-density
#   --e2e-adaptive-compact
#   --e2e-adaptive-override
#   --e2e-adaptive-tall
#   --e2e-adaptive-wide
#   --e2e-tiling-maximize
#   --e2e-window-buttons
#   --e2e-actions-exec
#   --e2e-actions-filter
#   --e2e-actions-floating
#   --e2e-actions-match
#   --e2e-terminal-cwd
#   --e2e-zoxide-quickopen
#   --e2e-vim-bounds
#   --e2e-vim-ctrl-l
#   --e2e-vim-editable-typing
#   --e2e-vim-new-window-chord
#   --e2e-vim-q-input
#   --e2e-vim-space-leader
# Requires: ImageMagick, zip (full suite only). Screenshots use WebKitGTK directly
# (optional override: GALL_SELFTEST_CAPTURE=webview|spectacle|grim).
# Runs in a private headless compositor when available (kwin_wayland, labwc, weston,
# sway, or Xvfb), so your mouse, keyboard and screen stay free; otherwise uses the
# active desktop (auto-floating on Hyprland/Omarchy). Set GALL_GUI_BACKEND=... or
# GALL_SELFTEST_REAL=1 to override. Close Gall first for the full suite.
set -u
MODE=1
if [ "${1:-}" = "--themed-select" ]; then MODE=themed-select; shift; fi
if [ "${1:-}" = "--updates" ]; then MODE=updates; shift; fi
if [ "${1:-}" = "--quick-open" ]; then MODE=quick-open; shift; fi
if [ "${1:-}" = "--search-hidden" ]; then MODE=search-hidden; shift; fi
if [ "${1:-}" = "--bug-report" ]; then MODE=bug-report; shift; fi
if [ "${1:-}" = "--bug-report-entry" ]; then MODE=bug-report-entry; shift; fi
if [ "${1:-}" = "--notifications" ]; then MODE=notifications; shift; fi
if [ "${1:-}" = "--linux-live" ]; then MODE=linux-live; shift; fi
if [ "${1:-}" = "--settings-sections" ]; then MODE=settings-sections; shift; fi
if [ "${1:-}" = "--window-overlays" ]; then MODE=window-overlays; shift; fi
if [ "${1:-}" = "--titlebar" ]; then MODE=titlebar; shift; fi
if [ "${1:-}" = "--keyboard" ]; then MODE=keyboard; shift; fi
if [ "${1:-}" = "--popup-placement" ]; then MODE=popup-placement; shift; fi
if [ "${1:-}" = "--key-help" ]; then MODE=key-help; shift; fi
if [ "${1:-}" = "--key-profiles" ]; then MODE=key-profiles; shift; fi
if [ "${1:-}" = "--i18n" ]; then MODE=i18n; shift; fi
if [ "${1:-}" = "--rendering" ]; then MODE=rendering; shift; fi
if [ "${1:-}" = "--omarchy" ]; then MODE=omarchy; shift; fi
if [ "${1:-}" = "--adaptive-layout" ]; then MODE=adaptive-layout; shift; fi
if [ "${1:-}" = "--workspaces-pairs" ]; then MODE=workspaces-pairs; shift; fi
if [ "${1:-}" = "--e2e-adaptive-density" ]; then MODE=e2e-adaptive-density; shift; fi
if [ "${1:-}" = "--git-compare" ]; then MODE=git-compare; shift; fi
if [ "${1:-}" = "--git-workspace" ]; then MODE=git-workspace; shift; fi
if [ "${1:-}" = "--standalone-merge" ]; then MODE=standalone-merge; shift; fi
if [ "${1:-}" = "--merge-ai" ]; then MODE=merge-ai; shift; fi
if [ "${1:-}" = "--compare-ai" ]; then MODE=compare-ai; shift; fi
if [ "${1:-}" = "--outline-generated" ]; then MODE=outline-generated; shift; fi
if [ "${1:-}" = "--rich-formats" ]; then MODE=rich-formats; shift; fi
if [ "${1:-}" = "--patch-import" ]; then MODE=patch-import; shift; fi
if [ "${1:-}" = "--doc-compare" ]; then MODE=doc-compare; shift; fi
if [ "${1:-}" = "--compare-tabs" ]; then MODE=compare-tabs; shift; fi
if [ "${1:-}" = "--git-scrubber" ]; then MODE=git-scrubber; shift; fi
if [ "${1:-}" = "--git-baseline" ]; then MODE=git-baseline; shift; fi
if [ "${1:-}" = "--file-groups" ]; then MODE=file-groups; shift; fi
if [ "${1:-}" = "--structural-diff" ]; then MODE=structural-diff; shift; fi
if [ "${1:-}" = "--git-rebase-submodules" ]; then MODE=git-rebase-submodules; shift; fi
if [ "${1:-}" = "--compare-scale" ]; then MODE=e2e-compare-scale; shift; fi
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IS_MACOS=0
[ "$(uname -s)" = "Darwin" ] && IS_MACOS=1
resolve_path() {
  if command -v realpath >/dev/null 2>&1; then
    realpath "$1"
  else
    python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"
  fi
}
sed_inplace() {
  if [ "$IS_MACOS" -eq 1 ]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}
touch_days_ago() {
  local days="$1"; shift
  python3 - "$days" "$@" <<'PY'
import os, sys, time
t = time.time() - int(sys.argv[1]) * 86400
for p in sys.argv[2:]:
    os.utime(p, (t, t))
PY
}
make_test_image() {
  local kind="$1" out="$2"
  if command -v magick >/dev/null 2>&1; then
    case "$kind" in
      sunset) magick -size 640x400 gradient:'#2b3a55-#e9b44c' "$out" ;;
      plasma) magick -size 300x300 plasma:fractal "$out" ;;
      img-a)  magick -size 320x200 gradient:'#2b3a55-#e9b44c' "$out" ;;
      img-b)  magick Images/a.png -fill red -draw 'rectangle 40,40 120,100' "$out" ;;
    esac
    return
  fi
  python3 - "$kind" "$out" <<'PY'
import os, struct, subprocess, sys, zlib

kind, out = sys.argv[1], sys.argv[2]

def write_png(path, w, h, pixel_fn):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            r, g, b = pixel_fn(x, y, w, h)
            raw.extend((r, g, b))
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)

def grad(x, y, w, h):
    t = y / max(1, h - 1)
    return (int(0x2b + (0xe9 - 0x2b) * t), int(0x3a + (0xb4 - 0x3a) * t), int(0x55 + (0x4c - 0x55) * t))

if kind == "sunset":
    tmp = out + ".tmp.png"
    write_png(tmp, 640, 400, grad)
    if os.path.exists("/usr/bin/sips"):
        subprocess.run(["/usr/bin/sips", "-s", "format", "jpeg", tmp, "--out", out], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        os.remove(tmp)
    else:
        os.replace(tmp, out)
elif kind == "plasma":
    write_png(out, 300, 300, lambda x, y, w, h: ((x * 3 + y) % 256, (y * 2 + x) % 256, (x ^ y) % 256))
elif kind == "img-a":
    write_png(out, 320, 200, grad)
elif kind == "img-b":
    write_png(out, 320, 200, lambda x, y, w, h: (220, 40, 40) if (40 <= x <= 120 and 40 <= y <= 100) else grad(x, y, w, h))
PY
}
APP_INPUT="${1:-target/debug/gall}"
if [ -d "$APP_INPUT" ] && [ -x "$APP_INPUT/Contents/MacOS/gall" ]; then
  APP_INPUT="$APP_INPUT/Contents/MacOS/gall"
fi
APP="$(resolve_path "$APP_INPUT")"
if [ "$MODE" = "e2e-compare-scale" ] || [ "$MODE" = "themed-select" ] || [ "$MODE" = "updates" ] || [ "$MODE" = "quick-open" ] || [ "$MODE" = "search-hidden" ] || [ "$MODE" = "bug-report" ] || [ "$MODE" = "bug-report-entry" ] || [ "$MODE" = "notifications" ] || [ "$MODE" = "linux-live" ] || [ "$MODE" = "settings-sections" ] || [ "$MODE" = "window-overlays" ] || [ "$MODE" = "titlebar" ] || [ "$MODE" = "keyboard" ] || [ "$MODE" = "key-help" ] || [ "$MODE" = "popup-placement" ] || [ "$MODE" = "key-profiles" ] || [ "$MODE" = "git-compare" ] || [ "$MODE" = "file-groups" ] || [ "$MODE" = "git-baseline" ] || [ "$MODE" = "git-workspace" ] || [ "$MODE" = "standalone-merge" ] || [ "$MODE" = "merge-ai" ] || [ "$MODE" = "compare-ai" ] || [ "$MODE" = "outline-generated" ] || [ "$MODE" = "rich-formats" ] || [ "$MODE" = "patch-import" ] || [ "$MODE" = "doc-compare" ] || [ "$MODE" = "compare-tabs" ] || [ "$MODE" = "git-scrubber" ] || [ "$MODE" = "git-rebase-submodules" ] || [ "$MODE" = "structural-diff" ] || [ "$MODE" = "omarchy" ] || [ "$MODE" = "adaptive-layout" ] || [ "$MODE" = "workspaces-pairs" ] || [ "$MODE" = "e2e-adaptive-density" ] || [ "$MODE" = "rendering" ] || [ "$MODE" = "i18n" ]; then
  # This focused test has isolated config/data and must preserve an existing user session.
  export GALL_MULTI_INSTANCE=1
elif pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-selftest-XXXX)" || exit 2
mkdir -p "$T/config" "$T/data" "$T/out" "$T/td" || exit 2
# macOS ignores XDG_CONFIG_HOME/XDG_DATA_HOME (AppDirs uses ~/Library/Application Support).
# Isolate every macOS run in a disposable portable copy next to a `portable` marker so the
# user's real Gall profile is never touched.
if [ "$IS_MACOS" -eq 1 ]; then
  mkdir -p "$T/app/UserData" || exit 2
  cp -c "$APP" "$T/app/gall" 2>/dev/null || cp "$APP" "$T/app/gall" || exit 2
  chmod +x "$T/app/gall"
  : > "$T/app/portable"
  APP="$T/app/gall"
  CFG_DIR="$T/app/UserData"
  DATA_DIR="$T/app/UserData"
else
  CFG_DIR="$T/config/gall"
  DATA_DIR="$T/data/gall"
fi
# Keep the user's fonts visible (XDG_DATA_HOME is redirected below); set
# GALL_SELFTEST_NOFONTS=1 to test the fallbacks for missing fonts.
[ -z "${GALL_SELFTEST_NOFONTS:-}" ] && [ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
cd "$T/td" || exit 2
# Use an isolated repository already in a real merge conflict. A failure in an
# unrelated Git workspace scenario must not prevent these merge checks.
if [ "$MODE" = "outline-generated" ]; then
  (
    set -e
    mkdir -p "$T/outline-gen/src"
    cd "$T/outline-gen"
    git init -q -b main
    git config user.name Test
    git config user.email test@example.com
    git config commit.gpgSign false
    git config core.hooksPath /dev/null
    git config core.autocrlf false
    : > .git/empty-excludes
    git config core.excludesFile "$PWD/.git/empty-excludes"
    printf '*.pb.rs linguist-generated\n' > .gitattributes
    printf 'pub fn value() -> u32 { 1 }\n' > src/lib.rs
    printf 'version = 1\n' > Cargo.lock
    printf 'pub const SCHEMA: u32 = 1;\n' > src/schema.pb.rs
    printf 'gen = 1\n' > src/custom.gen.txt
    git add -A
    git commit -qm 'initial'
    printf 'pub fn value() -> u32 { 2 }\n' > src/lib.rs
    printf 'version = 2\n' > Cargo.lock
    printf 'pub const SCHEMA: u32 = 2;\n' > src/schema.pb.rs
    printf 'gen = 2\n' > src/custom.gen.txt
    git add -A
    git commit -qm 'update generated'
    printf 'pub fn value() -> u32 { 3 }\n' > src/lib.rs
    printf 'version = 3\n' > Cargo.lock
    printf 'pub const SCHEMA: u32 = 3;\n' > src/schema.pb.rs
    printf 'gen = 3\n' > src/custom.gen.txt
  ) || exit 2
fi
if [ "$MODE" = "compare-ai" ]; then
  FAKE_BIN="$REPO_ROOT/target/debug/examples/compare_ai_fake"
  if [ -x "$FAKE_BIN" ]; then
    printf '%s\n' "$FAKE_BIN" > "$T/td/compare-ai-fake-path.txt"
  fi
fi
if [ "$MODE" = "git-workspace" ] || [ "$MODE" = "merge-ai" ]; then
  FAKE_BIN="$REPO_ROOT/target/debug/examples/compare_ai_fake"
  if [ -x "$FAKE_BIN" ]; then
    printf '%s\n' "$FAKE_BIN" > "$T/td/compare-ai-fake-path.txt"
    (
      set -e
      mkdir -p "$T/merge-ai/src"
      cd "$T/merge-ai"
      git init -q -b main
      git config user.name Test
      git config user.email test@example.com
      git config commit.gpgSign false
      git config core.hooksPath /dev/null
      git config core.autocrlf false
      # Rust file without a final newline to verify composed syntax-check boundaries.
      printf 'fn main() {\n    let x = 1;\n}' > src/sample.rs
      # CRLF file with two separated conflicts and a long line.
      python3 -c '
from pathlib import Path
long_base = "base_long_line_" + ("b" * 160)
Path("notes.txt").write_bytes(
    f"header\r\n{long_base}\r\nmiddle 1\r\nmiddle 2\r\nmiddle 3\r\nmiddle 4\r\nmiddle 5\r\nsecond base\r\nfooter\r\n".encode()
)
'
      git add src/sample.rs notes.txt
      git commit -qm 'merge-ai base'
      git switch -qc incoming
      printf 'fn main() {\n    let y = 3;\n}' > src/sample.rs
      python3 -c '
from pathlib import Path
long_r = "remote_long_line_" + ("r" * 220)
Path("notes.txt").write_bytes(
    f"header\r\n{long_r}\r\nmiddle 1\r\nmiddle 2\r\nmiddle 3\r\nmiddle 4\r\nmiddle 5\r\nsecond remote\r\nfooter\r\n".encode()
)
'
      git commit -qam 'merge-ai incoming'
      git switch -q main
      printf 'fn main() {\n    let x = 2;\n}' > src/sample.rs
      python3 -c '
from pathlib import Path
long_l = "local_long_line_" + ("l" * 220)
Path("notes.txt").write_bytes(
    f"header\r\n{long_l}\r\nmiddle 1\r\nmiddle 2\r\nmiddle 3\r\nmiddle 4\r\nmiddle 5\r\nsecond local\r\nfooter\r\n".encode()
)
'
      git commit -qam 'merge-ai local'
      if git merge incoming >/dev/null 2>&1; then
        echo "Expected the merge-ai fixture to conflict" >&2
        exit 2
      fi
      git rev-parse --verify MERGE_HEAD >/dev/null
    ) || exit 2
  elif [ "$MODE" = "merge-ai" ]; then
    echo "Build target/debug/examples/compare_ai_fake first: cargo build -p gall-core --example compare_ai_fake" >&2
    exit 2
  fi
fi
if [ "$MODE" = "patch-import" ]; then
  # Patches written by GNU diff in its three formats, each for its own copy of one file; the copy of uni starts two lines
  # later (its hunks are found away from the stated lines), the one of bad differs where the patch changes it, and the
  # second folder of wr cannot be written (the first file written is put back).
  (
    set -e
    P="$T/patch-import"
    mkdir -p "$P/orig" "$P/new" "$P/uni" "$P/ctx" "$P/nor" "$P/bad" "$P/race" "$P/cmp" "$P/wr/one" "$P/wr/two"
    seq 1 20 | sed 's/^/line /' > "$P/orig/f.txt"
    sed -e 's/^line 5$/line five/' -e '/^line 15$/a line 15b' "$P/orig/f.txt" > "$P/new/f.txt"
    cd "$P"
    diff -u --label a/f.txt --label b/f.txt orig/f.txt new/f.txt > uni/fix.patch || true
    diff -c --label a/f.txt --label b/f.txt orig/f.txt new/f.txt > ctx/fix.patch || true
    diff orig/f.txt new/f.txt > nor/fix.diff || true
    cp uni/fix.patch bad/fix.patch
    cp uni/fix.patch race/fix.patch
    { printf 'added 1\nadded 2\n'; cat orig/f.txt; } > uni/f.txt
    cp orig/f.txt ctx/f.txt
    cp orig/f.txt nor/f.txt
    sed 's/^line 5$/line 5 edited elsewhere/' orig/f.txt > bad/f.txt
    cp orig/f.txt race/f.txt
    cp orig/f.txt cmp/left.txt
    cp new/f.txt cmp/right.txt
    cp orig/f.txt wr/one/a.txt
    cp orig/f.txt wr/two/b.txt
    { diff -u --label a/one/a.txt --label b/one/a.txt orig/f.txt new/f.txt || true
      diff -u --label a/two/b.txt --label b/two/b.txt orig/f.txt new/f.txt || true; } > wr/fix.patch
    chmod 444 wr/two/b.txt
    chmod 555 wr/two
  ) || exit 2
fi
if [ "$MODE" = "rich-formats" ]; then
  (
    set -e
    RF="$T/rich-formats"
    mkdir -p "$RF/git-img"
    python3 - "$RF" <<'PY'
import os, struct, sys, zlib
from pathlib import Path

rf = Path(sys.argv[1])

def write_png(path: Path, w: int, h: int, rows: list[bytes]) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)  # 8-bit RGBA
    raw = b"".join(b"\x00" + row for row in rows)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")
    path.write_bytes(data)

# Unequal RGBA PNGs (320x240 vs 420x280) with transparent border and semi-transparent badge.
left_rows = []
for y in range(240):
    row = bytearray(320 * 4)
    for x in range(320):
        i = x * 4
        if x < 24 or y < 24 or x >= 296 or y >= 216:
            row[i:i+4] = bytes((0, 0, 0, 0))
        elif 40 <= x < 160 and 40 <= y < 140:
            row[i:i+4] = bytes((52, 132, 240, 220))
        elif 160 <= x < 280 and 100 <= y < 200:
            row[i:i+4] = bytes((76, 185, 118, 128))
        else:
            row[i:i+4] = bytes((38, 44, 58, 255))
    left_rows.append(bytes(row))
write_png(rf / "left.png", 320, 240, left_rows)

right_rows = []
for y in range(280):
    row = bytearray(420 * 4)
    for x in range(420):
        i = x * 4
        if x >= 320 or y >= 240:
            row[i:i+4] = bytes((180, 140, 230, 180)) if (x + y) % 16 < 8 else bytes((0, 0, 0, 0))
        elif x < 24 or y < 24 or x >= 296 or y >= 216:
            row[i:i+4] = bytes((0, 0, 0, 0))
        elif 40 <= x < 160 and 40 <= y < 140:
            row[i:i+4] = bytes((235, 92, 64, 240))  # changed colour + alpha
        elif 160 <= x < 280 and 100 <= y < 200:
            row[i:i+4] = bytes((76, 185, 118, 128))
        else:
            row[i:i+4] = bytes((38, 44, 58, 255))
    right_rows.append(bytes(row))
write_png(rf / "right.png", 420, 280, right_rows)

# SVG pair
(rf / "left.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="240" height="160"><rect width="240" height="160" fill="#243044"/><circle cx="120" cy="80" r="48" fill="#4aa3ff"/></svg>\n')
(rf / "right.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="240" height="160"><rect width="240" height="160" fill="#243044"/><circle cx="120" cy="80" r="48" fill="#f06848"/></svg>\n')

# 4 MiB T01-scale binary pair (unscaled 1:1 row height, 262,144 rows) for 60 Hz hex scroll budget
buf_a = bytearray(bytes(range(256)) * (4 * 1024 * 1024 // 256))
buf_b = bytearray(buf_a)
for off in range(0, len(buf_b), 4096):
    buf_b[off] ^= 0xFF
(rf / "t01-a.bin").write_bytes(buf_a)
(rf / "t01-b.bin").write_bytes(buf_b)

# 4 GiB sparse binary pair with differences at head, middle and tail
size = 4 * 1024 * 1024 * 1024
for name, tag in [("sparse-a.bin", b"A"), ("sparse-b.bin", b"B")]:
    with open(rf / name, "wb") as f:
        f.truncate(size)
        f.seek(0)
        f.write(b"GALL-SPARSE-HEAD-" + tag)
        f.seek(2 * 1024 * 1024 * 1024)
        f.write(b"MID-" + tag)
        f.seek(size - 32)
        f.write(b"GALL-SPARSE-TAIL-" + tag)

# 50,000-row CSV pair (T01 scale)
with open(rf / "table-a.csv", "w", newline="\n") as fa, open(rf / "table-b.csv", "w", newline="\n") as fb:
    fa.write("id,name,region,amount,status\n")
    fb.write("id,name,region,amount,status\n")
    for i in range(1, 50001):
        region = ("eu-west", "us-east", "ap-south", "eu-central")[i % 4]
        fa.write(f"row-{i:05d},Item {i},{region},{i * 10},active\n")
        if i % 100 == 0:
            fb.write(f"row-{i:05d},Item {i},{region},{i * 10 + 7},updated\n")
        else:
            fb.write(f"row-{i:05d},Item {i},{region},{i * 10},active\n")
PY
    cd "$RF/git-img"
    git init -q -b main
    git config user.name Test
    git config user.email test@example.com
    git config commit.gpgSign false
    git config core.hooksPath /dev/null
    cp "$RF/left.png" sample.png
    git add sample.png
    git commit -qm 'add sample image'
    cp "$RF/right.png" sample.png
  ) || exit 2
fi
if [ "$MODE" = "doc-compare" ]; then
  (
    set -e
    DC="$T/doc-compare"
    mkdir -p "$DC"
    python3 - "$DC" <<'PY'
import sys, zipfile
from pathlib import Path

dc = Path(sys.argv[1])

def build_pdf(pages: list[list[tuple[int, int, int, str]]]) -> bytes:
    objs: list[bytes] = []
    # 1: Catalog, 2: Pages, 3: Font
    objs.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    page_ids = []
    page_objs = []
    next_id = 4
    for items in pages:
        stream_lines = []
        for x, y, sz, text in items:
            esc = text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
            stream_lines.append(f"BT /F1 {sz} Tf {x} {y} Td ({esc}) Tj ET")
        stream_data = "\n".join(stream_lines).encode("latin-1")
        content_id = next_id
        page_id = next_id + 1
        next_id += 2
        page_ids.append(page_id)
        page_objs.append((content_id, f"<< /Length {len(stream_data)} >>\nstream\n".encode() + stream_data + b"\nendstream"))
        page_objs.append((page_id, f"<< /Type /Page /Parent 2 0 R /Contents {content_id} 0 R >>".encode()))
    kids = " ".join(f"{pid} 0 R" for pid in page_ids)
    objs.append(f"<< /Type /Pages /Kids [{kids}] /Count {len(page_ids)} /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] >>".encode())
    objs.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    for _, body in page_objs:
        objs.append(body)
    out = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for idx, body in enumerate(objs, start=1):
        offsets.append(len(out))
        out.extend(f"{idx} 0 obj\n".encode() + body + b"\nendobj\n")
    xref_pos = len(out)
    out.extend(f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode())
    for off in offsets[1:]:
        out.extend(f"{off:010d} 00000 n \n".encode())
    out.extend(f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n".encode())
    return bytes(out)

(dc / "left.pdf").write_bytes(build_pdf([
    [
        (50, 740, 20, "Two-Column Specification Overview Document"),
        (50, 680, 11, "Left column first line"),
        (330, 680, 11, "Right column first line"),
        (50, 660, 11, "Left column second line"),
        (330, 660, 11, "Right column second line"),
        (50, 640, 11, "Compliance notice moved to page two."),
        (330, 640, 11, "Right column third line"),
    ],
    [
        (50, 700, 11, "Second page summary block."),
    ],
]))

(dc / "right.pdf").write_bytes(build_pdf([
    [
        (50, 740, 20, "Two-Column Specification Overview Document"),
        (50, 680, 11, "Left column first line"),
        (330, 680, 11, "Right column first line"),
        (50, 660, 11, "Left column second line"),
        (330, 660, 11, "Right column second line"),
        (330, 640, 11, "Right column third line"),
    ],
    [
        (50, 700, 11, "Second page summary block."),
        (50, 670, 11, "Compliance notice moved to page two."),
    ],
]))

W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
def write_docx(path: Path, body: str) -> None:
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("word/document.xml", f'<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{W_NS}"><w:body>{body}</w:body></w:document>')

p = lambda t: f"<w:p><w:r><w:t>{t}</w:t></w:r></w:p>"
h = lambda n, t: f'<w:p><w:pPr><w:pStyle w:val="Heading{n}"/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>'
row = lambda cells: "<w:tr>" + "".join(f"<w:tc>{p(c)}</w:tc>" for c in cells) + "</w:tr>"

write_docx(dc / "left.docx",
    h(1, "Architecture Overview")
    + p("Security requirement paragraph relocated later.")
    + p("Core pipeline description stays in place.")
    + "<w:tbl>" + row(["Region", "Units", "Revenue"]) + row(["North", "120", "4800"]) + "</w:tbl>"
)
write_docx(dc / "right.docx",
    h(2, "Architecture Overview")
    + p("Core pipeline description stays in place.")
    + "<w:tbl>" + row(["Region", "Units", "Revenue"]) + row(["North", "145", "5800"]) + row(["East", "80", "3200"]) + "</w:tbl>"
    + p("Security requirement paragraph relocated later.")
)
PY
  ) || exit 2
fi
if [ "$MODE" = "git-workspace" ] || [ "$MODE" = "standalone-merge" ]; then
  (
    set -e
    mkdir "$T/windowed-merge"
    cd "$T/windowed-merge"
    git init -q -b main
    git config user.name Test
    git config user.email test@example.com
    git config commit.gpgSign false
    git config core.hooksPath /dev/null
    git config core.autocrlf false
    awk 'BEGIN {
      for (i = 1; i <= 5000; i++) {
        if (i == 18) {
          printf "wide "
          for (j = 0; j < 1024; j++) printf "x"
          print " long tail"
        } else printf "windowed line %04d\n", i
      }
    }' > windowed.txt
    git add windowed.txt
    git commit -qm 'windowed merge base'
    git switch -qc incoming
    sed_inplace '10s/.*/remote conflict one/;2500s/.*/remote clean edit/;3500s/.*/remote conflict two/' windowed.txt
    git commit -qam 'incoming windowed edits'
    git switch -q main
    sed_inplace '10s/.*/local conflict one/;3500s/.*/local conflict two/' windowed.txt
    git commit -qam 'local windowed edits'
    if git merge incoming >/dev/null 2>&1; then
      echo "Expected the windowed merge fixture to conflict" >&2
      exit 2
    fi
    git rev-parse --verify MERGE_HEAD >/dev/null
  ) || exit 2
fi
if [ "$MODE" = "linux-live" ]; then
  [ "$(uname -s)" = Linux ] || { echo "--linux-live requires Linux" >&2; exit 2; }
  data_mount="$(findmnt --kernel -n -o TARGET --target "$T/data")" || exit 2
  fixture_mount="$(findmnt --kernel -n -o TARGET --target "$T/td")" || exit 2
  [ "$data_mount" = "$fixture_mount" ] ||
    { echo "The Trash fixture and its private XDG data must be on the same filesystem" >&2; exit 2; }
  mkdir Projects || exit 2
  printf 'hello' > gall-trash-probe.txt || exit 2
  lsblk --json --bytes --list --output PATH,KNAME,PKNAME,TYPE,FSTYPE,PARTTYPE,MOUNTPOINT,MOUNTPOINTS > live-block-devices.json || exit 2
  # The freedesktop Trash implementation also scans per-volume trash folders.
  # Inspect directory metadata only and hide each existing folder in the app's
  # private mount namespace so it only reads its isolated XDG Trash.
  if ! python3 - > "$T/mounted-trash-paths" <<'PY'
import os
import re
import stat
import sys
from pathlib import Path

uid = str(os.getuid())
paths = set()
with open("/proc/self/mounts", "rb") as mounts:
    for line in mounts:
        mount = re.sub(rb"\\([0-7]{3})", lambda match: bytes([int(match[1], 8)]), line.split()[1])
        root = Path(os.fsdecode(mount))
        common = root / ".Trash"
        candidates = [root / f".Trash-{uid}"]
        try:
            mode = common.lstat().st_mode
            if stat.S_ISDIR(mode) and mode & stat.S_ISVTX:
                candidates.append(common / uid)
        except (FileNotFoundError, NotADirectoryError, PermissionError):
            pass
        for path in candidates:
            try:
                if stat.S_ISDIR(path.stat().st_mode):
                    paths.add(path.resolve(strict=True))
            except (FileNotFoundError, NotADirectoryError, PermissionError):
                pass
for path in sorted(paths):
    sys.stdout.buffer.write(os.fsencode(path) + b"\0")
PY
  then
    echo "Could not determine the mounted Trash directories to isolate" >&2
    exit 2
  fi
  mapfile -d '' -t mounted_trashes < "$T/mounted-trash-paths" || exit 2
  if [ "${#mounted_trashes[@]}" -gt 0 ]; then
    command -v bwrap >/dev/null ||
      { echo "bubblewrap is needed to hide mounted Trash directories in --linux-live" >&2; exit 2; }
    # Ordinary bind mounts are nodev under bubblewrap. WebKit needs
    # /dev/urandom and the live checks need access to host devices.
    trash_mounts=(bwrap --die-with-parent --bind / / --dev-bind /dev /dev)
    for path in "${mounted_trashes[@]}"; do
      trash_mounts+=(--tmpfs "$path")
    done
  else
    trash_mounts=()
  fi
elif [ "$MODE" = "git-rebase-submodules" ]; then
  if [ -n "${GALL_SELFTEST_REBASE_SOURCE:-}" ]; then
    # Read the real repositories only; all index changes and checkouts use disposable clones.
    rebase_source=$(resolve_path "$GALL_SELFTEST_REBASE_SOURCE")
    staged_link=$(git -C "$rebase_source" ls-files --stage -- .worktrees)
    if [ -z "$staged_link" ]; then echo "Source has no staged .worktrees entry"; exit 2; fi
    git clone --shared --no-checkout -q "$rebase_source" . || exit 2
    git config core.autocrlf false
    git config user.name Test
    git config user.email test@example.com
    git config commit.gpgSign false
    git config core.hooksPath /dev/null
    GIT_LFS_SKIP_SMUDGE=1 git checkout --no-recurse-submodules -q MaybeSometime || exit 2
    mkdir -p Projects
    git branch Dev origin/Dev || exit 2
    git clone --shared --no-checkout -q "$rebase_source/.worktrees" .worktrees || exit 2
    git -C .worktrees config core.autocrlf false
    link_id=$(printf '%s\n' "$staged_link" | awk '$1 == "160000" && $3 == "0" {print $2}')
    if [ -z "$link_id" ]; then echo "Source .worktrees is not a staged gitlink"; exit 2; fi
    GIT_LFS_SKIP_SMUDGE=1 git -C .worktrees -c core.hooksPath=/dev/null checkout --no-recurse-submodules -q --detach "$link_id" || exit 2
    printf '%s\n' "$staged_link" | git update-index --index-info || exit 2
  else
    mkdir -p Projects
    git init -q -b main "$T/module-source"
    git -C "$T/module-source" config user.name Test
    git -C "$T/module-source" config user.email test@example.com
    git -C "$T/module-source" config core.autocrlf false
    printf 'module content\n' > "$T/module-source/module.txt"
    git -C "$T/module-source" add module.txt
    git -C "$T/module-source" -c commit.gpgSign=false -c core.hooksPath=/dev/null commit -qm 'module base'
    git init -q -b main
    git config user.name Test
    git config user.email test@example.com
    git config core.autocrlf false
    git config commit.gpgSign false
    git config core.hooksPath /dev/null
    git -c protocol.file.allow=always submodule add -q "$T/module-source" module
    git -C module config core.autocrlf false
    git commit -qm 'add initialized submodule'
    git checkout -qb topic
    printf 'topic content\n' > topic.txt
    git add topic.txt
    git commit -qm 'topic change'
    git checkout -q main
    printf 'upstream content\n' > upstream.txt
    git add upstream.txt
    git commit -qm 'upstream change'
    git checkout -q topic
    git config submodule.recurse true
    printf 'module local edit\n' > module/module.txt
    printf 'keep untracked content\n' > module/keep.txt
    git clone -q "$T/module-source" .worktrees
    git -C .worktrees config core.autocrlf false
    git add .worktrees
  fi
  printf 'nested repository sentinel\n' > .worktrees/keep.txt
elif [ "$MODE" = "file-groups" ]; then
  mkdir -p Projects mixed/src mixed/Docs empty single many
  # Name, size, date of the last change; "touch -t" is the same on Linux and macOS.
  while read -r name size date; do
    head -c "$size" /dev/zero | tr "\0" x > "mixed/$name"
    touch -t "${date}0000" "mixed/$name"
  done <<EOF
b.PNG 100 20260121
a.png 250 20260111
c10.png 50 20260131
c2.png 50 20260131
notes.txt 10 20260220
read.txt 700 20260106
x.md 5 20260302
pack.tar.gz 2000 20260126
.bashrc 7 20260116
Makefile 9 20260109
LICENSE 3 20260103
noext 1 20260104
EOF
  for i in 1 2 3 4 5; do printf x > "single/file$i.txt"; done
  for i in $(seq -f %03g 1 300); do printf x > "many/file.x$i"; done
elif [ "$MODE" = "git-baseline" ]; then
  mkdir -p Projects
  python3 "$REPO_ROOT/scripts/selftest_git_baseline_fixture.py" "$PWD/repos"
elif [ "$MODE" = "standalone-merge" ] || [ "$MODE" = "merge-ai" ] || [ "$MODE" = "patch-import" ]; then
  mkdir -p Projects
elif [ "$MODE" = "git-scrubber" ]; then
  # One file with a root commit, a rename, and a merge of two edits (so the merge differs from both parents).
  mkdir -p Projects
  git init -q -b main
  git config user.name Test
  git config user.email test@example.com
  git config commit.gpgSign false
  git config core.hooksPath /dev/null
  git config core.autocrlf false
  printf 'one\ntwo\nthree\nfour\nfive\n' > old.txt
  git add old.txt && git commit -qm 'add old.txt'
  sed_inplace '2s/.*/two edited/' old.txt && git commit -qam 'edit line two'
  git mv old.txt new.txt && git commit -qm 'rename to new.txt'
  git checkout -qb side
  sed_inplace '1s/.*/one from side/' new.txt && git commit -qam 'side edits line one'
  git checkout -q main
  sed_inplace '5s/.*/five from main/' new.txt && git commit -qam 'main edits line five'
  git merge -q --no-ff side -m 'merge side'
elif [ "$MODE" = "compare-tabs" ]; then
  mkdir -p Projects
  CT="$T/compare-tabs"
  mkdir -p "$CT/p1" "$CT/p2" "$CT/changed" "$CT/moved-new/sub" "$CT/big" "$CT/out"
  python3 - "$CT" <<'PY'
import os, sys
ct = sys.argv[1]
# p1: 400 lines with 10 separated hunks (folded context still exceeds the window) so scrolling and hunk 2 restoration are unambiguous
left1 = [f"line {i:03d} alpha\n" for i in range(1, 401)]
right1 = list(left1)
for n, i in enumerate(range(9, 400, 40), 1):
    right1[i] = f"line {i + 1:03d} CHANGED {n}\n"
open(os.path.join(ct, "p1", "left.txt"), "w").writelines(left1)
open(os.path.join(ct, "p1", "right.txt"), "w").writelines(right1)
# p2..p8: distinct pairs for multi-tab strip
open(os.path.join(ct, "p2", "left.txt"), "w").write("hello\nworld\n")
open(os.path.join(ct, "p2", "right.txt"), "w").write("hello\ngall\n")
for i in range(3, 9):
    d = os.path.join(ct, f"p{i}")
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, f"a{i}.txt"), "w").write(f"pair {i} before\nline 2\n")
    open(os.path.join(d, f"b{i}.txt"), "w").write(f"pair {i} after\nline 2\n")
open(os.path.join(ct, "changed", "c_left.txt"), "w").write("changed left\n")
open(os.path.join(ct, "changed", "c_right.txt"), "w").write("changed right\n")
open(os.path.join(ct, "moved-new", "sub", "m_left.txt"), "w").write("moved left\nline b\n")
open(os.path.join(ct, "moved-new", "sub", "m_right.txt"), "w").write("moved right\nline b\n")
# Large pair so HTML report export takes long enough to click Cancel reliably
with open(os.path.join(ct, "big", "big_a.txt"), "w") as fa, open(os.path.join(ct, "big", "big_b.txt"), "w") as fb:
    for i in range(45000):
        fa.write(f"row {i:06d} old content value {(i * 17) % 997}\n")
        fb.write(f"row {i:06d} {'new' if i % 3 == 0 else 'old'} content value {(i * 19) % 997}\n")
PY
elif [ "$MODE" = "git-workspace" ]; then
  mkdir -p Projects
  git init -q -b main
  git config user.name Test
  git config user.email test@example.com
  git config commit.gpgSign false
  git config core.hooksPath /dev/null
  git config core.excludesFile /dev/null
  seq 1 30 > file.txt
  git add file.txt
  git commit -qm 'initial file'
  workspace_base=$(git rev-parse HEAD)
  printf 'alpha\n' > alpha.txt
  git add alpha.txt
  git commit -qm 'add alpha'
  printf 'beta\n' > beta.txt
  git add beta.txt
  git commit -qm 'add beta'
  printf 'gamma\n' > gamma.txt
  git add gamma.txt
  git commit -qm 'add gamma'
  git checkout -qb conflict-left "$workspace_base"
  sed_inplace '15s/.*/left side/' file.txt
  git commit -qam 'left conflict'
  git checkout -qb conflict-right "$workspace_base"
  sed_inplace '15s/.*/right side/' file.txt
  git commit -qam 'right conflict'
  git checkout -q main
  git clone --bare -q . "$T/remote.git"
  git remote add origin "$T/remote.git"
  git fetch -q origin
  git branch --set-upstream-to=origin/main main >/dev/null
  mkdir -p "$T/submodule-host"
  git -C "$T/submodule-host" init -q -b main
  git -C "$T/submodule-host" config user.name Test
  git -C "$T/submodule-host" config user.email test@example.com
  git -C "$T/submodule-host" config commit.gpgSign false
  git -C "$T/submodule-host" config core.hooksPath /dev/null
  git -C "$T/submodule-host" -c protocol.file.allow=always submodule add -q "$T/remote.git" module
  git -C "$T/submodule-host" commit -qm 'add submodule'
  git -C "$T/submodule-host" submodule deinit -q module
  # More than one page of history, so the "Load more commits" button is shown.
  git init -q -b main "$T/many-commits"
  git -C "$T/many-commits" config core.autocrlf false
  for i in $(seq 1 230); do
    printf 'commit refs/heads/main\ncommitter Test <test@example.com> %d +0000\ndata 15\ncommit no. %03d\n\n' "$((1700000000 + i * 60))" "$i"
  done | git -C "$T/many-commits" fast-import --quiet
  git -C "$T/many-commits" reset -q --hard
  sed_inplace '2s/.*/second changed/;25s/.*/twenty-five changed/' file.txt
  printf 'untracked\n' > new.txt
elif [ "$MODE" = "e2e-compare-scale" ]; then
  mkdir -p Projects
  python3 - <<'FIXTURE'
from pathlib import Path

def lines(name, prefix, step=1):
    with Path(name).open("w", encoding="utf-8") as out:
        for i in range(100_000):
            out.write(f"{prefix if i % step == 0 else 'original'} line {i:06d} for comparison\n")

lines("scale-left.txt", "original")
lines("scale-sparse.txt", "edited", 100)
lines("scale-dense.txt", "replacement")
Path("scale-long.txt").write_bytes(b"x" * (32 * 1024 * 1024))
FIXTURE
elif [ "$MODE" = "structural-diff" ]; then
  # One file, three versions: committed, committed again with a reflowed call and one changed
  # argument, and a working copy with one line only reformatted and one changed number.
  mkdir -p Projects
  body() {
    printf 'fn total(items: &[u32]) -> u32 {\n%s\n    %s\n    sum\n}\n\nfn main() {\n    println!("{}", total(&[1, %s]));\n}\n' "$1" "$2" "$3"
  }
  git -c user.name=Test -c user.email=test@example.com init -q -b main
  git config core.autocrlf false
  body '    let sum = add(items[0], items[1]);' 'report(sum, "total  ok");' 2 > calc.rs
  git add calc.rs
  git -c user.name=Test -c user.email=test@example.com commit -qm first
  reflowed="$(printf '    let sum = add(\n        items[0],\n        items[2],\n    );')"
  body "$reflowed" 'report(sum, "total  ok");' 2 > calc.rs
  git add calc.rs
  git -c user.name=Test -c user.email=test@example.com commit -qm 'reflow the call'
  body "$reflowed" 'report( sum, "total  ok" );' 3 > calc.rs
elif [ "$MODE" = "git-compare" ]; then
  mkdir -p Projects
  printf 'first\n' > before.txt
  git -c user.name=Test -c user.email=test@example.com init -q -b main
  git config core.autocrlf false
  : > .git/empty-excludes
  git config core.excludesFile "$PWD/.git/empty-excludes"
  git add before.txt
  git -c user.name=Test -c user.email=test@example.com commit -qm first
  git mv before.txt file.txt
  git -c user.name=Test -c user.email=test@example.com commit -qm second
  printf 'first\r\n' > file.txt
  git add file.txt
  git -c user.name=Test -c user.email=test@example.com commit -qm 'convert line endings'
  initial_branch=$(git symbolic-ref --short HEAD)
  git -c core.hooksPath=/dev/null checkout --detach -q HEAD
  printf 'reflog only\n' > reflog.txt
  git add reflog.txt
  git -c user.name=Test -c user.email=test@example.com commit -qm 'reflog-only commit'
  git -c core.hooksPath=/dev/null checkout -q "$initial_branch"
  git -c user.name=Test -c user.email=test@example.com commit --allow-empty -qm "unrelated commit"
  printf 'working\n' > file.txt
  printf 'untracked\n' > new.txt
elif [ "$MODE" = "themed-select" ] || [ "$MODE" = "updates" ] || [ "$MODE" = "quick-open" ] || [ "$MODE" = "notifications" ] || [ "$MODE" = "settings-sections" ] || [ "$MODE" = "window-overlays" ] || [ "$MODE" = "titlebar" ] || [ "$MODE" = "keyboard" ] || [ "$MODE" = "key-help" ] || [ "$MODE" = "popup-placement" ] || [ "$MODE" = "key-profiles" ] || [ "$MODE" = "omarchy" ] || [ "$MODE" = "adaptive-layout" ] || [ "$MODE" = "workspaces-pairs" ] || [ "$MODE" = "e2e-adaptive-density" ] || [ "$MODE" = "rendering" ] || [ "$MODE" = "i18n" ] || [ "$MODE" = "compare-ai" ] || [ "$MODE" = "outline-generated" ]; then
  mkdir -p Projects Documents Music Pictures Downloads Videos Desktop src node_modules .git/hooks Photos
  if [ "$MODE" = "omarchy" ] || [ "$MODE" = "rendering" ]; then
    git init -q -b main Projects
    git -C Projects config core.autocrlf false
    : > Projects/.git/empty-excludes
    git -C Projects config core.excludesFile "$PWD/Projects/.git/empty-excludes"
    for subject in "Theme graph base" "Theme graph fixture"; do
      git -C Projects -c user.name=Test -c user.email=test@example.com -c commit.gpgSign=false -c core.hooksPath=/dev/null commit -q --allow-empty -m "$subject"
    done
  fi
  if [ "$MODE" = "popup-placement" ]; then
    for i in $(seq -w 1 60); do echo "line $i" > "Documents/file$i.txt"; done
    git_fixture() { git -c user.name=Test -c user.email=test@example.com -c commit.gpgSign=false -c core.hooksPath=/dev/null -c core.autocrlf=false -c protocol.file.allow=always "$@"; }
    git_fixture init -q -b main ../module-source
    git_fixture -C ../module-source commit -q --allow-empty -m "module"
    git_fixture init -q -b main Projects
    git -C Projects config core.autocrlf false
    : > Projects/.git/empty-excludes
    git -C Projects config core.excludesFile "$PWD/Projects/.git/empty-excludes"
    echo "one" > Projects/file.txt; git_fixture -C Projects add -A; git_fixture -C Projects commit -q -m "first"
    git_fixture -C Projects submodule --quiet add "$(resolve_path ../module-source)" module
    git_fixture -C Projects commit -q -m "second"
    echo "two" > Projects/file.txt
  fi
  if [ "$MODE" = "window-overlays" ]; then
    printf '%s\n' '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="140"><rect width="240" height="140" fill="#3d8bfd"/></svg>' > sample.svg
  fi
  for i in 1 2 10 20; do echo "line $i" > "Documents/file$i.txt"; done
  echo "x" > readme.md; echo "x" > main.rs; echo "x" > app.ts; echo "x" > pic.png
else
mkdir -p Projects/Alpha/src Projects/Beta Photos Music "Documents/Reports 2026"
printf '# Alpha\n\nA test project.\n' > Projects/Alpha/README.md
for i in 1 2 10 20; do echo "line $i" > "Documents/file$i.txt"; done
echo "Buy milk" > Documents/Shopping.2do
make_test_image sunset Photos/sunset.jpg
make_test_image plasma Photos/plasma.png
(cd Documents && zip -q archive.zip file1.txt file2.txt)
printf 'int main(void) { return 0; }\n' > Projects/Alpha/src/main.c
printf '#pragma once\n' > Projects/Alpha/src/main.h
printf '#pragma once\n' > Projects/Alpha/src/vec.hpp
echo x > Projects/Beta/app.properties; echo x > Projects/Beta/notes.md
# compare: word change, whitespace-only change, long unchanged middle (folding), added line
{ echo "name: gall"; echo "version: 0.1.0"; echo "  indent:  two"; for i in $(seq 1 40); do echo "line $i"; done; echo "theme: dark"; } > Projects/Beta/config.old.yaml
{ echo "name: gall"; echo "version: 0.2.0"; echo "  indent: two"; for i in $(seq 1 40); do echo "line $i"; done; echo "theme: light"; echo "vim: true"; } > Projects/Beta/config.yaml
cp Projects/Beta/config.old.yaml Projects/Alpha/config.yaml
# git repo for "compare with HEAD"
# folder compare / sync
for s in L R; do mkdir -p Sync/$s/same-dir/deep; echo same > Sync/$s/same.txt; echo x > Sync/$s/same-dir/deep/a.txt; done
echo "left newer version" > Sync/L/newer-left.txt; echo "old" > Sync/R/newer-left.txt
echo "old" > Sync/L/newer-right.txt; echo "right newer version" > Sync/R/newer-right.txt
touch_days_ago 3 Sync/R/newer-left.txt Sync/L/newer-right.txt
echo aaaa > Sync/L/samesize.txt; echo bbbb > Sync/R/samesize.txt; touch_days_ago 2 Sync/L/samesize.txt Sync/R/samesize.txt
echo L > Sync/L/only-left.txt
mkdir -p Sync/R/only-right-dir; echo R > Sync/R/only-right-dir/f.txt
# merge / images / similarity matrix
mkdir -p Merge Variants Images
cp Documents/archive.zip Images/archive-copy.zip
# conflict: a folder on both sides (merge), one file in both
mkdir -p Sizes/small Sizes/big/deep; head -c 300 /dev/zero > Sizes/small/a.bin; head -c 900 /dev/zero > Sizes/big/b.bin; head -c 600 /dev/zero > Sizes/big/deep/c.bin
mkdir -p Rename; for i in 1 2 3; do echo "$i" > "Rename/IMG_000$i.HEIC"; done
mkdir -p MergeDirs/L/Photos MergeDirs/R/Photos
echo new > MergeDirs/L/Photos/a.txt; echo old > MergeDirs/R/Photos/a.txt
echo l > MergeDirs/L/Photos/l.txt; echo r > MergeDirs/R/Photos/r.txt
printf 'alpha\nbeta\ngamma\ndelta\n' > Merge/left.txt
printf 'alpha\nBETA\ngamma\ndelta\nepsilon\n' > Merge/right.txt
make_test_image img-a Images/a.png
make_test_image img-b Images/b.png
for i in 1 2 3 4; do seq 1 60 > Variants/v$i.txt; done
sed_inplace 's/^10$/ten/' Variants/v2.txt; sed_inplace 's/^20$/twenty/;s/^30$/thirty/' Variants/v3.txt
seq 100 140 > Variants/odd.txt
# tables: reordered columns, removed/added column, key rows, number formats
mkdir -p Tables
printf 'id;name;price;old\n1;Apple;1.0;x\n2;Pear;2,50;y\n3;Plum;3;z\n' > Tables/a.csv
printf 'name;id;price;stock\nPlum;3;3.5;9\nApple;1;1;5\nKiwi;4;4;1\n' > Tables/b.csv
printf 'sku,qty,city\nA1,10,Ljubljana\nB2,5,Maribor\nC3,7,Celje\n' > Tables/x1.csv
printf 'sku,qty,city\nA1,10,Ljubljana\nB2,6,Maribor\nD4,1,Koper\n' > Tables/x2.csv
if command -v soffice >/dev/null 2>&1; then
  ( cd Tables && (command -v timeout >/dev/null 2>&1 && timeout 60 soffice --headless --convert-to xlsx x1.csv x2.csv || soffice --headless --convert-to xlsx x1.csv x2.csv) >/dev/null 2>&1 ) || true
fi
rm -f Tables/x1.csv Tables/x2.csv
printf 'first\nsecond\n' > Projects/Alpha/notes.txt
( cd Projects/Alpha && G='git -c core.fsmonitor=false -c user.email=t@t -c user.name=t' && $G init -q && $G add notes.txt && $G commit -qm init && printf 'first\nsecond changed\n' > notes.txt ) 2>/dev/null || true
mkdir -p Many
for i in $(seq 1 400); do printf 'needle %s\n' "$i" > "Many/item_$i.txt"; done
touch_days_ago 45 Documents/file20.txt; touch_days_ago 400 Documents/file10.txt
fi
if [ "$MODE" = "search-hidden" ]; then
  mkdir -p Projects src gen dist node_modules/pkg
  git init -q
  printf 'dist/\n' > .gitignore
  printf '/gen/\n' > .gallignore
  printf 'shared_needle\n' > src/report_visible.ts
  printf 'shared_needle\nhidden_only_needle\n' > gen/report_generated.ts
  printf 'hidden_only_needle\n' > dist/xyzzy_bundle.ts
  printf 'hidden_only_needle\n' > node_modules/pkg/xyzzy_vendor.ts
fi
if [ "$MODE" = "notifications" ]; then
  mkdir -p "$CFG_DIR"
  printf 'broken settings fixture\n' > "$CFG_DIR/settings.json"
  printf '%s\n' '{"theme":"dark"}' > "$CFG_DIR/settings.backup.json"
fi
if [ "$MODE" = "bug-report" ]; then
  mkdir -p "$DATA_DIR"
  printf '%s' '{"unclean":true,"events":["command: 0ms list_dir failed"]}' > "$DATA_DIR/diagnostics.json"
fi
UPDATER_SRV_PID=""
cleanup_selftest() {
  [ -n "$UPDATER_SRV_PID" ] && kill "$UPDATER_SRV_PID" 2>/dev/null || true
  [ -d "$T/patch-import/wr/two" ] && chmod -R u+w "$T/patch-import/wr" 2>/dev/null || true
}
trap cleanup_selftest EXIT

if [ "$MODE" = "updates" ]; then
  mkdir -p "$T/keys" "$T/pkg" "$T/srv"
  (cd "$REPO_ROOT" && npm run --silent tauri -- signer generate -w "$T/keys/test.key" -p "" -f --ci >/dev/null) || { echo "Failed to generate test signing key"; exit 2; }
  TARGET_VER=$(grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d'"' -f2)
  cp "$APP" "$T/pkg/gall"
  strip --strip-debug "$T/pkg/gall" 2>/dev/null || true
  chmod +x "$T/pkg/gall"
  (cd "$REPO_ROOT" && npm run --silent tauri -- signer sign -f "$T/keys/test.key" -p "" --app-version "$TARGET_VER" "$T/pkg/gall" >/dev/null) || { echo "Failed to sign test binary"; exit 2; }
  tar -czf "$T/srv/Gall_${TARGET_VER}_linux_x86_64_update.tar.gz" -C "$T/pkg" gall gall.sig
  (cd "$REPO_ROOT" && npm run --silent tauri -- signer sign -f "$T/keys/test.key" -p "" --app-version "$TARGET_VER" "$T/srv/Gall_${TARGET_VER}_linux_x86_64_update.tar.gz" >/dev/null) || { echo "Failed to sign test archive"; exit 2; }
  touch "$T/fail-once"
  python3 - "$T" "$TARGET_VER" <<'PY' &
import http.server, json, os, sys
root, ver = sys.argv[1], sys.argv[2]
srv_dir = os.path.join(root, "srv")
fail_once = os.path.join(root, "fail-once")
pkg_name = f"Gall_{ver}_linux_x86_64_update.tar.gz"
sig_text = open(os.path.join(srv_dir, pkg_name + ".sig"), "r", encoding="utf-8").read().strip()

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        if self.path == "/latest.json":
            if os.path.exists(fail_once):
                try: os.remove(fail_once)
                except OSError: pass
                self.send_response(503)
                self.end_headers()
                self.wfile.write(b"Simulated transient endpoint outage")
                return
            port = self.server.server_address[1]
            manifest = {
                "version": ver,
                "notes": f"Signed in-app self-update to {ver}\nAutomatic background download verified.",
                "pub_date": "2026-11-20T12:00:00Z",
                "platforms": {
                    "linux-x86_64-bin": {
                        "url": f"http://127.0.0.1:{port}/{pkg_name}",
                        "signature": sig_text,
                    }
                },
            }
            body = json.dumps(manifest).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/" + pkg_name:
            data = open(os.path.join(srv_dir, pkg_name), "rb").read()
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
open(os.path.join(root, "http.port"), "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
  UPDATER_SRV_PID=$!
  for _ in $(seq 1 50); do [ -s "$T/http.port" ] && break; sleep 0.1; done
  PORT="$(cat "$T/http.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { echo "Updater test HTTP server did not start"; exit 2; }
  export GALL_UPDATER_TEST_VERSION="0.1.0"
  export GALL_UPDATER_TEST_FORCE_OVERLAY="1"
  export GALL_UPDATER_TEST_PUBKEY="$(cat "$T/keys/test.key.pub")"
  export GALL_UPDATER_TEST_ENDPOINT="http://127.0.0.1:$PORT/latest.json"
fi

# shellcheck source=scripts/gui-session.sh
. "$REPO_ROOT/scripts/gui-session.sh"

BACKEND="$(gall_gui_select_backend)" || exit $?
echo "Self-test mode: $(gall_gui_backend_label "$BACKEND") (override: GALL_GUI_BACKEND=... or GALL_SELFTEST_REAL=1)"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_SELFTEST="$MODE${GALL_SELFTEST_SCALE:+@$GALL_SELFTEST_SCALE}" GALL_SELFTEST_DIR="$T/out"
# The config folder above hides the machine's Git identity (~/.config/git/config), and repositories the app
# creates itself (Repository setup > Initialize) have none: give the commits of the self-test an author.
export GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@example.com" GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@example.com"
if [ "$MODE" = "updates" ]; then
  # Keep the graphical/D-Bus session alive across the in-app restart from base -> overlay.
  cat > "$T/run-app.sh" <<EOF
#!/bin/sh
"$APP" -p "$T/td" -p2 "$T/td/Projects" &
i=0
while [ ! -f "$T/out/report.txt" ] && [ \$i -lt 900 ]; do
  sleep 0.2
  i=\$((i + 1))
done
EOF
elif [ "$MODE" = "linux-live" ]; then
  {
    printf '#!/usr/bin/env bash\nexec '
    if [ "${#trash_mounts[@]}" -gt 0 ]; then
      printf '%q ' "${trash_mounts[@]}" --
    fi
    printf '%q ' "$APP" -p "$T/td" -p2 "$T/td/Projects"
    printf '\n'
  } > "$T/run-app.sh"
elif [ "$MODE" = "e2e-compare-scale" ] && [ -x /usr/bin/time ]; then
  printf '#!/bin/sh\nexec /usr/bin/time -f "Gall peak RSS: %%M KiB" "%s" -p "%s/td" -p2 "%s/td/Projects"\n' "$APP" "$T" "$T" > "$T/run-app.sh"
  uname -sm
  awk -F ': ' '/^model name/{print $2; exit}' /proc/cpuinfo
else
  printf '#!/bin/sh\nexec "%s" -p "%s/td" -p2 "%s/td/Projects"\n' "$APP" "$T" "$T" > "$T/run-app.sh"
fi
chmod +x "$T/run-app.sh"
GALL_GUI_BACKEND="$BACKEND" gall_gui_run "$T" 240 "$T/app.log" "$T/run-app.sh" || true
if { [ "$MODE" = "merge-ai" ] || [ "$MODE" = "git-workspace" ]; } && [ -f "$T/out/report.txt" ] && [ -f "$T/td/compare-ai-fake-path.txt" ]; then
  if pgrep -f "compare_ai_fake sleep" >/dev/null 2>&1; then
    echo "FAIL  merge-ai: cancelled compare_ai_fake sleep process is still running" >> "$T/out/report.txt"
    pkill -f "compare_ai_fake sleep" >/dev/null 2>&1 || true
  else
    echo "PASS  merge-ai: cancelled compare_ai_fake sleep process was killed (checked with ps)" >> "$T/out/report.txt"
  fi
fi
if [ "$MODE" = "e2e-compare-scale" ]; then
  awk '/Gall peak RSS:/{print}' "$T/app.log"
fi
if [ "$MODE" = "updates" ] && [ -f "$T/out/report.txt" ]; then
  python3 - "$T" <<'PY'
import json, os, re, sys
t = sys.argv[1]
active_path = os.path.join(t, "data", "gall", "updates", "active.json")
report_path = os.path.join(t, "out", "report.txt")
ok = False
detail = "missing active.json"
if os.path.exists(active_path):
    data = json.load(open(active_path))
    ok = data.get("verified") is True and data.get("bootAttempts") == 0
    detail = json.dumps(data)
report = open(report_path).read()
if not ok:
    report = f"FAIL  overlay marked verified in active.json  — {detail}\n" + report
    report = re.sub(r'(\d+) failure\(s\)', lambda m: f"{int(m.group(1)) + 1} failure(s)", report)
else:
    report = f"PASS  overlay marked verified in active.json  — {detail}\n" + report
open(report_path, "w").write(report)
PY
fi
cat "$T/out/report.txt" 2>/dev/null || { echo "NO REPORT"; tail -30 "$T/app.log"; exit 1; }
echo; echo "Screenshots: $T/out"
grep -q "^0 failure" "$T/out/report.txt"
