#!/usr/bin/env bash
# Real-application check of the FileChooser portal backend (xdg-desktop-portal-gall) and the Gall
# picker window: calls OpenFile / SaveFile over D-Bus, answers in the real picker window with a key
# press, and checks the reply. It uses a private D-Bus session and disposable settings, but the
# picker window appears on the active Wayland desktop (focus moves to it) and the key press is sent
# with wtype, so do not type while it runs.
#   ./scripts/portal-smoke.sh [dir-with-gall-and-xdg-desktop-portal-gall]   default: target/debug
# Needs: dbus-run-session, busctl, gdbus, wtype, hyprctl (Hyprland), a graphical session.
set -u
BINDIR="$(realpath "${1:-target/debug}")"
for tool in dbus-run-session busctl gdbus wtype hyprctl; do
  command -v "$tool" >/dev/null || { echo "missing: $tool"; exit 2; }
done
{ [ -x "$BINDIR/gall" ] && [ -x "$BINDIR/xdg-desktop-portal-gall" ]; } || { echo "build first: cargo build -p gall --features tauri/custom-protocol -p gall-portal"; exit 2; }

T="$(mktemp -d -t gall-portal-XXXX)"
mkdir -p "$T/only" "$T/save" "$T/c" "$T/d"
printf 'x\n' > "$T/only/a b.txt"
mkdir -p "$T/filt"
printf 'x\n' > "$T/filt/a.txt"
touch -d '2 hours ago' "$T/filt/a.txt"
printf 'x\n' > "$T/filt/b.png"
export XDG_CONFIG_HOME="$T/c" XDG_DATA_HOME="$T/d" GALL_MULTI_INSTANCE=1 GALL_PORTAL_GALL="$BINDIR/gall"
export T BINDIR
exec dbus-run-session -- bash -s <<'INNER'
set -u
fails=0
check() { if [ "$2" = ok ]; then echo "PASS  $1"; else echo "FAIL  $1  — $3"; fails=$((fails + 1)); fi; }
"$BINDIR/xdg-desktop-portal-gall" 2>"$T/portal.log" &
PORTAL=$!
for _ in $(seq 1 50); do
  gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
    --method org.freedesktop.DBus.NameHasOwner org.freedesktop.impl.portal.desktop.gall 2>/dev/null | grep -q true && break
  sleep 0.1
done
gall_window() { hyprctl clients -j 2>/dev/null | grep -q '"class": "gall"'; }
ay() { printf 'ay %s' "$(printf '%s' "$1" | wc -c)"; printf ' %s' $(printf '%s' "$1" | od -An -tu1); }
call() { # method request-id title [option triples for a{sv}: key signature value...]
  local method="$1" id="$2" title="$3"; shift 3
  busctl --user --timeout=120 call org.freedesktop.impl.portal.desktop.gall /org/freedesktop/portal/desktop \
    org.freedesktop.impl.portal.FileChooser "$method" 'osssa{sv}' "/org/freedesktop/portal/desktop/request/smoke/$id" smoke "" "$title" "$@"
}
answer() { # reply-file, keys for wtype...
  local out="$1"; shift
  for _ in $(seq 1 60); do gall_window && break; sleep 0.25; done
  sleep 4
  hyprctl dispatch focuswindow "class:^gall$" >/dev/null 2>&1
  sleep 1
  wtype "$@"
  for _ in $(seq 1 50); do [ -s "$out" ] && break; sleep 0.2; done
  for _ in $(seq 1 50); do gall_window || break; sleep 0.2; done
}

# 1. Open a file: Enter on the only file answers with it.
call OpenFile t1 "Pick a file" 1 current_folder $(ay "$T/only") > "$T/r1.txt" 2>&1 &
answer "$T/r1.txt" -k Return
r=$(cat "$T/r1.txt")
case "$r" in *"ua{sv} 0 "*"only/a%20b.txt"*) check "OpenFile: Enter on a file returns its URI" ok;; *) check "OpenFile: Enter on a file returns its URI" bad "$r";; esac

# 2. Cancel with Escape.
call OpenFile t2 "Pick a file" 1 current_folder $(ay "$T/only") > "$T/r2.txt" 2>&1 &
answer "$T/r2.txt" -k Escape
r=$(cat "$T/r2.txt")
case "$r" in *"ua{sv} 1 "*) check "OpenFile: Escape cancels (response 1)" ok;; *) check "OpenFile: Escape cancels (response 1)" bad "$r";; esac

# 3. Save as: the suggested name is taken with Enter.
call SaveFile t3 "Save as" 2 current_folder $(ay "$T/save") current_name s "new file.txt" > "$T/r3.txt" 2>&1 &
answer "$T/r3.txt" -k Return
r=$(cat "$T/r3.txt")
case "$r" in *"ua{sv} 0 "*"save/new%20file.txt"*) check "SaveFile: Enter takes the suggested name" ok;; *) check "SaveFile: Enter takes the suggested name" bad "$r";; esac

# 4. Choose a folder: Ctrl+Enter chooses the current folder.
call OpenFile t4 "Pick a folder" 2 directory b true current_folder $(ay "$T/save") > "$T/r4.txt" 2>&1 &
answer "$T/r4.txt" -M ctrl -k Return -m ctrl
r=$(cat "$T/r4.txt")
case "$r" in *"ua{sv} 0 "*"file://$T/save\""*) check "OpenFile (directory): Ctrl+Enter chooses the folder" ok;; *) check "OpenFile (directory): Ctrl+Enter chooses the folder" bad "$r";; esac

# 5. A file-type filter hides the other files: b.png is newer (listed first) but only a.txt is shown.
call OpenFile t5 "Pick a text file" 2 current_folder $(ay "$T/filt") filters a\(sa\(us\)\) 1 Text 1 0 '*.txt' > "$T/r5.txt" 2>&1 &
answer "$T/r5.txt" -k Return
r=$(cat "$T/r5.txt")
case "$r" in *"ua{sv} 0 "*"filt/a.txt"*) check "OpenFile: the filter hides non-matching files" ok;; *) check "OpenFile: the filter hides non-matching files" bad "$r";; esac

# 5b. Extra options (choices) round-trip: a combo (initial Latin-1) and a checkbox (initial true) come back
# with the values the picker held when Enter chose the file.
call OpenFile t7 "Pick with options" 2 current_folder $(ay "$T/only") choices 'a(ssa(ss)s)' 2 \
  enc Encoding 2 u8 UTF-8 l1 Latin-1 l1 \
  ow Overwrite 0 true > "$T/r7.txt" 2>&1 &
answer "$T/r7.txt" -k Return
r=$(cat "$T/r7.txt")
case "$r" in *'"enc" "l1"'*'"ow" "true"'*) check "OpenFile: choices come back with the picker's values" ok;; *) check "OpenFile: choices come back with the picker's values" bad "$r";; esac

# 6. The caller gives up (Request.Close): the picker window closes and the reply is "cancelled".
call OpenFile t6 "Pick a file" 1 current_folder $(ay "$T/only") > "$T/r6.txt" 2>&1 &
for _ in $(seq 1 60); do gall_window && break; sleep 0.25; done
sleep 3
busctl --user call org.freedesktop.impl.portal.desktop.gall /org/freedesktop/portal/desktop/request/smoke/t6 org.freedesktop.impl.portal.Request Close 2>"$T/close.err"
for _ in $(seq 1 50); do [ -s "$T/r6.txt" ] && break; sleep 0.2; done
for _ in $(seq 1 50); do gall_window || break; sleep 0.2; done
r=$(cat "$T/r6.txt")
if gall_window; then check "Request.Close closes the picker window" bad "window still open; Close: $(cat "$T/close.err"); reply: $r"; else check "Request.Close closes the picker window" ok; fi

kill "$PORTAL" 2>/dev/null
echo "$fails failure(s)"
[ "$fails" = 0 ]
INNER
