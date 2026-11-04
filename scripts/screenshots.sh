#!/usr/bin/env bash
# Regenerates docs/screenshots/*.png from a debug build with a fictional home folder.
#   ./scripts/screenshots.sh [binary]     (KDE Spectacle, ImageMagick; close Gall first)
set -eu
REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP="$(realpath "${1:-$REPO/target/debug/gall}")"
if pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-demo-XXXX)"; H="$T/home/alex"
mkdir -p "$H"/{Desktop,Documents/Invoices,Downloads,Music,Videos} "$H/Pictures/Iceland" "$H/Projects/website/assets" "$H/Projects/website/src" "$H/Projects/notes" "$T/config" "$T/data" "$T/out"
P="$H/Pictures/Iceland"
magick -size 1600x1000 gradient:'#0f2d4a-#7fc8e8' -swirl 30 "$P/Glacier lagoon.jpg"
magick -size 1600x1000 gradient:'#1d1035-#e86f51' "$P/Midnight sun.jpg"
magick -size 1200x1200 plasma:'#2e7d32-#a5d6a7' -blur 0x2 "$P/Moss fields.jpg"
magick -size 1600x900 gradient:'#222831-#76838f' -wave 40x400 "$P/Black beach.jpg"
magick -size 1000x1400 radial-gradient:'#fdf6e3-#3a6ea5' "$P/Aurora.jpg"
magick -size 1400x900 plasma:'#455a64-#cfd8dc' "$P/Basalt columns.jpg"
W="$H/Projects/website"
printf '<!DOCTYPE html>\n<html lang="en">\n<head><meta charset="utf-8"><title>Iceland trip</title></head>\n<body>\n  <h1>Iceland 2026</h1>\n</body>\n</html>\n' > "$W/index.html"
printf 'body { font-family: system-ui; margin: 0 auto; max-width: 60rem; }\n' > "$W/style.css"
printf 'export const photos = ["glacier", "aurora", "basalt"];\n' > "$W/src/gallery.js"
printf '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#3d8bfd"/></svg>\n' > "$W/logo.svg"
printf '# Website\n\nStatic photo gallery.\n' > "$W/README.md"
printf 'Book ferry to Heimaey\n' > "$H/Projects/notes/Book ferry.2do"
printf 'Invoice 2026-031\n' > "$H/Documents/Invoices/2026-031.txt"
for f in Report.pdf Budget.ods Letter.odt; do head -c 20000 /dev/urandom > "$H/Documents/$f"; done
# Compare demos: an older page, two budgets (Excel, needs LibreOffice) and an edited photo.
printf '<!DOCTYPE html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n  <meta name="viewport" content="width=device-width">\n  <title>Iceland trip</title>\n  <link rel="stylesheet" href="style.css">\n</head>\n<body>\n  <h1>Iceland 2026</h1>\n  <p>Ten days around the Ring Road.</p>\n  <script type="module" src="src/gallery.js"></script>\n</body>\n</html>\n' > "$W/index.html"
printf '<!DOCTYPE html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n  <title>Iceland</title>\n</head>\n<body>\n  <h1>Iceland 2025</h1>\n  <p>A week around the Ring Road.</p>\n  <script src="gallery.js"></script>\n</body>\n</html>\n' > "$W/index.old.html"
printf 'item,category,amount,month\nFerry,Travel,120,June\nRental car,Travel,640,June\nGuesthouse,Lodging,900,June\nCamera lens,Gear,450,May\nHiking boots,Gear,180,May\n' > "$T/Budget 2025.csv"
printf 'item,category,amount,month\nFerry,Travel,135,June\nRental car,Travel,640,July\nGuesthouse,Lodging,1150,July\nHiking boots,Gear,180,May\nGlacier tour,Activities,220,July\n' > "$T/Budget 2026.csv"
( cd "$T" && timeout 60 soffice --headless --convert-to xlsx "Budget 2025.csv" "Budget 2026.csv" >/dev/null 2>&1 \
  && mv "Budget 2025.xlsx" "Budget 2026.xlsx" "$H/Documents/" ) || echo "LibreOffice not available: Excel compare skipped"
magick "$P/Aurora.jpg" -fill '#1b2f4d' -draw 'circle 700,420 700,560' -fill '#f7d774' -draw 'circle 300,1050 300,1110' "$P/Aurora (edited).jpg"
# Keep the user's fonts (Nerd Font glyphs) visible; XDG_DATA_HOME is redirected below.
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export HOME="$H" USER=alex XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data"
export GALL_SELFTEST=demo GALL_SELFTEST_DIR="$T/out" GALL_SELFTEST_DBUS="${DBUS_SESSION_BUS_ADDRESS:-}"
unset XDG_DESKTOP_DIR XDG_DOCUMENTS_DIR XDG_DOWNLOAD_DIR XDG_PICTURES_DIR XDG_MUSIC_DIR XDG_VIDEOS_DIR
timeout 300 dbus-run-session -- "$APP" -p "$P" -p2 "$W" > "$T/app.log" 2>&1 || true
mkdir -p "$REPO/docs/screenshots"
G="$T/gif"; mkdir -p "$G"
# The demo sets a fixed window size and Spectacle captures exactly the (undecorated) window.
for f in "$T"/out/*.png; do
  n="$(basename "$f" .png)"
  if [[ "$n" == gif-* ]]; then cp "$f" "$G/$n.png"; continue; fi
  # CROP <name> <x> <y> <w> <h> <viewport width> lines from the demo cut a UI region out.
  c="$(grep "^CROP $n " "$T/out/report.txt" 2>/dev/null | head -1 || true)"
  if [ -n "$c" ]; then
    read -r _ _ x y w h vw <<<"$c"
    iw="$(magick identify -format '%w' "$f")"
    g="$(awk -v x="$x" -v y="$y" -v w="$w" -v h="$h" -v k="$iw" -v vw="$vw" 'BEGIN{s=k/vw; printf "%dx%d+%d+%d", w*s, h*s, x*s, y*s}')"
    magick "$f" -crop "$g" +repage "$REPO/docs/screenshots/$n.png"
  else
    magick "$f" -resize '1600x1600>' "$REPO/docs/screenshots/$n.png"
  fi
done
# Frames gif-<name>-NN.png become docs/screenshots/<name>.gif (last frame held longer).
for name in $(ls "$G" 2>/dev/null | sed -E 's/^gif-(.*)-[0-9]+\.png$/\1/' | sort -u); do
  frames=("$G"/gif-"$name"-*.png)
  last="${frames[-1]}"; unset 'frames[-1]'
  magick -delay 70 "${frames[@]}" -delay 250 "$last" -loop 0 -resize '1000x1000>' \
    -dither None -colors 128 -layers OptimizePlus "$REPO/docs/screenshots/$name.gif"
done
ls -la "$REPO/docs/screenshots"; echo "work dir: $T"
