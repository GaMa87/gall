#!/usr/bin/env bash
# Regenerates docs/screenshots/*.png and *.gif from a debug build with a fictional home folder.
#   ./scripts/screenshots.sh [binary]
# Uses ImageMagick + LibreOffice on Linux, or native macOS CoreGraphics/ImageIO + Python on Darwin.
# Uses a private headless compositor when available (kwin_wayland, labwc, weston,
# sway, or Xvfb), or the active desktop (macOS WKWebView / Hyprland).
set -eu
REPO="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/gui-session.sh
. "$REPO/scripts/gui-session.sh"
IS_MACOS=0
[ "$(uname -s)" = "Darwin" ] && IS_MACOS=1

APP="$(realpath "${1:-$REPO/target/debug/gall}")"
if [ -d "$APP" ] && [ -x "$APP/Contents/MacOS/gall" ]; then
  APP="$APP/Contents/MacOS/gall"
fi
if [ "$IS_MACOS" -eq 0 ] && pgrep -x gall >/dev/null 2>&1; then
  echo "Gall is running; close it first."
  exit 2
fi

T="$(realpath "$(mktemp -d /tmp/g.XXX)")"
H="$T/alex"
mkdir -p "$H"/{Desktop,Documents/Invoices,Downloads,Music,Videos} "$H/Pictures/Iceland" "$H/Projects/website/assets" "$H/Projects/website/src" "$H/Projects/notes" "$T/config" "$T/data" "$T/out"
P="$H/Pictures/Iceland"
W="$H/Projects/website"

# Build small native macOS image/GIF helper when ImageMagick is absent.
IMG_TOOL=""
if ! command -v magick >/dev/null 2>&1 && [ "$IS_MACOS" -eq 1 ] && command -v swiftc >/dev/null 2>&1; then
  IMG_TOOL="$T/gall-img-tool"
  cat > "$T/gall-img-tool.swift" <<'SWIFT'
import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

func makeColor(_ hex: String) -> CGColor {
    let s = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
    let v = UInt64(s, radix: 16) ?? 0
    return CGColor(
        red: CGFloat((v >> 16) & 0xff) / 255.0,
        green: CGFloat((v >> 8) & 0xff) / 255.0,
        blue: CGFloat(v & 0xff) / 255.0,
        alpha: 1.0
    )
}

func saveImage(_ img: CGImage, to path: String, uti: UTType) {
    let url = URL(fileURLWithPath: path)
    guard let dst = CGImageDestinationCreateWithURL(url as CFURL, uti.identifier as CFString, 1, nil) else {
        fputs("Failed to create image destination: \(path)\n", stderr)
        exit(1)
    }
    CGImageDestinationAddImage(dst, img, nil)
    if !CGImageDestinationFinalize(dst) {
        fputs("Failed to finalize image: \(path)\n", stderr)
        exit(1)
    }
}

func loadImage(_ path: String) -> CGImage {
    let url = URL(fileURLWithPath: path)
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
        fputs("Failed to load image: \(path)\n", stderr)
        exit(1)
    }
    return img
}

func renderGradient(w: Int, h: Int, c1: String, c2: String, radial: Bool, edited: Bool, out: String) {
    let cs = CGColorSpaceCreateDeviceRGB()
    let ctx = CGContext(
        data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
        space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    let grad = CGGradient(colorsSpace: cs, colors: [makeColor(c1), makeColor(c2)] as CFArray, locations: [0, 1])!
    if radial {
        let center = CGPoint(x: CGFloat(w) * 0.5, y: CGFloat(h) * 0.52)
        ctx.drawRadialGradient(grad, startCenter: center, startRadius: 10, endCenter: center, endRadius: CGFloat(max(w, h)) * 0.65, options: [.drawsAfterEndLocation])
    } else {
        ctx.drawLinearGradient(grad, start: CGPoint(x: 0, y: CGFloat(h)), end: CGPoint(x: CGFloat(w), y: 0), options: [])
    }
    if edited {
        ctx.setFillColor(makeColor("#1b2f4d"))
        ctx.fillEllipse(in: CGRect(x: 560, y: CGFloat(h) - 560, width: 280, height: 280))
        ctx.setFillColor(makeColor("#f7d774"))
        ctx.fillEllipse(in: CGRect(x: 240, y: CGFloat(h) - 1110, width: 120, height: 120))
    }
    saveImage(ctx.makeImage()!, to: out, uti: .jpeg)
}

func resizeImage(_ img: CGImage, maxDim: Int) -> CGImage {
    let iw = img.width, ih = img.height
    if iw <= maxDim && ih <= maxDim { return img }
    let scale = min(Double(maxDim) / Double(iw), Double(maxDim) / Double(ih))
    let nw = max(1, Int(round(Double(iw) * scale)))
    let nh = max(1, Int(round(Double(ih) * scale)))
    let cs = CGColorSpaceCreateDeviceRGB()
    let ctx = CGContext(data: nil, width: nw, height: nh, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: nw, height: nh))
    return ctx.makeImage()!
}

let args = CommandLine.arguments
guard args.count >= 2 else { exit(2) }
switch args[1] {
case "gen":
    // gen <w> <h> <c1> <c2> <linear|radial> <0|1 edited> <out>
    renderGradient(w: Int(args[2])!, h: Int(args[3])!, c1: args[4], c2: args[5], radial: args[6] == "radial", edited: args[7] == "1", out: args[8])
case "resize":
    // resize <in> <maxDim> <out>
    let img = loadImage(args[2])
    saveImage(resizeImage(img, maxDim: Int(args[3])!), to: args[4], uti: .png)
case "crop":
    // crop <in> <x> <y> <w> <h> <vw> <out>
    let img = loadImage(args[2])
    let x = Double(args[3])!, y = Double(args[4])!, w = Double(args[5])!, h = Double(args[6])!, vw = Double(args[7])!
    let s = Double(img.width) / vw
    let rect = CGRect(x: round(x * s), y: round(y * s), width: round(w * s), height: round(h * s))
    let clipped = rect.intersection(CGRect(x: 0, y: 0, width: img.width, height: img.height))
    guard let cropped = img.cropping(to: clipped) else { exit(1) }
    saveImage(cropped, to: args[8], uti: .png)
case "gif":
    // gif <out> <maxDim> <frame1> <frame2> ...
    let outPath = args[2]
    let maxDim = Int(args[3])!
    let frames = Array(args[4...])
    let url = URL(fileURLWithPath: outPath)
    guard let dst = CGImageDestinationCreateWithURL(url as CFURL, UTType.gif.identifier as CFString, frames.count, nil) else { exit(1) }
    let fileProps: [CFString: Any] = [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]]
    CGImageDestinationSetProperties(dst, fileProps as CFDictionary)
    for (idx, path) in frames.enumerated() {
        let img = resizeImage(loadImage(path), maxDim: maxDim)
        let delay = (idx == frames.count - 1) ? 2.5 : 0.7
        let frameProps: [CFString: Any] = [
            kCGImagePropertyGIFDictionary: [
                kCGImagePropertyGIFDelayTime: delay,
                kCGImagePropertyGIFUnclampedDelayTime: delay
            ]
        ]
        CGImageDestinationAddImage(dst, img, frameProps as CFDictionary)
    }
    if !CGImageDestinationFinalize(dst) { exit(1) }
default:
    exit(2)
}
SWIFT
  swiftc -O "$T/gall-img-tool.swift" -o "$IMG_TOOL"
fi

if command -v magick >/dev/null 2>&1; then
  magick -size 1600x1000 gradient:'#0f2d4a-#7fc8e8' -swirl 30 "$P/Glacier lagoon.jpg"
  magick -size 1600x1000 gradient:'#1d1035-#e86f51' "$P/Midnight sun.jpg"
  magick -size 1200x1200 plasma:'#2e7d32-#a5d6a7' -blur 0x2 "$P/Moss fields.jpg"
  magick -size 1600x900 gradient:'#222831-#76838f' -wave 40x400 "$P/Black beach.jpg"
  magick -size 1000x1400 radial-gradient:'#fdf6e3-#3a6ea5' "$P/Aurora.jpg"
  magick -size 1400x900 plasma:'#455a64-#cfd8dc' "$P/Basalt columns.jpg"
  magick "$P/Aurora.jpg" -fill '#1b2f4d' -draw 'circle 700,420 700,560' -fill '#f7d774' -draw 'circle 300,1050 300,1110' "$P/Aurora (edited).jpg"
elif [ -n "$IMG_TOOL" ]; then
  "$IMG_TOOL" gen 1600 1000 '#0f2d4a' '#7fc8e8' linear 0 "$P/Glacier lagoon.jpg"
  "$IMG_TOOL" gen 1600 1000 '#1d1035' '#e86f51' linear 0 "$P/Midnight sun.jpg"
  "$IMG_TOOL" gen 1200 1200 '#2e7d32' '#a5d6a7' linear 0 "$P/Moss fields.jpg"
  "$IMG_TOOL" gen 1600 900  '#222831' '#76838f' linear 0 "$P/Black beach.jpg"
  "$IMG_TOOL" gen 1000 1400 '#fdf6e3' '#3a6ea5' radial 0 "$P/Aurora.jpg"
  "$IMG_TOOL" gen 1400 900  '#455a64' '#cfd8dc' linear 0 "$P/Basalt columns.jpg"
  "$IMG_TOOL" gen 1000 1400 '#fdf6e3' '#3a6ea5' radial 1 "$P/Aurora (edited).jpg"
else
  echo "Need either ImageMagick (magick) or macOS swiftc." >&2
  exit 1
fi

printf '<!DOCTYPE html>\n<html lang="en">\n<head><meta charset="utf-8"><title>Iceland trip</title></head>\n<body>\n  <h1>Iceland 2026</h1>\n</body>\n</html>\n' > "$W/index.html"
printf 'body { font-family: system-ui; margin: 0 auto; max-width: 60rem; }\n' > "$W/style.css"
printf 'export const photos = ["glacier", "aurora", "basalt"];\n' > "$W/src/gallery.js"
printf '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#3d8bfd"/></svg>\n' > "$W/logo.svg"
printf '# Website\n\nStatic photo gallery.\n' > "$W/README.md"
printf 'Book ferry to Heimaey\n' > "$H/Projects/notes/Book ferry.2do"
printf 'Invoice 2026-031\n' > "$H/Documents/Invoices/2026-031.txt"
for f in Report.pdf Budget.ods Letter.odt; do head -c 20000 /dev/urandom > "$H/Documents/$f"; done

# Initialize realistic Git history in ~/Projects/website for inline Git status & Git Workspace screenshot.
(
  cd "$W"
  git init -q -b main
  git config user.name "Alex Novak"
  git config user.email "alex@example.com"
  git config commit.gpgSign false
  git config core.hooksPath /dev/null
  git add README.md logo.svg
  GIT_AUTHOR_DATE="2026-11-10T10:15:00Z" GIT_COMMITTER_DATE="2026-11-10T10:15:00Z" \
    git commit -q -m "Initial site structure and vector mark"
  git add index.html style.css
  GIT_AUTHOR_DATE="2026-11-12T14:20:00Z" GIT_COMMITTER_DATE="2026-11-12T14:20:00Z" \
    git commit -q -m "Add Iceland 2026 landing layout and base typography"
  git tag -a v0.1 -m "First preview"
  git checkout -q -b feature/gallery-grid
  git add src/gallery.js
  GIT_AUTHOR_DATE="2026-11-15T16:40:00Z" GIT_COMMITTER_DATE="2026-11-15T16:40:00Z" \
    git commit -q -m "Export Iceland photo manifest for gallery module"
  git checkout -q main
  printf 'body { font-family: system-ui; margin: 0 auto; max-width: 64rem; color: #e7e9ee; background: #0d0f13; }\n' > "$W/style.css"
  git add style.css
  GIT_AUTHOR_DATE="2026-11-18T09:10:00Z" GIT_COMMITTER_DATE="2026-11-18T09:10:00Z" \
    git commit -q -m "Tune dark palette and readable line measure"
  GIT_AUTHOR_DATE="2026-11-20T18:05:00Z" GIT_COMMITTER_DATE="2026-11-20T18:05:00Z" \
    git merge -q --no-ff feature/gallery-grid -m "Merge branch 'feature/gallery-grid'"
  git tag -a v1.0 -m "Public launch"
)

# Compare demos: an older page, two budgets (Excel) and an edited photo.
printf '<!DOCTYPE html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n  <meta name="viewport" content="width=device-width">\n  <title>Iceland trip</title>\n  <link rel="stylesheet" href="style.css">\n</head>\n<body>\n  <h1>Iceland 2026</h1>\n  <p>Ten days around the Ring Road.</p>\n  <script type="module" src="src/gallery.js"></script>\n</body>\n</html>\n' > "$W/index.html"
printf '<!DOCTYPE html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n  <title>Iceland</title>\n</head>\n<body>\n  <h1>Iceland 2025</h1>\n  <p>A week around the Ring Road.</p>\n  <script src="gallery.js"></script>\n</body>\n</html>\n' > "$W/index.old.html"

printf 'item,category,amount,month\nFerry,Travel,120,June\nRental car,Travel,640,June\nGuesthouse,Lodging,900,June\nCamera lens,Gear,450,May\nHiking boots,Gear,180,May\n' > "$T/Budget 2025.csv"
printf 'item,category,amount,month\nFerry,Travel,135,June\nRental car,Travel,640,July\nGuesthouse,Lodging,1150,July\nHiking boots,Gear,180,May\nGlacier tour,Activities,220,July\n' > "$T/Budget 2026.csv"
if command -v soffice >/dev/null 2>&1; then
  ( cd "$T" && timeout 60 soffice --headless --convert-to xlsx "Budget 2025.csv" "Budget 2026.csv" >/dev/null 2>&1 \
    && mv "Budget 2025.xlsx" "Budget 2026.xlsx" "$H/Documents/" ) || true
fi
if [ ! -f "$H/Documents/Budget 2025.xlsx" ]; then
  python3 - "$T" "$H/Documents" <<'PY'
import csv, pathlib, sys, zipfile
from xml.sax.saxutils import escape

src_dir, dst_dir = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
for name in ("Budget 2025", "Budget 2026"):
    rows = list(csv.reader((src_dir / f"{name}.csv").open()))
    sheet_rows = []
    for r_idx, row in enumerate(rows, 1):
        cells = []
        for c_idx, val in enumerate(row):
            col = chr(ord("A") + c_idx)
            ref = f"{col}{r_idx}"
            if r_idx > 1 and val.isdigit():
                cells.append(f'<c r="{ref}"><v>{val}</v></c>')
            else:
                cells.append(f'<c r="{ref}" t="inlineStr"><is><t>{escape(val)}</t></is></c>')
        sheet_rows.append(f'<row r="{r_idx}">{"".join(cells)}</row>')
    sheet_xml = (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f'<sheetData>{"".join(sheet_rows)}</sheetData></worksheet>'
    )
    with zipfile.ZipFile(dst_dir / f"{name}.xlsx", "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml",
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '</Types>')
        zf.writestr("_rels/.rels",
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            '</Relationships>')
        zf.writestr("xl/workbook.xml",
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
            '<sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>')
        zf.writestr("xl/_rels/workbook.xml.rels",
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
            '</Relationships>')
        zf.writestr("xl/worksheets/sheet1.xml", sheet_xml)
PY
fi

# Isolate macOS runs in a disposable portable copy so the user's real profile is never touched.
if [ "$IS_MACOS" -eq 1 ]; then
  mkdir -p "$T/app/UserData"
  cp -c "$APP" "$T/app/gall" 2>/dev/null || cp "$APP" "$T/app/gall"
  chmod +x "$T/app/gall"
  : > "$T/app/portable"
  APP="$T/app/gall"
  export GALL_MULTI_INSTANCE=1
fi

# Keep the user's fonts (Nerd Font glyphs) visible; XDG_DATA_HOME is redirected below.
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export HOME="$H" USER=alex XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data"
export GALL_SELFTEST=demo GALL_SELFTEST_DIR="$T/out"
unset XDG_DESKTOP_DIR XDG_DOCUMENTS_DIR XDG_DOWNLOAD_DIR XDG_PICTURES_DIR XDG_MUSIC_DIR XDG_VIDEOS_DIR
BACKEND="$(gall_gui_select_backend)" || exit $?
echo "Screenshot mode: $(gall_gui_backend_label "$BACKEND")"
printf '#!/bin/sh\nexec "%s" -p "%s" -p2 "%s"\n' "$APP" "$P" "$W" > "$T/run-app.sh"
chmod +x "$T/run-app.sh"
GALL_GUI_BACKEND="$BACKEND" gall_gui_run "$T" 300 "$T/app.log" "$T/run-app.sh" || true
mkdir -p "$REPO/docs/screenshots"
G="$T/gif"; mkdir -p "$G"

for f in "$T"/out/*.png; do
  [ -e "$f" ] || continue
  n="$(basename "$f" .png)"
  if [[ "$n" == gif-* ]]; then cp "$f" "$G/$n.png"; continue; fi
  c="$(grep "^CROP $n " "$T/out/report.txt" 2>/dev/null | head -1 || true)"
  if [ -n "$c" ]; then
    read -r _ _ x y w h vw <<<"$c"
    if command -v magick >/dev/null 2>&1; then
      iw="$(magick identify -format '%w' "$f")"
      g="$(awk -v x="$x" -v y="$y" -v w="$w" -v h="$h" -v k="$iw" -v vw="$vw" 'BEGIN{s=k/vw; printf "%dx%d+%d+%d", w*s, h*s, x*s, y*s}')"
      magick "$f" -crop "$g" +repage "$REPO/docs/screenshots/$n.png"
    else
      "$IMG_TOOL" crop "$f" "$x" "$y" "$w" "$h" "$vw" "$REPO/docs/screenshots/$n.png"
    fi
  else
    if command -v magick >/dev/null 2>&1; then
      magick "$f" -resize '1600x1600>' "$REPO/docs/screenshots/$n.png"
    else
      "$IMG_TOOL" resize "$f" 1600 "$REPO/docs/screenshots/$n.png"
    fi
  fi
done

for name in $(ls "$G" 2>/dev/null | sed -E 's/^gif-(.*)-[0-9]+\.png$/\1/' | sort -u); do
  frames=("$G"/gif-"$name"-*.png)
  if command -v magick >/dev/null 2>&1; then
    last="${frames[-1]}"; unset 'frames[-1]'
    magick -delay 70 "${frames[@]}" -delay 250 "$last" -loop 0 -resize '1000x1000>' \
      -dither None -colors 128 -layers OptimizePlus "$REPO/docs/screenshots/$name.gif"
  else
    "$IMG_TOOL" gif "$REPO/docs/screenshots/$name.gif" 1000 "${frames[@]}"
  fi
done

ls -la "$REPO/docs/screenshots"
echo "work dir: $T"
