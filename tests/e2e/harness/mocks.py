"""External OS, Omarchy, Hyprland, GVFS, and Wayland recording mock shims."""

from __future__ import annotations

import base64
import json
import os
import selectors
import shutil
import socket
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import TYPE_CHECKING, Any, Sequence

if TYPE_CHECKING:
    from .sandbox import SandboxEnv

_PY_LOG_PREAMBLE = r"""#!/usr/bin/env python3
import json, os, sys, time

def _log_call(cmd_name: str) -> None:
    log_path = os.environ.get("GALL_E2E_MOCK_LOG", "")
    if not log_path:
        return
    rec = {
        "cmd": cmd_name,
        "argv": sys.argv[1:],
        "cwd": os.getcwd(),
        "env": {
            k: os.environ.get(k)
            for k in (
                "PATH", "HOME", "XDG_RUNTIME_DIR", "XDG_STATE_HOME",
                "XDG_CONFIG_HOME", "XDG_DATA_HOME", "TERMINAL",
                "EDITOR", "VISUAL", "HYPRLAND_INSTANCE_SIGNATURE",
                "DBUS_SESSION_BUS_ADDRESS", "GALL_E2E_RUN_ID",
            )
            if k in os.environ
        },
        "ts": time.time(),
    }
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    data = (json.dumps(rec, ensure_ascii=False) + "\n").encode("utf-8")
    fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    try:
        os.write(fd, data)
    finally:
        os.close(fd)
"""


def _write_executable(path: Path, content: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    os.chmod(path, 0o755)
    return path


class ClipboardOwnerUnavailable(RuntimeError):
    """Raised when GTK3/Wayland clipboard ownership cannot be initialized."""


_GTK_CLIPBOARD_OWNER_SCRIPT = r"""import base64
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

spec = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
ready_file = Path(sys.argv[2])
payloads = [(mime, base64.b64decode(b64)) for mime, b64 in spec.items()]


def _try_wl_copy_single():
    if len(payloads) != 1 or not os.environ.get("WAYLAND_DISPLAY"):
        return False
    wl_copy = shutil.which("wl-copy", path="/usr/local/bin:/usr/bin:/bin")
    if not wl_copy:
        return False
    mime, raw = payloads[0]
    proc = subprocess.Popen(
        [wl_copy, "--foreground", "--type", mime],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def _feed():
        try:
            assert proc.stdin is not None
            proc.stdin.write(raw)
            proc.stdin.close()
        except OSError:
            pass

    threading.Thread(target=_feed, daemon=True).start()

    def _cleanup(_signum, _frame):
        if proc.poll() is None:
            proc.terminate()
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, _cleanup)
    signal.signal(signal.SIGINT, _cleanup)

    time.sleep(0.25)
    if proc.poll() is not None:
        return False
    ready_file.write_text("ready\n", encoding="utf-8")
    rc = proc.wait()
    raise SystemExit(rc)


if os.environ.get("WAYLAND_DISPLAY") and len(payloads) == 1:
    _try_wl_copy_single()

try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, Gtk
except Exception as exc:
    if not _try_wl_copy_single():
        sys.stderr.write(f"gi/Gtk3 unavailable: {exc}\n")
        raise SystemExit(2)

ok, _ = Gtk.init_check([])
if not ok:
    if not _try_wl_copy_single():
        sys.stderr.write("Gtk.init_check([]) failed (no reachable display)\n")
        raise SystemExit(2)

owner = Gtk.Invisible()
owner.realize()
targets = [Gtk.TargetEntry.new(mime, 0, idx) for idx, (mime, _) in enumerate(payloads)]
Gtk.selection_add_targets(owner, Gdk.SELECTION_CLIPBOARD, targets)


def _get(_widget, selection_data, info, _time_val):
    mime, raw = payloads[info]
    selection_data.set(Gdk.Atom.intern(mime, False), 8, raw)


owner.connect("selection-get", _get)

if not Gtk.selection_owner_set(owner, Gdk.SELECTION_CLIPBOARD, Gdk.CURRENT_TIME):
    if not _try_wl_copy_single():
        sys.stderr.write("Gtk.selection_owner_set(CLIPBOARD) returned False\n")
        raise SystemExit(3)

ready_file.write_text("ready\n", encoding="utf-8")
Gtk.main()
"""


def start_gtk_clipboard_owner(
    sb: "SandboxEnv",
    mime_payloads: dict[str, bytes],
    timeout: float = 5.0,
) -> subprocess.Popen[Any]:
    """Spawn a sandbox-scoped GTK3/Wayland clipboard owner serving exact MIME payloads."""
    if not mime_payloads:
        raise ValueError("mime_payloads must not be empty")

    uid = uuid.uuid4().hex
    spec_path = sb.tmp / f"clip-owner-{uid}.json"
    ready_path = sb.tmp / f"clip-owner-{uid}.ready"
    helper = sb.tmp / f"clip-owner-{uid}.py"

    encoded = {m: base64.b64encode(data).decode("ascii") for m, data in mime_payloads.items()}
    spec_path.write_text(json.dumps(encoded), encoding="utf-8")
    helper.write_text(_GTK_CLIPBOARD_OWNER_SCRIPT, encoding="utf-8")

    proc = sb.popen([sys.executable, str(helper), str(spec_path), str(ready_path)])
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if ready_path.is_file() or proc.poll() is not None:
            break
        time.sleep(0.05)

    if proc.poll() is not None:
        stderr_txt = ""
        if proc.stderr is not None:
            try:
                stderr_txt = proc.stderr.read().decode("utf-8", errors="replace").strip()
            except Exception:
                pass
        detail = f": {stderr_txt}" if stderr_txt else ""
        if proc.returncode in (2, 3):
            raise ClipboardOwnerUnavailable(
                f"GTK/Wayland clipboard owner exited early (rc={proc.returncode}){detail}"
            )
        raise RuntimeError(
            f"GTK/Wayland clipboard owner crashed unexpectedly (rc={proc.returncode}){detail}"
        )

    if not ready_path.is_file():
        try:
            proc.terminate()
            proc.wait(timeout=1.5)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
        raise RuntimeError(
            f"GTK/Wayland clipboard owner did not become ready within {timeout:.1f}s"
        )

    return proc


class HyprSocket2Server:
    """Real AF_UNIX SOCK_STREAM server simulating Hyprland .socket2.sock."""

    def __init__(self, socket_path: Path) -> None:
        self.socket_path = socket_path
        self.socket_path.parent.mkdir(parents=True, exist_ok=True)
        if self.socket_path.exists():
            self.socket_path.unlink()

        self._srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._srv.bind(str(self.socket_path))
        self._srv.listen(16)
        self._srv.setblocking(False)

        self._lock = threading.Lock()
        self._clients: list[socket.socket] = []
        self._stop = threading.Event()
        self._wake_r, self._wake_w = os.pipe()
        self._sel = selectors.DefaultSelector()
        self._sel.register(self._srv, selectors.EVENT_READ, data="accept")
        self._sel.register(self._wake_r, selectors.EVENT_READ, data="wake")

        self._thread = threading.Thread(target=self._loop, name="hypr-socket2", daemon=True)
        self._thread.start()

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                events = self._sel.select(timeout=0.5)
            except OSError:
                break
            for key, _ in events:
                if key.data == "wake":
                    try:
                        os.read(self._wake_r, 64)
                    except OSError:
                        pass
                elif key.data == "accept":
                    try:
                        conn, _ = self._srv.accept()
                        conn.setblocking(True)
                        with self._lock:
                            self._clients.append(conn)
                    except OSError:
                        pass

    def client_count(self) -> int:
        with self._lock:
            return len(self._clients)

    def wait_for_client(self, timeout: float = 5.0) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.client_count() > 0:
                return True
            time.sleep(0.02)
        return self.client_count() > 0

    def push_event(self, line: str) -> int:
        payload = (line.rstrip("\n") + "\n").encode("utf-8")
        sent = 0
        dead: list[socket.socket] = []
        with self._lock:
            for conn in self._clients:
                try:
                    conn.sendall(payload)
                    sent += 1
                except OSError:
                    dead.append(conn)
            for conn in dead:
                self._clients.remove(conn)
                try:
                    conn.close()
                except OSError:
                    pass
        return sent

    def close(self) -> None:
        if self._stop.is_set():
            return
        self._stop.set()
        try:
            os.write(self._wake_w, b"x")
        except OSError:
            pass
        self._thread.join(timeout=2.0)
        try:
            self._sel.close()
        except Exception:
            pass
        for fd in (self._wake_r, self._wake_w):
            try:
                os.close(fd)
            except OSError:
                pass
        with self._lock:
            for conn in self._clients:
                try:
                    conn.close()
                except OSError:
                    pass
            self._clients.clear()
        try:
            self._srv.close()
        except OSError:
            pass
        try:
            if self.socket_path.exists():
                self.socket_path.unlink()
        except OSError:
            pass


class MockRegistry:
    """Installs recording executable shims and state fixtures into a SandboxEnv."""

    def __init__(self, sb: "SandboxEnv") -> None:
        self.sb = sb

    def install_tool(
        self,
        name: str,
        stdout: str = "",
        stderr: str = "",
        exit_code: int = 0,
        delay: float = 0.0,
        script_body: str | None = None,
    ) -> Path:
        target = self.sb.mock_bin / name
        if script_body is not None:
            if script_body.lstrip().startswith("#!"):
                # Wrap custom-shebang executable so GALL_E2E_MOCK_LOG is always recorded first.
                impl = self.sb.tmp / f".mock-impl-{name}"
                impl.write_text(script_body, encoding="utf-8")
                os.chmod(impl, 0o755)
                wrapper = (
                    _PY_LOG_PREAMBLE
                    + f"""
_log_call({name!r})
delay = {float(delay)!r}
if delay > 0:
    time.sleep(delay)
os.execv({str(impl)!r}, [{str(impl)!r}, *sys.argv[1:]])
"""
                )
                return _write_executable(target, wrapper)

            impl_sh = self.sb.tmp / f".mock-sh-{name}.sh"
            impl_sh.write_text("#!/usr/bin/env bash\n" + script_body + "\n", encoding="utf-8")
            os.chmod(impl_sh, 0o755)
            wrapper = (
                _PY_LOG_PREAMBLE
                + f"""
_log_call({name!r})
delay = {float(delay)!r}
if delay > 0:
    time.sleep(delay)
os.execv("/usr/bin/env", ["env", "bash", {str(impl_sh)!r}, *sys.argv[1:]])
"""
            )
            return _write_executable(target, wrapper)

        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call({name!r})
env_prefix = "MOCK_" + {name!r}.upper().replace("-", "_")
delay = float(os.environ.get(env_prefix + "_DELAY", {float(delay)!r}))
if delay > 0:
    time.sleep(delay)
out = os.environ.get(env_prefix + "_STDOUT", {stdout!r})
err = os.environ.get(env_prefix + "_STDERR", {stderr!r})
rc = int(os.environ.get(env_prefix + "_EXIT", {int(exit_code)!r}))
if out:
    sys.stdout.write(out)
    sys.stdout.flush()
if err:
    sys.stderr.write(err)
    sys.stderr.flush()
raise SystemExit(rc)
"""
        )
        return _write_executable(target, code)

    def install_xdg_terminal_exec(self) -> Path:
        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("xdg-terminal-exec")
args = sys.argv[1:]
inner = []
i = 0
while i < len(args):
    a = args[i]
    if a == "--":
        inner = args[i + 1:]
        break
    if a.startswith("--dir=") or a.startswith("--app-id=") or a.startswith("--title="):
        i += 1
        continue
    if a in ("--dir", "--app-id", "--title"):
        i += 2
        continue
    inner = args[i:]
    break
if os.environ.get("GALL_E2E_CHAIN_MOCKS") == "1" and inner:
    cand = os.path.join({str(self.sb.mock_bin)!r}, os.path.basename(inner[0]))
    if os.path.isfile(cand) and os.access(cand, os.X_OK):
        os.execv(cand, [cand, *inner[1:]])
rc = int(os.environ.get("MOCK_XDG_TERMINAL_EXEC_EXIT", "0"))
raise SystemExit(rc)
"""
        )
        return _write_executable(self.sb.mock_bin / "xdg-terminal-exec", code)

    def install_uwsm_app(self) -> Path:
        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("uwsm-app")
args = sys.argv[1:]
inner = list(args)
while inner and inner[0].startswith("-"):
    tok = inner.pop(0)
    if tok == "--":
        break
if os.environ.get("GALL_E2E_CHAIN_MOCKS") == "1" and inner:
    cand = os.path.join({str(self.sb.mock_bin)!r}, os.path.basename(inner[0]))
    if os.path.isfile(cand) and os.access(cand, os.X_OK):
        os.execv(cand, [cand, *inner[1:]])
rc = int(os.environ.get("MOCK_UWSM_APP_EXIT", "0"))
raise SystemExit(rc)
"""
        )
        return _write_executable(self.sb.mock_bin / "uwsm-app", code)

    def install_omarchy_launch_editor(self) -> Path:
        return self.install_tool("omarchy-launch-editor")

    def install_omarchy_floating_presentation(self) -> Path:
        code = (
            _PY_LOG_PREAMBLE
            + """
_log_call("omarchy-launch-floating-terminal-with-presentation")
args = sys.argv[1:]
if os.environ.get("GALL_E2E_CHAIN_MOCKS") == "1" and args:
    import subprocess
    if len(args) == 1:
        res = subprocess.run(["bash", "-c", args[0]], check=False)
        raise SystemExit(res.returncode)
    try:
        res = subprocess.run(args, check=False)
    except FileNotFoundError as exc:
        sys.stderr.write(f"omarchy-launch-floating-terminal-with-presentation: {args[0]}: {exc}\\n")
        raise SystemExit(127)
    except OSError as exc:
        sys.stderr.write(f"omarchy-launch-floating-terminal-with-presentation: {args[0]}: {exc}\\n")
        raise SystemExit(126)
    raise SystemExit(res.returncode)
rc = int(os.environ.get("MOCK_OMARCHY_LAUNCH_FLOATING_TERMINAL_WITH_PRESENTATION_EXIT", "0"))
raise SystemExit(rc)
"""
        )
        return _write_executable(
            self.sb.mock_bin / "omarchy-launch-floating-terminal-with-presentation",
            code,
        )

    def install_omarchy_transcode(self) -> Path:
        return self.install_tool("omarchy-transcode")

    def install_omarchy_cmd_terminal_cwd(self, cwd_out: str | Path) -> Path:
        out = str(cwd_out).rstrip("\n") + "\n"
        return self.install_tool("omarchy-cmd-terminal-cwd", stdout=out)

    def install_localsend(self) -> Path:
        return self.install_tool("localsend")

    def install_zoxide(self, dirs_list: Sequence[str | Path]) -> Path:
        lines = "\n".join(str(d) for d in dirs_list)
        if lines:
            lines += "\n"
        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("zoxide")
rc = int(os.environ.get("MOCK_ZOXIDE_EXIT", "0"))
if rc != 0:
    raise SystemExit(rc)
args = sys.argv[1:]
if len(args) >= 2 and args[0] == "query" and ("-l" in args or "--list" in args):
    out = os.environ.get("MOCK_ZOXIDE_STDOUT", {lines!r})
    sys.stdout.write(out)
    sys.stdout.flush()
raise SystemExit(0)
"""
        )
        return _write_executable(self.sb.mock_bin / "zoxide", code)

    def install_gio(
        self,
        mounts_output: str = "",
        mount_exit: int = 0,
        list_delay: float = 0.0,
    ) -> Path:
        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("gio")
args = sys.argv[1:]
list_delay = float(os.environ.get("MOCK_GIO_LIST_DELAY", {float(list_delay)!r}))
mount_exit = int(os.environ.get("MOCK_GIO_MOUNT_EXIT", {int(mount_exit)!r}))
mounts_out = os.environ.get("MOCK_GIO_MOUNTS_OUTPUT", {mounts_output!r})

if args and args[0] == "mount":
    rest = args[1:]
    if "-l" in rest or "--list" in rest:
        if list_delay > 0:
            time.sleep(list_delay)
        if mounts_out:
            sys.stdout.write(mounts_out)
            if not mounts_out.endswith("\\n"):
                sys.stdout.write("\\n")
            sys.stdout.flush()
        raise SystemExit(0)
    if mount_exit != 0:
        sys.stderr.write("gio: mount operation failed (mock)\\n")
        sys.stderr.flush()
    raise SystemExit(mount_exit)

raise SystemExit(0)
"""
        )
        return _write_executable(self.sb.mock_bin / "gio", code)

    def install_wl_clipboard(self) -> tuple[Path, Path]:
        """Install recording wl-copy/wl-paste shims backed by real Wayland/GTK3 clipboard ownership."""
        helper_path = self.sb.tmp / "wl_clip_owner_helper.py"
        helper_path.write_text(_GTK_CLIPBOARD_OWNER_SCRIPT, encoding="utf-8")
        pid_file = self.sb.tmp / "wl_copy_owner.pid"
        tmp_dir = self.sb.tmp

        wl_copy_code = (
            _PY_LOG_PREAMBLE
            + f"""
import base64, shutil, signal, subprocess, uuid

_log_call("wl-copy")
helper_path = {str(helper_path)!r}
pid_file = {str(pid_file)!r}
tmp_dir = {str(tmp_dir)!r}

args = sys.argv[1:]
mime = None
clear = False
trim_nl = False
foreground = False
pos = []
i = 0
while i < len(args):
    a = args[i]
    if a == "--":
        pos.extend(args[i + 1:])
        break
    if a in ("-t", "--type") and i + 1 < len(args):
        mime = args[i + 1]
        i += 2
        continue
    if a.startswith("--type="):
        mime = a.split("=", 1)[1]
        i += 1
        continue
    if a in ("-c", "--clear"):
        clear = True
        i += 1
        continue
    if a in ("-n", "--trim-newline"):
        trim_nl = True
        i += 1
        continue
    if a in ("-f", "--foreground"):
        foreground = True
        i += 1
        continue
    if a in ("-o", "--paste-once", "-p", "--primary"):
        i += 1
        continue
    pos.append(a)
    i += 1

if os.path.isfile(pid_file):
    try:
        old_pid = int(open(pid_file, "r", encoding="utf-8").read().strip())
        os.killpg(old_pid, signal.SIGTERM)
    except Exception:
        pass
    try:
        os.unlink(pid_file)
    except OSError:
        pass

if clear:
    if os.environ.get("WAYLAND_DISPLAY"):
        sys_wl_copy = shutil.which("wl-copy", path="/usr/local/bin:/usr/bin:/bin")
        if sys_wl_copy:
            subprocess.run([sys_wl_copy, "--clear"], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    raise SystemExit(0)

if pos:
    payload = " ".join(pos).encode("utf-8")
else:
    payload = sys.stdin.buffer.read()
if trim_nl and payload.endswith(b"\\n"):
    payload = payload[:-1]

target_mime = mime or "text/plain;charset=utf-8"
uid = uuid.uuid4().hex
os.makedirs(tmp_dir, exist_ok=True)
spec_path = os.path.join(tmp_dir, f"wlcopy-{{uid}}.json")
ready_path = os.path.join(tmp_dir, f"wlcopy-{{uid}}.ready")
with open(spec_path, "w", encoding="utf-8") as f:
    json.dump({{target_mime: base64.b64encode(payload).decode("ascii")}}, f)

proc = subprocess.Popen(
    [sys.executable, helper_path, spec_path, ready_path],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.PIPE,
    start_new_session=True,
)

deadline = time.monotonic() + 5.0
while time.monotonic() < deadline:
    if os.path.isfile(ready_path) or proc.poll() is not None:
        break
    time.sleep(0.05)

if proc.poll() is not None or not os.path.isfile(ready_path):
    err = ""
    if proc.stderr is not None:
        try:
            err = proc.stderr.read().decode("utf-8", errors="replace").strip()
        except Exception:
            pass
    if proc.poll() is None:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except OSError:
            pass
    sys.stderr.write(f"wl-copy GTK/Wayland clipboard owner failed: {{err}}\\n")
    raise SystemExit(1)

with open(pid_file, "w", encoding="utf-8") as f:
    f.write(f"{{proc.pid}}\\n")

if foreground:
    raise SystemExit(proc.wait())
raise SystemExit(0)
"""
        )

        wl_paste_code = (
            _PY_LOG_PREAMBLE
            + """
import shutil, subprocess

_log_call("wl-paste")
args = sys.argv[1:]
list_types = False
mime = None
no_nl = False
primary = False
i = 0
while i < len(args):
    a = args[i]
    if a in ("-l", "--list-types"):
        list_types = True
        i += 1
        continue
    if a in ("-t", "--type") and i + 1 < len(args):
        mime = args[i + 1]
        i += 2
        continue
    if a.startswith("--type="):
        mime = a.split("=", 1)[1]
        i += 1
        continue
    if a in ("-n", "--no-newline"):
        no_nl = True
        i += 1
        continue
    if a in ("-p", "--primary"):
        primary = True
        i += 1
        continue
    i += 1

if os.environ.get("WAYLAND_DISPLAY"):
    sys_wl_paste = shutil.which("wl-paste", path="/usr/local/bin:/usr/bin:/bin")
    if sys_wl_paste:
        res = subprocess.run([sys_wl_paste, *args], capture_output=True, check=False)
        if res.returncode == 0:
            sys.stdout.buffer.write(res.stdout)
            sys.stdout.buffer.flush()
            raise SystemExit(0)

try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, Gtk
except Exception as exc:
    sys.stderr.write(f"gi/Gtk3 unavailable for wl-paste: {exc}\\n")
    raise SystemExit(1)

ok, _ = Gtk.init_check([])
if not ok:
    sys.stderr.write("No display reachable for wl-paste\\n")
    raise SystemExit(1)

cb = Gtk.Clipboard.get(Gdk.SELECTION_PRIMARY if primary else Gdk.SELECTION_CLIPBOARD)
if list_types:
    ok_t, atoms = cb.wait_for_targets()
    if not ok_t or not atoms:
        sys.stderr.write("Nothing is copied\\n")
        raise SystemExit(1)
    for atom in atoms:
        sys.stdout.write(atom.name() + "\\n")
    sys.stdout.flush()
    raise SystemExit(0)

candidates = [mime] if mime else ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING"]
if mime == "text/plain":
    candidates.append("text/plain;charset=utf-8")
elif mime == "text/plain;charset=utf-8":
    candidates.append("text/plain")

data = None
for cand in candidates:
    if not cand:
        continue
    sel = cb.wait_for_contents(Gdk.Atom.intern(cand, False))
    if sel is not None:
        raw = sel.get_data()
        if raw is not None:
            data = bytes(raw)
            break

if data is None:
    sys.stderr.write("Nothing is copied\\n")
    raise SystemExit(1)

if no_nl and data.endswith(b"\\n"):
    data = data[:-1]
sys.stdout.buffer.write(data)
sys.stdout.buffer.flush()
"""
        )

        p1 = _write_executable(self.sb.mock_bin / "wl-copy", wl_copy_code)
        p2 = _write_executable(self.sb.mock_bin / "wl-paste", wl_paste_code)
        return p1, p2

    def install_hyprctl(self, rounding_file: Path | None = None, rounding: int = 0) -> Path:
        rfile = rounding_file or (self.sb.tmp / "hypr_rounding.txt")
        if not rfile.exists():
            rfile.parent.mkdir(parents=True, exist_ok=True)
            rfile.write_text(f"{rounding}\n", encoding="utf-8")

        code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("hyprctl")
rfile = {str(rfile)!r}
args = sys.argv[1:]
json_mode = "-j" in args
clean = [a for a in args if a != "-j"]

def read_rounding() -> int:
    if "MOCK_HYPR_ROUNDING" in os.environ:
        return int(os.environ["MOCK_HYPR_ROUNDING"])
    try:
        with open(rfile, "r", encoding="utf-8") as f:
            return int(f.read().strip())
    except Exception:
        return 0

if os.environ.get("MOCK_HYPRCTL_ERROR"):
    sys.stderr.write("mock hyprctl error\\n")
    raise SystemExit(int(os.environ.get("MOCK_HYPRCTL_EXIT", "1")))

if len(clean) >= 2 and clean[0] == "getoption" and clean[1] == "decoration:rounding":
    val = read_rounding()
    if json_mode:
        print(json.dumps({{
            "option": "decoration:rounding",
            "default": val == 0,
            "type": 0,
            "int": val,
            "float": float(val),
            "str": str(val),
        }}))
    else:
        print(f"int: {{val}}")
    raise SystemExit(0)

if clean and clean[0] == "clients":
    print("[]" if json_mode else "")
    raise SystemExit(0)

if clean and clean[0] == "activewindow":
    print("{{}}" if json_mode else "")
    raise SystemExit(0)

print("ok")
raise SystemExit(0)
"""
        )
        return _write_executable(self.sb.mock_bin / "hyprctl", code)

    def start_hypr_socket2(self, sig: str = "e2e-sig") -> HyprSocket2Server:
        sock_path = self.sb.runtime / "hypr" / sig / ".socket2.sock"
        self.sb.env["HYPRLAND_INSTANCE_SIGNATURE"] = sig
        srv = HyprSocket2Server(sock_path)
        self.sb.add_cleanup(srv.close)
        return srv

    def install_pactree_expac(self, pkg_db: dict[str, dict[str, Any] | int]) -> tuple[Path, Path]:
        db_path = self.sb.tmp / "pkg_db.json"
        db_path.write_text(json.dumps(pkg_db), encoding="utf-8")

        pactree_code = (
            _PY_LOG_PREAMBLE
            + f"""
from collections import deque

_log_call("pactree")
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)

args = [a for a in sys.argv[1:] if not a.startswith("-")]
if not args:
    sys.stderr.write("error: no target specified\\n")
    raise SystemExit(1)

root_pkg = args[0]
if root_pkg not in db:
    sys.stderr.write(f"error: package '{{root_pkg}}' not found\\n")
    raise SystemExit(1)

seen = []
visited = set()
q = deque([root_pkg])
while q:
    pkg = q.popleft()
    if pkg in visited or pkg not in db:
        continue
    visited.add(pkg)
    seen.append(pkg)
    entry = db[pkg]
    deps = entry.get("deps", []) if isinstance(entry, dict) else []
    for d in deps:
        if d not in visited:
            q.append(d)

for pkg in seen:
    print(pkg)
"""
        )

        expac_code = (
            _PY_LOG_PREAMBLE
            + f"""
_log_call("expac")
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)

args = sys.argv[1:]
fmt = "%m"
fmt_seen = False
pkgs = []
i = 0
while i < len(args):
    a = args[i]
    if a in ("-S", "-Q"):
        i += 1
        continue
    if a.startswith("-") and i + 1 < len(args) and a in ("-s", "-d"):
        i += 2
        continue
    if a.startswith("-"):
        i += 1
        continue
    if "%" in a and not fmt_seen and not pkgs:
        fmt = a
        fmt_seen = True
    else:
        pkgs.append(a)
    i += 1

targets = pkgs if pkgs else sorted(db.keys())
for pkg in targets:
    entry = db.get(pkg)
    if entry is None:
        continue
    if isinstance(entry, int):
        size = entry
        prov = "None"
    else:
        size = int(entry.get("size", entry.get("install_size", entry.get("bytes", 0))))
        raw_prov = entry.get("provides", [])
        if isinstance(raw_prov, str):
            prov = raw_prov.strip() or "None"
        elif raw_prov:
            prov = " ".join(str(p) for p in raw_prov if str(p).strip()).strip() or "None"
        else:
            prov = "None"
    line = fmt.replace("%n", pkg).replace("%m", str(size)).replace("%P", prov)
    print(line)
"""
        )

        p1 = _write_executable(self.sb.mock_bin / "pactree", pactree_code)
        p2 = _write_executable(self.sb.mock_bin / "expac", expac_code)
        return p1, p2

    def seed_omarchy_theme(
        self,
        name: str = "catppuccin",
        colors_toml: str = "",
        gall_css: str | None = None,
        light_mode: bool = False,
        generation: int = 1,
    ) -> Path:
        current = self.sb.state / "omarchy" / "current"
        theme_dir = current / "theme"
        theme_dir.mkdir(parents=True, exist_ok=True)
        (theme_dir / "colors.toml").write_text(colors_toml, encoding="utf-8")
        if gall_css is not None:
            (theme_dir / "gall.css").write_text(gall_css, encoding="utf-8")
        if light_mode:
            (theme_dir / "light.mode").write_text("", encoding="utf-8")
        (current / "theme.name").write_text(f"{name}\n", encoding="utf-8")
        (current / "theme.generation").write_text(f"{generation}\n", encoding="utf-8")
        return theme_dir

    def atomic_swap_omarchy_theme(
        self,
        name: str,
        colors_toml: str,
        gall_css: str | None = None,
        light_mode: bool = False,
        generation: int | None = None,
    ) -> Path:
        current = self.sb.state / "omarchy" / "current"
        next_dir = current / "next-theme"
        theme_dir = current / "theme"
        if next_dir.exists():
            shutil.rmtree(next_dir)
        next_dir.mkdir(parents=True, exist_ok=True)
        (next_dir / "colors.toml").write_text(colors_toml, encoding="utf-8")
        if gall_css is not None:
            (next_dir / "gall.css").write_text(gall_css, encoding="utf-8")
        if light_mode:
            (next_dir / "light.mode").write_text("", encoding="utf-8")

        if theme_dir.exists():
            shutil.rmtree(theme_dir)
        os.replace(next_dir, theme_dir)

        (current / "theme.name").write_text(f"{name}\n", encoding="utf-8")
        if generation is None:
            gen_file = current / "theme.generation"
            try:
                generation = int(gen_file.read_text(encoding="utf-8").strip()) + 1
            except Exception:
                generation = 1
        (current / "theme.generation").write_text(f"{generation}\n", encoding="utf-8")
        return theme_dir

    def seed_gvfs_mount(
        self,
        dir_name: str,
        files: dict[str, bytes | str] | None = None,
    ) -> Path:
        mount_dir = self.sb.runtime / "gvfs" / dir_name
        mount_dir.mkdir(parents=True, exist_ok=True)
        if files:
            for rel, content in files.items():
                dest = mount_dir / rel
                dest.parent.mkdir(parents=True, exist_ok=True)
                if isinstance(content, bytes):
                    dest.write_bytes(content)
                else:
                    dest.write_text(content, encoding="utf-8")
        return mount_dir
