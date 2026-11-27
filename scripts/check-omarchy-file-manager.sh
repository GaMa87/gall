#!/usr/bin/env bash
# Inspect folder association and FileManager1 independently in a Linux desktop session.
set -euo pipefail

if (( $# > 1 )); then
  printf 'Usage: %s [expected-desktop-id]\n' "$0" >&2
  exit 2
fi

default=$(xdg-mime query default inode/directory)
printf 'Folder association: %s\n' "${default:-unset}"

name=org.freedesktop.FileManager1
printf 'FileManager1 owner (does not activate a service):\n'
if pid=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
    org.freedesktop.DBus GetConnectionUnixProcessID s "$name" 2>&1); then
  ps -p "${pid#u }" -o pid=,comm=
elif [[ "$pid" == *"no owner"* || "$pid" == *"no such name"* ]]; then
  printf '  No owner\n'
else
  printf '%s\n' "$pid" >&2
  exit 1
fi

printf 'FileManager1 activation files (including competing providers):\n'
IFS=: read -r -a data_dirs <<< "${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
  data_dirs=("$XDG_RUNTIME_DIR" "${data_dirs[@]}")
fi
data_dirs=("${XDG_DATA_HOME:-$HOME/.local/share}" "${data_dirs[@]}")
shopt -s nullglob
declare -A seen=()
for dir in "${data_dirs[@]}"; do
  for service in "$dir"/dbus-1/services/*.service; do
    if [[ ! -v 'seen[$service]' ]] && rg -q '^Name=org\.freedesktop\.FileManager1[[:space:]]*$' "$service"; then
      printf '  %s\n' "$service"
      seen[$service]=1
    fi
  done
done

if (( $# )) && [[ "$default" != "$1" ]]; then
  printf 'Expected folder association: %s\n' "$1" >&2
  exit 1
fi
