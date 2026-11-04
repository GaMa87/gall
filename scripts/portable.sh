#!/usr/bin/env bash
# Builds the portable Windows release on Linux (cross-compiled with cargo-xwin):
# a zip with gall.exe and a `portable` marker, so settings, data, logs and the
# webview's data stay in .\UserData next to the exe. Linux twin of portable.ps1.
#   ./scripts/portable.sh            build, then pack
#   ./scripts/portable.sh --no-build pack an existing cross-built gall.exe
# Requires: clang, lld, llvm (lld-link, llvm-lib), zip, the Rust target
# x86_64-pc-windows-msvc and cargo-xwin (`cargo install --locked cargo-xwin`).
# The first build downloads the MSVC CRT and Windows SDK (accepting Microsoft's licence).
# Output: dist/Gall-<version>-windows-x64-portable.zip
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
TARGET=x86_64-pc-windows-msvc
VERSION="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"

if [ "${1:-}" != "--no-build" ]; then
  for t in clang lld-link llvm-lib zip cargo-xwin; do
    command -v "$t" >/dev/null || { echo "missing: $t (see the header of this script)"; exit 2; }
  done
  rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
  # Release exe only: the frontend is embedded in it, no installer needed.
  npx tauri build --runner cargo-xwin --target "$TARGET" --no-bundle
fi

EXE="target/$TARGET/release/gall.exe"
[ -f "$EXE" ] || { echo "$EXE not found; run without --no-build"; exit 1; }

NAME="Gall-$VERSION-windows-x64-portable"
STAGE="dist/$NAME"
rm -rf "$STAGE" "dist/$NAME.zip"
mkdir -p "$STAGE"
cp "$EXE" "$STAGE/gall.exe"
: > "$STAGE/portable"
cp LICENSE THIRD-PARTY-NOTICES.md "$STAGE/"
# Same README.txt as portable.ps1, with Windows line endings.
sed 's/$/\r/' > "$STAGE/README.txt" <<EOF
Gall $VERSION, portable

Run gall.exe. Nothing is installed and nothing is written to your user profile:
settings, data, logs and the browser engine's cache live in the UserData folder
next to gall.exe, so the whole folder can be moved or kept on a USB drive.

Delete the file named "portable" to use the normal per-user locations instead
(%APPDATA%\\gall and %LOCALAPPDATA%).

Requires the Microsoft Edge WebView2 Runtime, which Windows 10 and 11 include.
If gall.exe does not start, install it from
https://developer.microsoft.com/microsoft-edge/webview2/

https://github.com/GaMa87/gall
EOF
(cd dist && zip -qr "$NAME.zip" "$NAME")
echo "Built dist/$NAME.zip ($(du -h "dist/$NAME.zip" | cut -f1))"
