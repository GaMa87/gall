#!/usr/bin/env bash
# Verify that the Linux build host and bundled AppImage carry a security-patched
# WebKitGTK and JavaScriptCore (WSA-2026-0007 / CVE-2026-112623, >= 2.54.3),
# and record the bundled engine identity for release provenance.
set -euo pipefail

MIN_VERSION="2.54.3"
MODE=""
APPIMAGE=""
OUTPUT=""

fail() {
  echo "WebKitGTK security check failed: $*" >&2
  exit 1
}

usage() {
  cat >&2 <<'USAGE'
Usage:
  verify-appimage-webkitgtk.sh --check-host [--min-version VER] [--output FILE]
  verify-appimage-webkitgtk.sh --check-appimage PATH [--min-version VER] [--output FILE]
USAGE
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check-host)
      MODE="host"
      shift
      ;;
    --check-appimage)
      [[ $# -ge 2 ]] || usage
      MODE="appimage"
      APPIMAGE="$2"
      shift 2
      ;;
    --min-version)
      [[ $# -ge 2 ]] || usage
      MIN_VERSION="$2"
      shift 2
      ;;
    --output)
      [[ $# -ge 2 ]] || usage
      OUTPUT="$2"
      shift 2
      ;;
    -h|--help)
      usage
      ;;
    *)
      usage
      ;;
  esac
done

[[ -n "$MODE" ]] || usage
[[ -n "$MIN_VERSION" ]] || fail "--min-version must not be empty"

for tool in dpkg dpkg-query readelf sha256sum python3; do
  command -v "$tool" >/dev/null 2>&1 || fail "required tool missing: $tool"
done

strip_upstream_version() {
  local full="$1"
  # Debian Policy 5.6.12: strip optional epoch (N:) and revision after the last hyphen.
  local no_epoch="${full#*:}"
  if [[ "$no_epoch" == *-* ]]; then
    printf '%s\n' "${no_epoch%-*}"
  else
    printf '%s\n' "$no_epoch"
  fi
}

query_pkg() {
  local pkg_spec="$1"
  local raw
  raw=$(dpkg-query -W -f='${Status}\t${Package}\t${Architecture}\t${Version}\t${Source}\n' "$pkg_spec" 2>/dev/null) ||
    fail "package is not installed: $pkg_spec"
  local line_count
  line_count=$(printf '%s\n' "$raw" | wc -l)
  [[ "$line_count" -eq 1 ]] || fail "ambiguous dpkg-query output for $pkg_spec"
  local status name arch ver src
  IFS=$'\t' read -r status name arch ver src <<< "$raw"
  [[ "$status" == "install ok installed" ]] || fail "package $pkg_spec is not cleanly installed (status: $status)"
  [[ -n "$ver" ]] || fail "empty version for package $pkg_spec"
  [[ -n "$src" ]] || src="$name ($ver)"
  printf '%s\t%s\t%s\t%s\n' "$name" "$arch" "$ver" "$src"
}

find_host_file() {
  local pkg_spec="$1"
  local pattern="$2"
  local files
  files=$(dpkg -L "$pkg_spec") || fail "could not list files for $pkg_spec"
  local match=""
  while IFS= read -r path; do
    if [[ "$(basename "$path")" == $pattern && -f "$path" && ! -L "$path" ]]; then
      match="$path"
      break
    fi
  done <<< "$files"
  if [[ -z "$match" ]]; then
    while IFS= read -r path; do
      if [[ "$(basename "$path")" == $pattern && -e "$path" ]]; then
        match=$(readlink -f "$path")
        break
      fi
    done <<< "$files"
  fi
  [[ -n "$match" && -f "$match" ]] || fail "could not locate '$pattern' in package $pkg_spec"
  printf '%s\n' "$match"
}

elf_build_id() {
  local file="$1"
  local id
  # Drain stdin so readelf never hits SIGPIPE under pipefail.
  id=$(readelf -n "$file" 2>/dev/null | awk '/Build ID:/ && !seen { print $3; seen=1 }') || true
  [[ -n "$id" ]] || fail "could not read ELF NT_GNU_BUILD_ID from $file"
  printf '%s\n' "$id"
}

max_glibc_version() {
  python3 - "$@" <<'PY'
import os, re, subprocess, sys

versions = set()
pat = re.compile(r'GLIBC_(\d+(?:\.\d+)+)')
targets = []

for arg in sys.argv[1:]:
    if os.path.isdir(arg):
        for root, _, files in os.walk(arg):
            for name in files:
                full = os.path.join(root, name)
                if os.path.islink(full) or not os.path.isfile(full):
                    continue
                try:
                    with open(full, "rb") as f:
                        if f.read(4) == b"\x7fELF":
                            targets.append(full)
                except OSError:
                    continue
    else:
        targets.append(arg)

for path in targets:
    proc = subprocess.run(
        ["readelf", "--version-info", "-W", path],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        check=False,
    )
    for m in pat.finditer(proc.stdout):
        versions.add(tuple(int(p) for p in m.group(1).split(".")))

if not versions:
    print("unknown")
else:
    top = max(versions)
    print("GLIBC_" + ".".join(str(p) for p in top))
PY
}

find_squashfs_offset() {
  local appimage="$1"
  python3 - "$appimage" <<'PY'
import struct, sys

path = sys.argv[1]
with open(path, "rb") as f:
    data = f.read()

if len(data) < 64 or data[:4] != b"\x7fELF":
    sys.exit(1)

ei_class = data[4]
ei_data = data[5]
endian = "<" if ei_data == 1 else ">"
if ei_class == 2:
    e_phoff, e_shoff = struct.unpack(endian + "QQ", data[32:48])
    e_phentsize, e_phnum, e_shentsize, e_shnum = struct.unpack(endian + "HHHH", data[54:62])
else:
    e_phoff, e_shoff = struct.unpack(endian + "II", data[28:36])
    e_phentsize, e_phnum, e_shentsize, e_shnum = struct.unpack(endian + "HHHH", data[42:50])

end = 0
if e_shoff and e_shnum:
    end = max(end, e_shoff + e_shentsize * e_shnum)
if e_phoff and e_phnum:
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        if off + e_phentsize > len(data):
            break
        if ei_class == 2:
            p_offset, _, _, p_filesz = struct.unpack(endian + "QQQQ", data[off + 8 : off + 40])
        else:
            p_offset, _, _, p_filesz = struct.unpack(endian + "IIII", data[off + 4 : off + 20])
        end = max(end, p_offset + p_filesz)

def is_squashfs_v4(buf: bytes, pos: int) -> bool:
    if pos + 96 > len(buf) or buf[pos : pos + 4] != b"hsqs":
        return False
    block_size, _, _, block_log, _, _, s_major, s_minor = struct.unpack(
        "<IIHHHHHH", buf[pos + 12 : pos + 32]
    )
    return (
        s_major == 4
        and s_minor == 0
        and 4096 <= block_size <= 1048576
        and (block_size & (block_size - 1)) == 0
        and (1 << block_log) == block_size
    )

if is_squashfs_v4(data, end):
    print(end)
    sys.exit(0)

search = end
while True:
    pos = data.find(b"hsqs", search)
    if pos < 0:
        break
    if is_squashfs_v4(data, pos):
        print(pos)
        sys.exit(0)
    search = pos + 4

sys.exit(1)
PY
}

extract_appimage() {
  local appimage="$1"
  local dest="$2"
  if [[ -d "$appimage" ]]; then
    printf '%s\n' "$appimage"
    return
  fi
  [[ -f "$appimage" ]] || fail "AppImage not found: $appimage"

  local out_dir="$dest/squashfs-root"
  local offset=""
  offset=$(find_squashfs_offset "$appimage") || offset=""

  if [[ -n "$offset" ]] && command -v unsquashfs >/dev/null 2>&1; then
    if unsquashfs -no-progress -o "$offset" -d "$out_dir" "$appimage" >/dev/null; then
      printf '%s\n' "$out_dir"
      return
    fi
    rm -rf "$out_dir"
  fi

  local abs_img
  abs_img=$(readlink -f "$appimage")
  [[ -x "$abs_img" ]] || fail "could not extract $appimage via unsquashfs and file is not executable for --appimage-extract"
  (
    cd "$dest"
    "$abs_img" --appimage-extract >/dev/null
  ) || fail "failed to extract AppImage $appimage"
  [[ -d "$out_dir" ]] || fail "extracted AppImage did not create squashfs-root"
  printf '%s\n' "$out_dir"
}

# Inspect EVERY file/symlink matching `pattern` under `root`:
# - Fail if none exist.
# - Fail if `required_name` (e.g. the runtime SONAME `libwebkit2gtk-4.1.so.0`) is missing.
# - Fail if ANY matching symlink is dangling or escapes `root`.
# - Fail if ANY resolved regular file has a Build ID different from `expected_id`.
# Prints `<first_relative_path>\t<count>\t<first_sha256>`.
verify_bundled_matches() {
  local root="$1"
  local pattern="$2"
  local required_name="$3"
  local expected_id="$4"
  local label="$5"
  local real_root
  real_root=$(readlink -f "$root")

  local found_any=0
  local found_required=0
  local first_rel=""
  local first_sha=""
  local -A checked_files=()

  while IFS= read -r -d '' candidate; do
    found_any=1
    local base
    base=$(basename "$candidate")
    if [[ -n "$required_name" && "$base" == "$required_name" ]]; then
      found_required=1
    fi
    if [[ -L "$candidate" ]]; then
      local target
      target=$(readlink "$candidate")
      if [[ "$target" == /* ]]; then
        fail "bundled $label symlink $candidate points to absolute host path $target"
      fi
    fi
    local resolved
    resolved=$(readlink -f "$candidate" 2>/dev/null || true)
    if [[ -z "$resolved" || ! -f "$resolved" ]]; then
      fail "bundled $label entry $candidate is broken or not a regular file"
    fi
    if [[ "$resolved" != "$real_root"/* ]]; then
      fail "bundled $label entry $candidate resolves outside AppImage ($resolved)"
    fi
    if [[ -z "${checked_files[$resolved]+x}" ]]; then
      local got_id
      got_id=$(elf_build_id "$resolved")
      if [[ "$got_id" != "$expected_id" ]]; then
        fail "bundled $label ($resolved) Build ID ($got_id) does not match verified vendor binary ($expected_id)"
      fi
      checked_files["$resolved"]=1
      if [[ -z "$first_rel" ]]; then
        first_rel="${resolved#"$real_root"/}"
        first_sha=$(sha256sum "$resolved" | awk '{ print $1 }')
      fi
    fi
  done < <(find "$real_root" -name "$pattern" \( -type f -o -type l \) -print0 | sort -z)

  (( found_any == 1 )) || fail "bundled AppImage does not contain $pattern"
  if [[ -n "$required_name" && "$found_required" -ne 1 ]]; then
    fail "bundled AppImage is missing required runtime entry $required_name"
  fi
  printf '%s\t%s\t%s\n' "$first_rel" "${#checked_files[@]}" "$first_sha"
}

# 1. Verify host WebKitGTK and JavaScriptCore packages for the native architecture.
host_arch=$(dpkg --print-architecture) || fail "could not determine dpkg native architecture"
wk_info=$(query_pkg "libwebkit2gtk-4.1-0:${host_arch}")
jsc_info=$(query_pkg "libjavascriptcoregtk-4.1-0:${host_arch}")
IFS=$'\t' read -r wk_pkg wk_arch wk_ver wk_src <<< "$wk_info"
IFS=$'\t' read -r jsc_pkg jsc_arch jsc_ver jsc_src <<< "$jsc_info"

[[ "$wk_ver" == "$jsc_ver" ]] ||
  fail "version mismatch between $wk_pkg ($wk_ver) and $jsc_pkg ($jsc_ver)"
[[ "$wk_arch" == "$jsc_arch" ]] ||
  fail "architecture mismatch between $wk_pkg ($wk_arch) and $jsc_pkg ($jsc_arch)"

wk_upstream=$(strip_upstream_version "$wk_ver")
[[ -n "$wk_upstream" ]] || fail "could not parse upstream version from $wk_ver"
if ! dpkg --compare-versions "$wk_upstream" ge "$MIN_VERSION"; then
  fail "$wk_pkg $wk_ver (upstream $wk_upstream) is older than required minimum $MIN_VERSION (WSA-2026-0007 / CVE-2026-112623)"
fi

host_wk_so=$(find_host_file "libwebkit2gtk-4.1-0:${host_arch}" 'libwebkit2gtk-4.1.so.0*')
host_jsc_so=$(find_host_file "libjavascriptcoregtk-4.1-0:${host_arch}" 'libjavascriptcoregtk-4.1.so.0*')
host_webproc=$(find_host_file "libwebkit2gtk-4.1-0:${host_arch}" 'WebKitWebProcess')
host_netproc=$(find_host_file "libwebkit2gtk-4.1-0:${host_arch}" 'WebKitNetworkProcess')
host_bundle=$(find_host_file "libwebkit2gtk-4.1-0:${host_arch}" 'libwebkit2gtkinjectedbundle.so')

host_wk_build_id=$(elf_build_id "$host_wk_so")
host_jsc_build_id=$(elf_build_id "$host_jsc_so")
host_webproc_build_id=$(elf_build_id "$host_webproc")
host_netproc_build_id=$(elf_build_id "$host_netproc")
host_bundle_build_id=$(elf_build_id "$host_bundle")

host_wk_sha=$(sha256sum "$host_wk_so" | awk '{ print $1 }')
host_jsc_sha=$(sha256sum "$host_jsc_so" | awk '{ print $1 }')
host_glibc=$(max_glibc_version "$host_wk_so" "$host_jsc_so" "$host_webproc" "$host_netproc" "$host_bundle")

os_pretty="unknown"
if [[ -r /etc/os-release ]]; then
  os_pretty=$(awk -F= '/^PRETTY_NAME=/ { gsub(/^"|"$/, "", $2); print $2; exit }' /etc/os-release)
fi

report=$(cat <<EOF
WebKitGTK security verification: PASSED
Policy minimum upstream version: >= ${MIN_VERSION} (WSA-2026-0007 / CVE-2026-112623)
Build OS: ${os_pretty}
Architecture: ${wk_arch}
Vendor package (${wk_pkg}): ${wk_ver} (upstream ${wk_upstream}, source: ${wk_src})
Vendor package (${jsc_pkg}): ${jsc_ver} (upstream ${wk_upstream}, source: ${jsc_src})
Vendor libwebkit2gtk-4.1: ${host_wk_so}
  Build ID: ${host_wk_build_id}
  SHA-256:  ${host_wk_sha}
Vendor libjavascriptcoregtk-4.1: ${host_jsc_so}
  Build ID: ${host_jsc_build_id}
  SHA-256:  ${host_jsc_sha}
Vendor WebKitWebProcess Build ID: ${host_webproc_build_id}
Vendor WebKitNetworkProcess Build ID: ${host_netproc_build_id}
Vendor libwebkit2gtkinjectedbundle.so Build ID: ${host_bundle_build_id}
Vendor WebKitGTK/JSC max GLIBC symbol: ${host_glibc}
EOF
)

if [[ "$MODE" == "appimage" ]]; then
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' EXIT
  appdir=$(extract_appimage "$APPIMAGE" "$stage")

  wk_bundled_info=$(verify_bundled_matches "$appdir" 'libwebkit2gtk-4.1.so.0*' 'libwebkit2gtk-4.1.so.0' "$host_wk_build_id" "libwebkit2gtk-4.1")
  jsc_bundled_info=$(verify_bundled_matches "$appdir" 'libjavascriptcoregtk-4.1.so.0*' 'libjavascriptcoregtk-4.1.so.0' "$host_jsc_build_id" "libjavascriptcoregtk-4.1")
  webproc_bundled_info=$(verify_bundled_matches "$appdir" 'WebKitWebProcess' 'WebKitWebProcess' "$host_webproc_build_id" "WebKitWebProcess")
  netproc_bundled_info=$(verify_bundled_matches "$appdir" 'WebKitNetworkProcess' 'WebKitNetworkProcess' "$host_netproc_build_id" "WebKitNetworkProcess")
  bundle_bundled_info=$(verify_bundled_matches "$appdir" 'libwebkit2gtkinjectedbundle.so' 'libwebkit2gtkinjectedbundle.so' "$host_bundle_build_id" "libwebkit2gtkinjectedbundle.so")

  IFS=$'\t' read -r rel_wk wk_count bundled_wk_sha <<< "$wk_bundled_info"
  IFS=$'\t' read -r rel_jsc jsc_count bundled_jsc_sha <<< "$jsc_bundled_info"
  IFS=$'\t' read -r rel_webproc _ _ <<< "$webproc_bundled_info"
  IFS=$'\t' read -r rel_netproc _ _ <<< "$netproc_bundled_info"
  IFS=$'\t' read -r rel_bundle _ _ <<< "$bundle_bundled_info"

  appimage_sha="directory"
  if [[ -f "$APPIMAGE" ]]; then
    appimage_sha=$(sha256sum "$APPIMAGE" | awk '{ print $1 }')
  fi
  bundled_glibc=$(max_glibc_version "$appdir")

  report+=$'\n'
  report+=$(cat <<EOF
AppImage: $(basename "$APPIMAGE")
AppImage SHA-256: ${appimage_sha}
Bundled libwebkit2gtk-4.1 (${wk_count} file): ${rel_wk}
  Build ID: ${host_wk_build_id} (matches vendor)
  SHA-256:  ${bundled_wk_sha}
Bundled libjavascriptcoregtk-4.1 (${jsc_count} file): ${rel_jsc}
  Build ID: ${host_jsc_build_id} (matches vendor)
  SHA-256:  ${bundled_jsc_sha}
Bundled WebKitWebProcess: ${rel_webproc} (Build ID matches vendor)
Bundled WebKitNetworkProcess: ${rel_netproc} (Build ID matches vendor)
Bundled libwebkit2gtkinjectedbundle.so: ${rel_bundle} (Build ID matches vendor)
AppImage max GLIBC symbol (all bundled ELF files): ${bundled_glibc}
EOF
  )
fi

printf '%s\n' "$report"

if [[ -n "$OUTPUT" ]]; then
  mkdir -p "$(dirname "$OUTPUT")"
  printf '%s\n' "$report" > "$OUTPUT"
fi

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    printf '### Linux WebKitGTK security provenance (%s)\n\n```text\n%s\n```\n' "$wk_arch" "$report"
  } >> "$GITHUB_STEP_SUMMARY"
fi
