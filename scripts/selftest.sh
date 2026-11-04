#!/usr/bin/env bash
# UI self-test: drives a debug build of Gall against disposable test data
# (own D-Bus session, config and data dirs) and writes a report + screenshots.
#   ./scripts/selftest.sh [binary]      default: target/debug/gall
# Requires: KDE Spectacle (screenshots), ImageMagick, zip. Close Gall first.
set -u
APP="$(realpath "${1:-target/debug/gall}")"
if pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-selftest-XXXX)"
mkdir -p "$T/config" "$T/data" "$T/out" "$T/td"
# Keep the user's fonts visible (XDG_DATA_HOME is redirected below); set
# GALL_SELFTEST_NOFONTS=1 to test the fallbacks for missing fonts.
[ -z "${GALL_SELFTEST_NOFONTS:-}" ] && [ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
cd "$T/td"
mkdir -p Projects/Alpha/src Projects/Beta Photos Music "Documents/Reports 2026"
printf '# Alpha\n\nA test project.\n' > Projects/Alpha/README.md
for i in 1 2 10 20; do echo "line $i" > "Documents/file$i.txt"; done
echo "Buy milk" > Documents/Shopping.2do
magick -size 640x400 gradient:'#2b3a55-#e9b44c' Photos/sunset.jpg
magick -size 300x300 plasma:fractal Photos/plasma.png
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
touch -d '3 days ago' Sync/R/newer-left.txt Sync/L/newer-right.txt
echo aaaa > Sync/L/samesize.txt; echo bbbb > Sync/R/samesize.txt; touch -d '2 days ago' Sync/L/samesize.txt Sync/R/samesize.txt
echo L > Sync/L/only-left.txt
mkdir -p Sync/R/only-right-dir; echo R > Sync/R/only-right-dir/f.txt
# merge / images / similarity matrix
mkdir -p Merge Variants Images
# conflict: a folder on both sides (merge), one file in both
mkdir -p Sizes/small Sizes/big/deep; head -c 300 /dev/zero > Sizes/small/a.bin; head -c 900 /dev/zero > Sizes/big/b.bin; head -c 600 /dev/zero > Sizes/big/deep/c.bin
mkdir -p Rename; for i in 1 2 3; do echo "$i" > "Rename/IMG_000$i.HEIC"; done
mkdir -p MergeDirs/L/Photos MergeDirs/R/Photos
echo new > MergeDirs/L/Photos/a.txt; echo old > MergeDirs/R/Photos/a.txt
echo l > MergeDirs/L/Photos/l.txt; echo r > MergeDirs/R/Photos/r.txt
printf 'alpha\nbeta\ngamma\ndelta\n' > Merge/left.txt
printf 'alpha\nBETA\ngamma\ndelta\nepsilon\n' > Merge/right.txt
magick -size 320x200 gradient:'#2b3a55-#e9b44c' Images/a.png
magick Images/a.png -fill red -draw 'rectangle 40,40 120,100' Images/b.png
for i in 1 2 3 4; do seq 1 60 > Variants/v$i.txt; done
sed -i 's/^10$/ten/' Variants/v2.txt; sed -i 's/^20$/twenty/;s/^30$/thirty/' Variants/v3.txt
seq 100 140 > Variants/odd.txt
# tables: reordered columns, removed/added column, key rows, number formats
mkdir -p Tables
printf 'id;name;price;old\n1;Apple;1.0;x\n2;Pear;2,50;y\n3;Plum;3;z\n' > Tables/a.csv
printf 'name;id;price;stock\nPlum;3;3.5;9\nApple;1;1;5\nKiwi;4;4;1\n' > Tables/b.csv
printf 'sku,qty,city\nA1,10,Ljubljana\nB2,5,Maribor\nC3,7,Celje\n' > Tables/x1.csv
printf 'sku,qty,city\nA1,10,Ljubljana\nB2,6,Maribor\nD4,1,Koper\n' > Tables/x2.csv
( cd Tables && timeout 60 soffice --headless --convert-to xlsx x1.csv x2.csv >/dev/null 2>&1 ) || true
rm -f Tables/x1.csv Tables/x2.csv
printf 'first\nsecond\n' > Projects/Alpha/notes.txt
( cd Projects/Alpha && G='git -c core.fsmonitor=false -c user.email=t@t -c user.name=t' && $G init -q && $G add notes.txt && $G commit -qm init && printf 'first\nsecond changed\n' > notes.txt ) 2>/dev/null || true
mkdir -p Many
for i in $(seq 1 400); do printf 'needle %s\n' "$i" > "Many/item_$i.txt"; done
touch -d '45 days ago' Documents/file20.txt; touch -d '400 days ago' Documents/file10.txt
export GALL_SELFTEST_DBUS="${DBUS_SESSION_BUS_ADDRESS:-}"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_SELFTEST=1 GALL_SELFTEST_DIR="$T/out"
# Services activated on the private bus (portals, ksecretd, ...) outlive dbus-run-session;
# remember the bus address and stop everything that still uses it afterwards.
timeout 240 dbus-run-session -- sh -c 'echo "$DBUS_SESSION_BUS_ADDRESS" > "$0/bus"; exec "$1" -p "$0/td" -p2 "$0/td/Projects"' "$T" "$APP" > "$T/app.log" 2>&1
BUS="$(cat "$T/bus" 2>/dev/null)"
if [ -n "$BUS" ]; then
  for p in /proc/[0-9]*; do
    tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -qxF "DBUS_SESSION_BUS_ADDRESS=$BUS" && kill "${p#/proc/}" 2>/dev/null
  done
fi
cat "$T/out/report.txt" 2>/dev/null || { echo "NO REPORT"; tail -30 "$T/app.log"; exit 1; }
echo; echo "Screenshots: $T/out"
grep -q "^0 failure" "$T/out/report.txt"
