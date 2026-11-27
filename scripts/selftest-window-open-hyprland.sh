#!/usr/bin/env bash
# Capture a fixed crop while an isolated Gall window opens. The crop must be
# inside the eventual window and show a part of the fixture that becomes static.
set -euo pipefail

if [[ $# != 5 || -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]]; then
  echo "Usage: $0 GALL {window|picker|compare} {solid|glassy} 'X,Y WIDTHxHEIGHT' OUTPUT_DIR" >&2
  exit 2
fi
app=$(realpath "$1")
scenario=$2
appearance=$3
region=$4
out=$(realpath -m "$5")
if [[ ! -x "$app" || -e "$out" || ! "$region" =~ ^(-?[0-9]+),(-?[0-9]+)\ ([1-9][0-9]*)x([1-9][0-9]*)$ ]]; then
  echo "Supply an executable, a valid capture region, and an unused output directory." >&2
  exit 2
fi
crop_x=${BASH_REMATCH[1]}
crop_y=${BASH_REMATCH[2]}
crop_w=${BASH_REMATCH[3]}
crop_h=${BASH_REMATCH[4]}
for tool in grim hyprctl jq magick socat; do
  command -v "$tool" >/dev/null || { echo "Missing $tool." >&2; exit 2; }
done
case "$appearance" in
  solid) glassy=false ;;
  glassy) glassy=true ;;
  *) echo "Choose solid or glassy." >&2; exit 2 ;;
esac
case "$scenario" in
  window) args=(--new-window "$out/fixture") ;;
  picker) args=(--pick open --pick-result "$out/pick-result.json" "$out/fixture") ;;
  compare) args=(--compare "$out/fixture/left.txt" "$out/fixture/right.txt") ;;
  *) echo "Choose window, picker, or compare." >&2; exit 2 ;;
esac

socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
[[ -S "$socket" ]] || { echo "Cannot find the Hyprland event socket." >&2; exit 2; }
mkdir -p "$out"/{fixture,frames,config/gall,data,state/omarchy/toggles/hypr}
if [[ "$glassy" == true ]]; then
  touch "$out/state/omarchy/toggles/hypr/window-glassy.lua"
fi
start_maximized=${GALL_SELFTEST_START_MAXIMIZED:-true}
if [[ "$start_maximized" != true && "$start_maximized" != false ]]; then
  echo "Set GALL_SELFTEST_START_MAXIMIZED to true or false." >&2
  exit 2
fi
jq -n --argjson glassy "$glassy" --argjson maximize "$start_maximized" \
  '{theme:"omarchy", omarchyGlassy:$glassy, startMaximized:$maximize}' > "$out/config/gall/settings.json"
for i in $(seq -w 1 80); do touch "$out/fixture/file-$i.txt"; done
seq 100 > "$out/fixture/left.txt"
seq 2 101 > "$out/fixture/right.txt"

hyprctl -j monitors > "$out/monitors.json"
hyprctl -j animations > "$out/animations.json"
for option in decoration:active_opacity decoration:inactive_opacity decoration:blur:enabled; do
  hyprctl getoption "$option"
done > "$out/compositor-options"
socat -u "UNIX-CONNECT:$socket" - > "$out/events.log" 2>&1 &
events_pid=$!
app_pid=
cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  kill "$events_pid" 2>/dev/null || true
  wait "$events_pid" 2>/dev/null || true
}
trap cleanup EXIT
capture() {
  printf '%s %s\n' "$1" "$(date +%s%3N)" >> "$out/timestamps"
  grim -s 1 -t ppm -g "$region" "$out/frames/$1.ppm"
}
capture baseline
XDG_CONFIG_HOME="$out/config" XDG_DATA_HOME="$out/data" XDG_STATE_HOME="$out/state" \
  GALL_MULTI_INSTANCE=1 \
  "$app" "${args[@]}" > "$out/app.log" 2>&1 &
app_pid=$!
for frame in $(seq -w 1 50); do capture "$frame"; done

hyprctl -j clients | jq -e --argjson pid "$app_pid" --argjson x "$crop_x" --argjson y "$crop_y" \
  --argjson w "$crop_w" --argjson h "$crop_h" '
    .[] | select(.pid == $pid and .mapped)
    | select(.at[0] <= $x and .at[1] <= $y
        and .at[0] + .size[0] >= $x + $w and .at[1] + .size[1] >= $y + $h)
  ' > "$out/window.json" || { echo "The crop is outside the mapped Gall window. See $out." >&2; exit 2; }

# ImageMagick exits 1 for different images and greater than 1 for an error.
difference() {
  local result status=0
  result=$(magick compare -metric MAE "$out/frames/$1.ppm" "$out/frames/$2.ppm" null: 2>&1) || status=$?
  if ((status > 1)); then echo "$result" >&2; return 2; fi
  awk -F '[()]' '{print $2}' <<< "$result"
}
{
  printf 'frame\tbaseline_mae\tsettled_mae\n'
  for frame in $(seq -w 1 50); do
    before=$(difference "$frame" baseline)
    settled=$(difference "$frame" 50)
    printf '%s\t%s\t%s\n' "$frame" "$before" "$settled"
  done
} > "$out/frame-differences.tsv"

# Ignore small changes due to antialiasing and the fixture text. Require a
# visibly distinct baseline and a stable final crop before judging the reveal.
awk -F '\t' '
  NR == 1 { next }
  $1 >= 46 && ($2 < .02 || $3 > .005) { invalid = 1 }
  $2 >= .02 && $3 > .02 { printf "Intermediate frame %s (MAE %.3f)\n", $1, $3; flash = 1 }
  END {
    if (invalid) { print "Inconclusive: choose a stable crop distinct from the background."; exit 2 }
    if (flash) exit 1
  }
' "$out/frame-differences.tsv" || { status=$?; echo "Capture evidence: $out"; exit "$status"; }
echo "No intermediate frame detected in the sampled crop. Capture evidence: $out"
