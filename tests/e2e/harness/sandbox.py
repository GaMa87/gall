"""Per-test isolated sandbox environment for opaque-box E2E tests."""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Sequence

if TYPE_CHECKING:
    from .env_probe import EnvProbe


def _sanitize_case_id(case_id: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9_.-]+", "-", case_id).strip("-")
    return cleaned[:40] or "case"


def _terminate_proc(proc: subprocess.Popen[Any]) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except OSError:
        try:
            proc.terminate()
        except OSError:
            return
    try:
        proc.wait(timeout=2.0)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            try:
                proc.kill()
            except OSError:
                return
        try:
            proc.wait(timeout=1.0)
        except subprocess.TimeoutExpired:
            pass


def _find_free_x_display() -> int | None:
    for disp in range(99, 200):
        lock = Path(f"/tmp/.X{disp}-lock")
        sock = Path(f"/tmp/.X11-unix/X{disp}")
        if not lock.exists() and not sock.exists():
            return disp
    return None


def _has_active_display(sb: "SandboxEnv") -> bool:
    wd = sb.env.get("WAYLAND_DISPLAY", "").strip()
    if wd:
        cand = Path(wd) if Path(wd).is_absolute() else (sb.runtime / wd)
        if cand.exists():
            return True
    disp = sb.env.get("DISPLAY", "").strip()
    if disp:
        match = re.search(r":(\d+)(?:\.\d+)?$", disp)
        if match and Path(f"/tmp/.X11-unix/X{match.group(1)}").exists():
            return True
    return False


def ensure_headless_display(
    sb: "SandboxEnv",
    env_probe: "EnvProbe | None" = None,
    width: int = 1920,
    height: int = 1200,
) -> str | None:
    """Start a sandbox-scoped headless compositor when no live display socket is wired."""
    if _has_active_display(sb):
        return "live"

    ordered: list[str] = []
    if env_probe is not None and env_probe.headless_compositor:
        ordered.append(env_probe.headless_compositor)
    for cand in ("kwin_wayland", "labwc", "weston", "sway", "Xvfb"):
        if cand not in ordered:
            ordered.append(cand)

    available = [c for c in ordered if shutil.which(c)]
    for idx, comp in enumerate(available):
        clean_env: dict[str, str | None] = {
            "DISPLAY": None,
            "WAYLAND_DISPLAY": None,
            "HYPRLAND_INSTANCE_SIGNATURE": None,
        }
        sock_name = f"gall-e2e-wl-{os.getpid()}-{idx}"
        proc: subprocess.Popen[Any] | None = None

        if comp == "Xvfb":
            disp = _find_free_x_display()
            if disp is None:
                continue
            sock_path = Path(f"/tmp/.X11-unix/X{disp}")
            proc = sb.popen(
                ["Xvfb", f":{disp}", "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp"],
                env_overrides=clean_env,
            )
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                if sock_path.exists():
                    sb.env["DISPLAY"] = f":{disp}"
                    sb.env["GDK_BACKEND"] = "x11"
                    sb.add_cleanup(lambda p=proc: _terminate_proc(p))
                    return comp
                if proc.poll() is not None:
                    break
                time.sleep(0.05)
            _terminate_proc(proc)
            continue

        if comp == "weston":
            sock_path = sb.runtime / sock_name
            proc = sb.popen(
                [
                    "weston",
                    "--backend=headless-backend.so",
                    "--no-config",
                    "--idle-time=0",
                    f"--width={width}",
                    f"--height={height}",
                    f"--socket={sock_name}",
                ],
                env_overrides=clean_env,
            )
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                if sock_path.exists():
                    sb.env["WAYLAND_DISPLAY"] = sock_name
                    sb.add_cleanup(lambda p=proc: _terminate_proc(p))
                    return comp
                if proc.poll() is not None:
                    break
                time.sleep(0.05)
            _terminate_proc(proc)
            continue

        if comp == "kwin_wayland":
            sock_path = sb.runtime / sock_name
            proc = sb.popen(
                [
                    "kwin_wayland",
                    "--virtual",
                    "--no-lockscreen",
                    "--no-global-shortcuts",
                    "--socket",
                    sock_name,
                    "--width",
                    str(width),
                    "--height",
                    str(height),
                ],
                env_overrides=clean_env,
            )
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                if sock_path.exists():
                    sb.env["WAYLAND_DISPLAY"] = sock_name
                    sb.add_cleanup(lambda p=proc: _terminate_proc(p))
                    return comp
                if proc.poll() is not None:
                    break
                time.sleep(0.05)
            _terminate_proc(proc)
            continue

        if comp in ("labwc", "sway"):
            before = {p.name for p in sb.runtime.glob("wayland-*")}
            if comp == "labwc":
                cfg = sb.tmp / f"labwc-cfg-{idx}"
                cfg.mkdir(parents=True, exist_ok=True)
                (cfg / "rc.xml").write_text(
                    '<?xml version="1.0"?><labwc_config><core><decoration>client</decoration></core></labwc_config>\n',
                    encoding="utf-8",
                )
                comp_env = {
                    **clean_env,
                    "WLR_BACKENDS": "headless",
                    "WLR_LIBINPUT_NO_DEVICES": "1",
                    "WLR_HEADLESS_OUTPUTS": "1",
                }
                cmd = ["labwc", "-C", str(cfg)]
            else:
                cfg_file = sb.tmp / f"sway-{idx}.conf"
                cfg_file.write_text(
                    f"output HEADLESS-1 resolution {width}x{height}\ndefault_border none\n",
                    encoding="utf-8",
                )
                comp_env = {
                    **clean_env,
                    "WLR_BACKENDS": "headless",
                    "WLR_LIBINPUT_NO_DEVICES": "1",
                    "SWAYSOCK": "",
                }
                cmd = ["sway", "-c", str(cfg_file)]
            proc = sb.popen(cmd, env_overrides=comp_env)
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                after = {p.name for p in sb.runtime.glob("wayland-*") if not p.name.endswith(".lock")}
                created = sorted(after - before)
                if created:
                    sb.env["WAYLAND_DISPLAY"] = created[0]
                    sb.add_cleanup(lambda p=proc: _terminate_proc(p))
                    return comp
                if proc.poll() is not None:
                    break
                time.sleep(0.05)
            _terminate_proc(proc)
            continue

    return None


class SandboxEnv:
    """Isolated temporary XDG/HOME/runtime/D-Bus/mock-PATH sandbox."""

    def __init__(
        self,
        case_id: str,
        env_probe: "EnvProbe | None" = None,
        allow_gui: bool = False,
        multi_instance: bool = False,
        dbus_required: bool = False,
    ) -> None:
        self.case_id = case_id
        self.env_probe = env_probe
        self.allow_gui = allow_gui
        self.multi_instance = multi_instance
        self.dbus_required = dbus_required

        self.run_id = uuid.uuid4().hex
        self.root = Path(tempfile.mkdtemp(prefix=f"gall-e2e-{_sanitize_case_id(case_id)}-")).resolve()

        self.home = self.root / "home"
        self.config = self.root / "config"
        self.data = self.root / "data"
        self.state = self.root / "state"
        self.cache = self.root / "cache"
        self.runtime = self.root / "runtime"
        self.tmp = self.root / "tmp"
        self.mock_bin = self.root / "mock_bin"
        self.work = self.root / "work"
        self.logs = self.root / "logs"
        self.mock_log = self.logs / "mock_calls.jsonl"

        self.dbus_address: str | None = None
        self.dbus_pid: int | None = None
        self._dbus_proc: subprocess.Popen[bytes] | None = None
        self._procs: list[subprocess.Popen[Any]] = []
        self._cleanups: list[Callable[[], None]] = []
        self._entered = False
        self._closed = False

        self.env: dict[str, str] = {}
        self._init_dirs_and_env()

    def _init_dirs_and_env(self) -> None:
        for d in (
            self.home,
            self.config,
            self.data,
            self.state,
            self.cache,
            self.runtime,
            self.tmp,
            self.mock_bin,
            self.work,
            self.logs,
            self.config / "omarchy",
            self.config / "fontconfig",
            self.config / "gall" / "actions.d",
            self.state / "omarchy" / "current",
            self.state / "omarchy" / "toggles" / "hypr",
            self.data / "applications",
            self.data / "dbus-1" / "services",
            self.runtime / "gvfs",
            self.runtime / "hypr",
        ):
            d.mkdir(parents=True, exist_ok=True)
        os.chmod(self.runtime, 0o700)

        self.env = {
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "TERM": "xterm-256color",
            "PATH": f"{self.mock_bin}:/usr/local/bin:/usr/bin:/bin",
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.config),
            "XDG_DATA_HOME": str(self.data),
            "XDG_STATE_HOME": str(self.state),
            "XDG_CACHE_HOME": str(self.cache),
            "XDG_RUNTIME_DIR": str(self.runtime),
            "TMPDIR": str(self.tmp),
            "GALL_E2E_MOCK_LOG": str(self.mock_log),
            "GALL_E2E_RUN_ID": self.run_id,
            # Keep the host's GVfs daemon out of the sandbox: its gvfsd-fuse would mount over the
            # seeded $XDG_RUNTIME_DIR/gvfs fixtures (and leak the mount after the run).
            "GIO_USE_VFS": "local",
        }
        if self.multi_instance:
            self.env["GALL_MULTI_INSTANCE"] = "1"

        if self.allow_gui:
            self._wire_gui_env()

    def _wire_gui_env(self) -> None:
        wayland_sock: Path | None = None
        wayland_name: str | None = None
        x11_disp: str | None = None

        if self.env_probe is not None:
            wayland_sock = self.env_probe.wayland_socket
            if wayland_sock is not None:
                wayland_name = wayland_sock.name
            if self.env_probe.x11_socket is not None:
                x11_disp = self.env_probe.x11_display
        else:
            wd = os.environ.get("WAYLAND_DISPLAY", "").strip()
            if wd:
                candidate = Path(wd) if Path(wd).is_absolute() else Path(
                    os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
                ) / wd
                if candidate.exists():
                    wayland_sock = candidate
                    wayland_name = candidate.name
            if not wayland_sock and os.environ.get("DISPLAY"):
                x11_disp = os.environ.get("DISPLAY")

        if wayland_sock is not None and wayland_name:
            link_path = self.runtime / wayland_name
            if not link_path.exists():
                os.symlink(wayland_sock, link_path)
            self.env["WAYLAND_DISPLAY"] = wayland_name
        elif x11_disp:
            self.env["DISPLAY"] = x11_disp
            xauth = os.environ.get("XAUTHORITY")
            if xauth:
                self.env["XAUTHORITY"] = xauth

    def _assert_isolated_paths(self) -> None:
        root_resolved = self.root.resolve()
        for key in (
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
            "TMPDIR",
        ):
            val = Path(self.env[key]).resolve()
            try:
                val.relative_to(root_resolved)
            except ValueError as exc:
                raise AssertionError(
                    f"Sandbox isolation violated: {key}={val} is outside {root_resolved}"
                ) from exc

    def _start_dbus(self) -> None:
        bus_sock = self.runtime / "bus"
        r_fd, w_fd = os.pipe()
        try:
            cmd = [
                "dbus-daemon",
                "--session",
                "--nofork",
                f"--address=unix:path={bus_sock}",
                f"--print-address={w_fd}",
            ]
            proc = subprocess.Popen(
                cmd,
                env=self.env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                pass_fds=(w_fd,),
                start_new_session=True,
            )
        finally:
            os.close(w_fd)

        try:
            addr_chunks: list[bytes] = []
            deadline = time.monotonic() + 10.0
            os.set_blocking(r_fd, False)
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    err = proc.stderr.read().decode("utf-8", errors="replace") if proc.stderr else ""
                    raise RuntimeError(f"dbus-daemon exited prematurely (rc={proc.returncode}): {err.strip()}")
                try:
                    chunk = os.read(r_fd, 4096)
                    if chunk:
                        addr_chunks.append(chunk)
                        if b"\n" in chunk:
                            break
                    else:
                        break
                except BlockingIOError:
                    time.sleep(0.01)
            raw_addr = b"".join(addr_chunks).decode("utf-8", errors="replace").strip()
            if not raw_addr:
                proc.terminate()
                proc.wait(timeout=2)
                raise RuntimeError("Timed out waiting for dbus-daemon --print-address")
        finally:
            os.close(r_fd)

        self._dbus_proc = proc
        self.dbus_address = raw_addr.splitlines()[0].strip()
        self.dbus_pid = proc.pid
        self.env["DBUS_SESSION_BUS_ADDRESS"] = self.dbus_address

    def add_cleanup(self, fn: Callable[[], None]) -> None:
        self._cleanups.append(fn)

    def __enter__(self) -> "SandboxEnv":
        self._assert_isolated_paths()
        if self.dbus_required:
            self._start_dbus()
        if self.allow_gui and not (self.env_probe and self.env_probe.live_display):
            ensure_headless_display(self, self.env_probe)
        self._entered = True
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _merged_env(self, env_overrides: dict[str, str | None] | None = None) -> dict[str, str]:
        merged = dict(self.env)
        if env_overrides:
            for k, v in env_overrides.items():
                if v is None:
                    merged.pop(k, None)
                else:
                    merged[k] = str(v)
        return merged

    def run(
        self,
        argv: Sequence[str | os.PathLike[str]],
        cwd: str | Path | None = None,
        env_overrides: dict[str, str | None] | None = None,
        timeout: float = 30.0,
        check: bool = False,
        input: str | bytes | None = None,
        text: bool | None = None,
    ) -> subprocess.CompletedProcess[Any]:
        cmd = [os.fspath(a) for a in argv]
        run_cwd = str(cwd) if cwd is not None else str(self.work)
        use_text = (not isinstance(input, (bytes, bytearray))) if text is None else text
        return subprocess.run(
            cmd,
            cwd=run_cwd,
            env=self._merged_env(env_overrides),
            capture_output=True,
            text=use_text,
            input=input,
            timeout=timeout,
            check=check,
        )

    def run_in_dbus(
        self,
        argv: Sequence[str | os.PathLike[str]],
        cwd: str | Path | None = None,
        env_overrides: dict[str, str | None] | None = None,
        timeout: float = 30.0,
        check: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        cmd = [os.fspath(a) for a in argv]
        if self.dbus_address:
            return self.run(cmd, cwd=cwd, env_overrides=env_overrides, timeout=timeout, check=check)
        return self.run(
            ["dbus-run-session", "--", *cmd],
            cwd=cwd,
            env_overrides=env_overrides,
            timeout=timeout,
            check=check,
        )

    def popen(
        self,
        argv: Sequence[str | os.PathLike[str]],
        cwd: str | Path | None = None,
        env_overrides: dict[str, str | None] | None = None,
        **kwargs: Any,
    ) -> subprocess.Popen[Any]:
        cmd = [os.fspath(a) for a in argv]
        run_cwd = str(cwd) if cwd is not None else str(self.work)
        kwargs.setdefault("stdout", subprocess.PIPE)
        kwargs.setdefault("stderr", subprocess.PIPE)
        kwargs.setdefault("start_new_session", True)
        proc = subprocess.Popen(
            cmd,
            cwd=run_cwd,
            env=self._merged_env(env_overrides),
            **kwargs,
        )
        self._procs.append(proc)
        return proc

    def _sweep_proc_environ(self) -> None:
        needles = [f"GALL_E2E_RUN_ID={self.run_id}"]
        if self.dbus_address:
            needles.append(f"DBUS_SESSION_BUS_ADDRESS={self.dbus_address}")
        skip = {os.getpid(), os.getppid()}
        targets: list[int] = []
        try:
            proc_entries = os.listdir("/proc")
        except OSError:
            return
        for name in proc_entries:
            if not name.isdigit():
                continue
            pid = int(name)
            if pid in skip:
                continue
            try:
                raw = Path(f"/proc/{pid}/environ").read_bytes()
            except OSError:
                continue
            env_strs = raw.decode("utf-8", errors="ignore").split("\0")
            if any(n in env_strs for n in needles):
                targets.append(pid)

        for pid in targets:
            try:
                os.kill(pid, signal.SIGTERM)
            except OSError:
                pass
        if targets:
            deadline = time.monotonic() + 1.0
            while time.monotonic() < deadline:
                alive = []
                for pid in targets:
                    try:
                        os.kill(pid, 0)
                        alive.append(pid)
                    except OSError:
                        pass
                if not alive:
                    break
                time.sleep(0.05)
            for pid in targets:
                try:
                    os.kill(pid, signal.SIGKILL)
                except OSError:
                    pass

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True

        for fn in reversed(self._cleanups):
            try:
                fn()
            except Exception:
                pass

        for proc in self._procs:
            if proc.poll() is None:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except OSError:
                    try:
                        proc.terminate()
                    except OSError:
                        pass
        for proc in self._procs:
            if proc.poll() is None:
                try:
                    proc.wait(timeout=1.5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except OSError:
                        try:
                            proc.kill()
                        except OSError:
                            pass
                    try:
                        proc.wait(timeout=1.0)
                    except subprocess.TimeoutExpired:
                        pass

        if self._dbus_proc is not None:
            if self._dbus_proc.poll() is None:
                try:
                    self._dbus_proc.terminate()
                    self._dbus_proc.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    self._dbus_proc.kill()
                    self._dbus_proc.wait(timeout=1.0)
                except OSError:
                    pass
            if self._dbus_proc.stderr:
                try:
                    self._dbus_proc.stderr.close()
                except OSError:
                    pass
        elif self.dbus_pid is not None:
            try:
                os.kill(self.dbus_pid, signal.SIGTERM)
            except OSError:
                pass

        self._sweep_proc_environ()
        shutil.rmtree(self.root, ignore_errors=True)
