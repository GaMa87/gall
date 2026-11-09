#!/usr/bin/env bash
# Dry run of the release build, before a tag is pushed. Run it from a checkout of the
# commit you are about to tag (uncommitted changes are included).
#
#   scripts/release-dryrun.sh            # lint workflow + key checks + full Linux build
#   scripts/release-dryrun.sh --quick    # lint workflow + key checks only (seconds)
#
# It takes the real steps out of .github/workflows/release.yml (so the two cannot drift),
# runs them in an ubuntu:22.04 container like the CI runner, with a throwaway signing key,
# and checks the packages. It cannot build Windows or macOS: for those, run the Release
# workflow on GitHub with "dry_run" ticked (see the header of release.yml).
#
# Needs podman or docker and network access. First run ~15 minutes; later runs reuse
# volumes named gall-dryrun-* (remove them with `podman volume rm`).
set -euo pipefail

quick=0
[[ "${1:-}" == "--quick" ]] && quick=1

root=$(git rev-parse --show-toplevel)
cd "$root"
engine=${CONTAINER_ENGINE:-$(command -v podman || command -v docker || true)}
[[ -n "$engine" ]] || { echo "podman or docker is required" >&2; exit 1; }
workflow=.github/workflows/release.yml
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail() { echo "DRY RUN FAILED: $*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

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
"$engine" run --rm -v "$root":/repo:ro -w /repo --security-opt label=disable \
  docker.io/rhysd/actionlint:latest -color "$workflow" || fail "actionlint found problems"

mkdir -p "$work/steps" "$work/keys"
extract "Prepare signed updater build" > "$work/steps/prepare.sh"
extract "Install Tauri system dependencies (Linux)" > "$work/steps/deps.sh"

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

if (( quick )); then echo; echo "Quick checks passed (no build was run)."; exit 0; fi

step "Linux build in ubuntu:22.04 ($engine)"
src="$work/src"; mkdir -p "$src"
tar --exclude=./target --exclude=./node_modules --exclude=./ui/node_modules --exclude=./ui/dist \
    --exclude=./.git --exclude=./.worktrees --exclude=./.claude -cf - . | tar -xf - -C "$src"

cat > "$work/inside.sh" <<'INSIDE'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates git build-essential file xz-utils sudo rpm >/dev/null
curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
apt-get install -y -qq nodejs >/dev/null
bash -e /work/steps/deps.sh >/dev/null
if [[ ! -x /root/.cargo/bin/rustup ]]; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
fi
export PATH=/root/.cargo/bin:$PATH
rustup update stable --no-self-update >/dev/null 2>&1 || true
cd /work/src
npm ci --no-audit --no-fund >/dev/null && npm ci --prefix ui --no-audit --no-fund >/dev/null
# Throwaway signing key, no password: the real one never enters the container.
npm run --silent tauri -- signer generate -w /work/keys/dry.key -p "" -f --ci >/dev/null
export GALL_UPDATER_PUBLIC_KEY="$(cat /work/keys/dry.key.pub)"
export TAURI_SIGNING_PRIVATE_KEY="$(cat /work/keys/dry.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
export WORKSPACE=/work/src GITHUB_OUTPUT=/work/out.txt
: > "$GITHUB_OUTPUT"
bash -e -o pipefail /work/steps/prepare.sh
config=$(sed -n 's/^config=//p' "$GITHUB_OUTPUT")
echo "building with --config $config"
# Same command tauri-action runs.
npm run tauri build -- --config "$config"

b=target/release/bundle
shopt -s nullglob
deb=($b/deb/*.deb); rpm=($b/rpm/*.rpm); img=($b/appimage/*.AppImage)
[[ ${#deb[@]} -eq 1 && ${#rpm[@]} -eq 1 && ${#img[@]} -eq 1 ]] || { echo "missing deb, rpm or AppImage"; exit 1; }
for f in "${img[0]}" ; do
  [[ -s "$f.sig" ]] || { echo "missing signature for $f"; exit 1; }
done
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
  docker.io/library/ubuntu:22.04 bash /work/inside.sh || fail "the Linux build failed (see above)"

echo; echo "Dry run passed. Windows and macOS still need the GitHub dry run (Actions > Release)."
