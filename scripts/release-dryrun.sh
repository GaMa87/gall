#!/usr/bin/env bash
# Dry run of the release build, before a tag is pushed. Run it from a checkout of the
# commit you are about to tag (uncommitted changes are included).
#
#   scripts/release-dryrun.sh            # lint workflow + key checks + full Linux build
#   scripts/release-dryrun.sh --quick    # lint workflow + key checks only (seconds)
#
# It takes the real steps out of .github/workflows/release.yml (so the two cannot drift),
# runs them in a debian:trixie container like the Linux release CI job, with a throwaway
# signing key, verifies the bundled WebKitGTK security baseline, and checks the packages. It cannot build Windows or macOS: for those, run the Release
# workflow on GitHub with "dry_run" ticked (see the header of release.yml).
#
# --quick needs no container engine: it lints with a native `actionlint` when there is one (else
# through podman or docker). The full run needs a working podman or docker and network access.
# First run ~15 minutes; later runs reuse volumes named gall-dryrun-* (remove them with
# `podman volume rm`).
set -euo pipefail

quick=0
[[ "${1:-}" == "--quick" ]] && quick=1

root=$(git rev-parse --show-toplevel)
cd "$root"
engine=${CONTAINER_ENGINE:-$(command -v podman || command -v docker || true)}
workflow=.github/workflows/release.yml
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail() { echo "DRY RUN FAILED: $*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

# A container engine that answers (a docker CLI without a running daemon does not count).
engine_works() { [[ -n "$engine" ]] && "$engine" info >/dev/null 2>&1; }
no_engine_msg() {
  if [[ -z "$engine" ]]; then echo "no podman or docker found"
  else echo "$engine does not respond (is its daemon or socket running?)"; fi
}
# The full run builds in a container: say so now, not after the lint and the speed tests.
if (( ! quick )) && ! engine_works; then
  echo "$(no_engine_msg). The full dry run builds in a container: start podman or docker, or use --quick." >&2
  exit 1
fi

# The `run:` text of one named step of the workflow, dedented.
extract() {
  python3 - "$workflow" "$1" <<'PY'
import sys
lines = open(sys.argv[1]).read().split("\n")
name = "- name: " + sys.argv[2]
start = next((i for i, l in enumerate(lines) if l.strip() == name), None)
if start is None:
    sys.exit("step not found: " + sys.argv[2])
run = next(i for i in range(start, len(lines)) if lines[i].strip() == "run: |")
indent = len(lines[run]) - len(lines[run].lstrip()) + 2
out = []
for l in lines[run + 1:]:
    if l.strip() and (len(l) - len(l.lstrip())) < indent:
        break
    out.append(l[indent:] if l.strip() else "")
print("\n".join(out).rstrip() + "\n")
PY
}

step "Workflow syntax (actionlint)"
if command -v actionlint >/dev/null 2>&1; then
  actionlint -color "$workflow" || fail "actionlint found problems"
elif engine_works; then
  "$engine" run --rm -v "$root":/repo:ro -w /repo --security-opt label=disable \
    docker.io/rhysd/actionlint:latest -color "$workflow" || fail "actionlint found problems"
else
  fail "actionlint is not installed and $(no_engine_msg). Install actionlint (pacman -S actionlint, brew install actionlint, or go install github.com/rhysd/actionlint/cmd/actionlint@latest) or start podman or docker."
fi

mkdir -p "$work/steps" "$work/keys"
extract "Prepare signed updater build" > "$work/steps/prepare.sh"
extract "Install Tauri system dependencies (Linux)" > "$work/steps/deps.sh"
extract "Build signed Linux self-update archive" > "$work/steps/linux-update.sh"

# The prepare step on its own, without a container: bad keys must stop it at once, a
# Windows workspace path must come out with forward slashes only.
step "Key checks and Windows path (no build)"
prepare() { # <public key> <private key> <workspace> -> runs the step like GitHub: bash -e -o pipefail
  ( cd "$work" && rm -f .updater-release.json && : > out.txt &&
    GALL_UPDATER_PUBLIC_KEY="$1" TAURI_SIGNING_PRIVATE_KEY="$2" WORKSPACE="$3" \
      GITHUB_OUTPUT="$work/out.txt" bash -e -o pipefail "$work/steps/prepare.sh" ) >/dev/null 2>&1
}
# A minisign-shaped pair that is good enough for the format checks.
pub=$(printf 'untrusted comment: minisign public key: 0000000000000000\nRWQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n' | base64 -w0)
sec=$(printf 'untrusted comment: rsign encrypted secret key\nRWRTY0Iy\n' | base64 -w0)
prepare "$pub" "$sec" 'D:\a\gall\gall' || fail "the prepare step rejects well-formed keys"
grep -qx 'config=D:/a/gall/gall/.updater-release.json' "$work/out.txt" ||
  fail "Windows config path is not D:/a/gall/gall/... : $(cat "$work/out.txt")"
node -e 'const j=JSON.parse(require("fs").readFileSync(process.argv[1]));
  if(!j.bundle.createUpdaterArtifacts||!j.plugins.updater.pubkey) process.exit(1)' "$work/.updater-release.json" ||
  fail "overlay lacks createUpdaterArtifacts or plugins.updater.pubkey"
! prepare "${pub}%" "$sec" /w || fail "a public key with a trailing % was accepted"
! prepare "$pub" "${sec}%" /w || fail "a private key with a trailing % was accepted"
! prepare "$sec" "$pub" /w || fail "swapped keys were accepted"
! prepare "" "$sec" /w || fail "an empty public key was accepted"
echo "ok"

step "Git speed (time and process budget of every Git view)"
# Runs on this machine, so it covers Linux; CI runs the same test on macOS, on Windows
# and there once more for a repository inside WSL (job "Git speed").
( cd "$root" && cargo test -q -p gall-core --release --test git_speed -- --ignored --nocapture ) ||
  fail "a Git view is over its time or process budget (see the table above)"
echo "ok"

step "App speed (listing, search, Quick Open, diff)"
( cd "$root" && cargo test -q -p gall-core --release --test app_speed -- --ignored --nocapture ) ||
  fail "an operation is over its time limit (see the table above)"
echo "ok"

if (( quick )); then echo; echo "Quick checks passed (no build was run)."; exit 0; fi

step "Linux build in debian:trixie ($engine)"
src="$work/src"; mkdir -p "$src"
tar --exclude=./target --exclude=./node_modules --exclude=./ui/node_modules --exclude=./ui/dist \
    --exclude=./.git --exclude=./.worktrees --exclude=./.claude -cf - . | tar -xf - -C "$src"

cat > "$work/inside.sh" <<'INSIDE'
set -euo pipefail
# Ensure host cleanup of /work succeeds when running under a root Docker daemon.
trap 'rm -rf /work/src/node_modules /work/src/ui/node_modules /work/src/ui/dist /work/src/dist /work/keys /work/.updater-release.json /work/out.txt 2>/dev/null || true' EXIT
export DEBIAN_FRONTEND=noninteractive
export PATH=/root/.cargo/bin:$PATH
bash -e -o pipefail /work/steps/deps.sh >/dev/null
curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
apt-get install -y -qq nodejs >/dev/null
rustup toolchain install stable --profile minimal >/dev/null
rustup default stable >/dev/null
cd /work/src
./scripts/verify-appimage-webkitgtk.sh --check-host
npm ci --no-audit --no-fund >/dev/null && npm ci --prefix ui --no-audit --no-fund >/dev/null
# Throwaway signing key, no password: the real one never enters the container.
npm run --silent tauri -- signer generate -w /work/keys/dry.key -p "" -f --ci >/dev/null
export GALL_UPDATER_PUBLIC_KEY="$(cat /work/keys/dry.key.pub)"
export TAURI_SIGNING_PRIVATE_KEY="$(cat /work/keys/dry.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
export WORKSPACE=/work/src GITHUB_OUTPUT=/work/out.txt
export APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=true
: > "$GITHUB_OUTPUT"
bash -e -o pipefail /work/steps/prepare.sh
config=$(sed -n 's/^config=//p' "$GITHUB_OUTPUT")
echo "building with --config $config"
# The target volume is kept between runs: packages of an earlier version would fail the count below.
rm -rf target/release/bundle
# Same command tauri-action runs.
npm run tauri build -- --config "$config"
bash -e -o pipefail /work/steps/linux-update.sh

b=target/release/bundle
shopt -s nullglob
deb=($b/deb/*.deb); rpm=($b/rpm/*.rpm); img=($b/appimage/*.AppImage); upd=(dist/*_linux_x86_64_update.tar.gz)
[[ ${#deb[@]} -eq 1 && ${#rpm[@]} -eq 1 && ${#img[@]} -eq 1 && ${#upd[@]} -eq 1 ]] || { echo "missing deb, rpm, AppImage, or update archive"; exit 1; }
./scripts/verify-appimage-webkitgtk.sh --check-appimage "${img[0]}"
for f in "${img[0]}" "${upd[0]}"; do
  [[ -s "$f.sig" ]] || { echo "missing signature for $f"; exit 1; }
done
entries=$(tar -tzf "${upd[0]}")
grep -qx 'gall' <<< "$entries" || { echo "update archive missing gall"; exit 1; }
grep -qx 'gall.sig' <<< "$entries" || { echo "update archive missing gall.sig"; exit 1; }
# The desktop file must make Gall selectable as the default file manager.
dpkg-deb -x "${deb[0]}" /tmp/debroot
desk=$(find /tmp/debroot -name '*.desktop' | head -1)
grep -qx 'MimeType=inode/directory;' "$desk" || { echo "desktop file lacks inode/directory"; exit 1; }
grep -q '^Categories=.*FileManager' "$desk" || { echo "desktop file lacks FileManager"; exit 1; }
echo "PACKAGES OK: $(ls -1 $b/deb/*.deb $b/rpm/*.rpm $b/appimage/*.AppImage $b/appimage/*.sig | xargs -n1 basename | tr '\n' ' ')"
INSIDE

"$engine" run --rm --security-opt label=disable \
  -v "$work":/work \
  -v gall-dryrun-cargo:/root/.cargo -v gall-dryrun-rustup:/root/.rustup \
  -v gall-dryrun-npm:/root/.npm -v gall-dryrun-target:/work/src/target \
  docker.io/library/debian:trixie bash /work/inside.sh || fail "the Linux build failed (see above)"

echo; echo "Dry run passed. Windows and macOS still need the GitHub dry run (Actions > Release)."
