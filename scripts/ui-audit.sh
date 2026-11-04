#!/usr/bin/env bash
# Exploratory UI audit: drives a debug build through hard cases (long and foreign names,
# huge numbers, 5000 items, deep paths, folders that refuse or vanish, small windows,
# both themes, many tabs, notification floods, dialogs and menus at the edges) and
# checks every state for overlapping or cut-off text and controls outside the window.
#   ./scripts/ui-audit.sh [binary]     default: target/debug/gall
# Needs a build with the bundled UI for reliable results (see CONTRIBUTING.md).
# Requires KDE Spectacle. Close Gall first. Prints the report; screenshots in out/.
set -u
APP="$(realpath "${1:-target/debug/gall}")"
if pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-audit-XXXX)"
mkdir -p "$T/config" "$T/data" "$T/out" "$T/td"
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
cd "$T/td"
mkdir -p Names Big Trouble/Vanish/inner Deep Other
(
  cd Names
  echo x > "A very long file name that goes on and on and describes the content in far too much detail for any column - final version (2) (copy).pdf"
  echo x > "😀 emoji party 🎉.txt"
  echo x > "日本語のファイル名とても長い名前のテスト.txt"
  echo x > "ملف عربي طويل للاختبار.txt"
  echo x > "  leading spaces.txt"
  echo x > ".hidden-file"
  echo x > "no_extension"
  echo x > "UPPER CASE NAME.TXT"
  echo x > "file.averyveryverylongextension"
  mkdir -p "A folder with a really long name that should be shortened nicely in every view"
  truncate -s 5T huge-disk-image.img
  echo old > ancient.txt; touch -d '1971-01-01' ancient.txt
  echo future > future.txt; touch -d '2099-12-31' future.txt
)
for i in $(seq -w 1 5000); do : > "Big/file-$i.txt"; done
d=Deep; for i in $(seq -w 1 25); do d="$d/level-$i-folder"; done; mkdir -p "$d"; echo x > "$d/bottom.txt"
mkdir -p Trouble/Locked; echo x > Trouble/Locked/secret; chmod 000 Trouble/Locked
ln -s /nonexistent/target Trouble/broken-link
ln -s loop Trouble/loop
echo x > Trouble/Vanish/inner/f.txt
cd "$T"
export GALL_SELFTEST_DBUS="${DBUS_SESSION_BUS_ADDRESS:-}"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_SELFTEST=audit GALL_SELFTEST_DIR="$T/out"
timeout 400 dbus-run-session -- "$APP" -p "$T/td" -p2 "$T/td/Other" > "$T/app.log" 2>&1
chmod 755 "$T/td/Trouble/Locked" 2>/dev/null
if [ -f "$T/out/report.txt" ]; then cat "$T/out/report.txt"; else echo "NO REPORT"; tail -20 "$T/app.log"; fi
echo "Screenshots: $T/out"
