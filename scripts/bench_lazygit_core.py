"""Shared part of scripts/bench-lazygit.py: terminal drivers, fixtures and config.

The layout is shared between environments so WN-52 (Linux, WSL, macOS with tmux) can adopt it:

    Terminal            driver interface: press / type_text / wait_for / text / close
    ConptyTerminal      native Windows driver (pywinpty ConPTY + pyte screen buffer)
    environment()       what the numbers are labelled with
    build_*()           disposable fixtures with the shapes of crates/gall-core/tests/git_speed.rs
    lazygit_env()       isolated lazygit config and Git config, identical for every run

Nothing here copies lazygit code or configuration into Gall (docs/00-clean-room-policy.md):
lazygit is only driven from the outside through its terminal.
"""

from __future__ import annotations

import os
import platform
import shlex
import shutil
import statistics
import subprocess
import tempfile
import threading
import time
from pathlib import Path

COLS, ROWS = 200, 50
POLL = 0.005  # seconds between screen reads, the same as `tmux capture-pane` polling in WN-52
LEFT = 66  # columns [0, LEFT) are the lazygit side panels, the rest is the main view

KEYS = {
    "enter": "\r",
    "esc": "\x1b",
    "tab": "\t",
    "space": " ",
    "backspace": "\x7f",
    "ctrl+u": "\x15",
    "up": "\x1b[A",
    "down": "\x1b[B",
    "right": "\x1b[C",
    "left": "\x1b[D",
}

AUTHOR = "Bench <bench@example.com>"
EPOCH = 1_700_000_000
WEEK = 7 * 24 * 3600


class Timeout(RuntimeError):
    pass


class Terminal:
    """A virtual terminal of COLS x ROWS cells running one program.

    `keys` counts every key actually sent, `typed` how many of those were literal text
    (a commit message, a search), so a scenario reports both.
    """

    name = "abstract"

    def __init__(self) -> None:
        self.keys = 0
        self.typed = 0

    def _send(self, data: str) -> None:
        raise NotImplementedError

    def text(self) -> str:
        raise NotImplementedError

    def alive(self) -> bool:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError

    def press(self, *names: str) -> None:
        for name in names:
            self.keys += 1
            self._send(KEYS.get(name, name))

    def type_text(self, text: str) -> None:
        for ch in text:
            self.keys += 1
            self.typed += 1
            self._send(ch)

    def lines(self) -> list[str]:
        return self.text().split("\n")

    def region(self, left: bool) -> str:
        rows = self.lines()
        return "\n".join(r[:LEFT] if left else r[LEFT:] for r in rows)

    def wait_for(self, check, timeout: float = 20.0, what: str = "screen") -> float:
        """Poll every POLL seconds until check(self) is true; returns seconds since the call."""
        start = time.perf_counter()
        while True:
            if check(self):
                return time.perf_counter() - start
            if time.perf_counter() - start > timeout:
                raise Timeout(f"timed out after {timeout:.0f}s waiting for {what}\n{self.text()}")
            time.sleep(POLL)

    def contains(self, needle: str, left: bool | None = None):
        def check(t: "Terminal") -> bool:
            hay = t.text() if left is None else t.region(left)
            return needle in hay

        return check


class KillOnClose:
    """A Windows job object that ends every process started inside it when its handle closes (or when this process dies, so an
    interrupted run leaves nothing behind either). Processes lazygit starts belong to the job of lazygit, and they used to outlive
    it: `git.exe` waiting on a pipe nobody reads, for hours."""

    def __init__(self, pid: int) -> None:
        import ctypes
        from ctypes import wintypes

        class IoCounters(ctypes.Structure):
            _fields_ = [(name, ctypes.c_uint64) for name in ("ReadOperationCount", "WriteOperationCount", "OtherOperationCount", "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]

        class Basic(ctypes.Structure):
            _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64), ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t), ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD), ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD), ("SchedulingClass", wintypes.DWORD)]

        class Extended(ctypes.Structure):
            _fields_ = [("BasicLimitInformation", Basic), ("IoInfo", IoCounters), ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t), ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]

        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        k32.CreateJobObjectW.restype = wintypes.HANDLE
        k32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
        k32.OpenProcess.restype = wintypes.HANDLE
        k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        k32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        k32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
        k32.CloseHandle.argtypes = [wintypes.HANDLE]
        self._close = k32.CloseHandle
        self._job = k32.CreateJobObjectW(None, None)
        if not self._job:
            raise OSError(ctypes.get_last_error(), "CreateJobObjectW failed")
        info = Extended()
        info.BasicLimitInformation.LimitFlags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not k32.SetInformationJobObject(self._job, 9, ctypes.byref(info), ctypes.sizeof(info)):
            raise OSError(ctypes.get_last_error(), "SetInformationJobObject failed")
        process = k32.OpenProcess(0x0100 | 0x0001, False, pid)  # PROCESS_SET_QUOTA | PROCESS_TERMINATE
        if not process:
            raise OSError(ctypes.get_last_error(), "OpenProcess failed")
        try:
            if not k32.AssignProcessToJobObject(self._job, process):
                raise OSError(ctypes.get_last_error(), "AssignProcessToJobObject failed")
        finally:
            k32.CloseHandle(process)

    def close(self) -> None:
        if self._job:
            self._close(self._job)
            self._job = None


def pid_alive(pid: int) -> bool:
    """Whether a Windows process still runs (a process that ended but is still held open by someone counts as ended)."""
    import ctypes
    from ctypes import wintypes

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    try:
        code = wintypes.DWORD()
        return bool(k32.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259  # STILL_ACTIVE
    finally:
        k32.CloseHandle(handle)


class ConptyTerminal(Terminal):
    """Native Windows driver: the program runs in a real ConPTY, a reader thread feeds its
    output to a pyte screen and wait_for() reads that screen buffer."""

    name = "pywinpty ConPTY + pyte screen buffer"

    def __init__(self, argv: list[str], cwd: str, env: dict[str, str]) -> None:
        super().__init__()
        import pyte
        import winpty

        self._screen = pyte.Screen(COLS, ROWS)
        self._stream = pyte.Stream(self._screen)
        self._lock = threading.Lock()
        self._proc = winpty.PtyProcess.spawn(argv, cwd=cwd, env=env, dimensions=(ROWS, COLS))
        # Everything the program starts ends with it, also when this run is cut short.
        self._job = KillOnClose(self._proc.pid)
        self._thread = threading.Thread(target=self._pump, daemon=True)
        self._thread.start()

    def _pump(self) -> None:
        while True:
            try:
                data = self._proc.read(65536)
            except Exception:
                return
            if data:
                with self._lock:
                    self._stream.feed(data)

    def _send(self, data: str) -> None:
        self._proc.write(data)

    def text(self) -> str:
        with self._lock:
            return "\n".join(line.rstrip() for line in self._screen.display)

    def alive(self) -> bool:
        return bool(self._proc.isalive())

    def close(self) -> None:
        # The tree first, while the parent still lives (taskkill finds the children through it); the job then ends what is left.
        try:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(self._proc.pid)], capture_output=True, timeout=15, check=False)
        except Exception:
            pass
        try:
            if self._proc.isalive():
                self._proc.terminate(force=True)
        except Exception:
            pass
        self._job.close()


TMUX_NAMED = {
    "enter": "Enter",
    "esc": "Escape",
    "tab": "Tab",
    "space": "Space",
    "backspace": "BSpace",
    "ctrl+u": "C-u",
    "up": "Up",
    "down": "Down",
    "right": "Right",
    "left": "Left",
}


class TmuxTerminal(Terminal):
    """Linux, WSL and macOS driver: lazygit runs in a detached tmux session of COLS x ROWS cells and wait_for()
    reads it with `tmux capture-pane -p` every POLL seconds. Every poll starts one tmux process (a few ms), which is
    part of the numbers and is reported by driver_overhead()."""

    name = "tmux detached session, capture-pane polling"
    _count = 0

    def __init__(self, argv: list[str], cwd: str, env: dict[str, str]) -> None:
        super().__init__()
        TmuxTerminal._count += 1
        self._sock = f"gallbench{os.getpid()}"
        self._session = f"b{TmuxTerminal._count}"
        self._env = env
        subprocess.run(
            ["tmux", "-L", self._sock, "-f", "/dev/null", "new-session", "-d", "-s", self._session,
             "-x", str(COLS), "-y", str(ROWS), "-c", cwd, " ".join(shlex.quote(a) for a in argv)],
            env=env, check=True, capture_output=True,
        )

    def _tmux(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(["tmux", "-L", self._sock, *args], env=self._env, capture_output=True, text=True)

    def _send(self, data: str) -> None:
        named = {"\r": "Enter", "\x1b": "Escape", "\t": "Tab", "\x7f": "BSpace", "\x15": "C-u"}
        if data in named:
            self._tmux("send-keys", "-t", self._session, named[data])
        elif data.startswith("\x1b["):
            self._tmux("send-keys", "-t", self._session, {"A": "Up", "B": "Down", "C": "Right", "D": "Left"}[data[-1]])
        else:
            self._tmux("send-keys", "-t", self._session, "-l", "--", data)

    def text(self) -> str:
        out = self._tmux("capture-pane", "-p", "-t", self._session)
        return "\n".join(line.rstrip() for line in out.stdout.split("\n")[:ROWS])

    def alive(self) -> bool:
        return self._tmux("has-session", "-t", self._session).returncode == 0

    def close(self) -> None:
        self._tmux("kill-server")


def open_terminal(argv: list[str], cwd: str, env: dict[str, str]) -> Terminal:
    if os.name == "nt":
        return ConptyTerminal(argv, cwd, env)
    return TmuxTerminal(argv, cwd, env)


def environment() -> str:
    """The label every number carries; numbers of different environments are never compared."""
    if os.name == "nt":
        return "Windows native"
    if platform.system() == "Darwin":
        return "macOS"
    try:
        if "microsoft" in Path("/proc/version").read_text().lower():
            return "WSL"
    except OSError:
        pass
    return "Linux"


def version_line(argv: list[str]) -> str:
    try:
        out = subprocess.run(argv, capture_output=True, text=True, timeout=20)
        return (out.stdout or out.stderr).strip().splitlines()[0]
    except Exception as err:  # noqa: BLE001
        return f"unavailable ({err})"


def median(values: list[float]) -> float:
    return statistics.median(values) if values else float("nan")


# ----------------------------------------------------------------------------- Git plumbing


def git_env(home: Path) -> dict[str, str]:
    """Environment with a private Git configuration so no user or system setting leaks in."""
    cfg = home / "gitconfig"
    if not cfg.exists():
        cfg.write_text(
            "[user]\n\tname = Bench\n\temail = bench@example.com\n"
            "[core]\n\tautocrlf = false\n\teol = lf\n\thooksPath = " + os.devnull + "\n\tfsmonitor = false\n"
            "[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n"
            "[advice]\n\tdetachedHead = false\n[gc]\n\tauto = 0\n",
            encoding="utf-8",
        )
    env = dict(os.environ)
    env.update(
        GIT_CONFIG_GLOBAL=str(cfg),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_TERMINAL_PROMPT="0",
        GIT_EDITOR="true",
    )
    return env


class Git:
    def __init__(self, repo: Path, env: dict[str, str]) -> None:
        self.repo, self.env = Path(repo), env

    def run(self, *args: str, input: bytes | None = None, check: bool = True, env: dict | None = None) -> str:
        out = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            input=input,
            capture_output=True,
            env=env or self.env,
        )
        if check and out.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {out.stderr.decode(errors='replace')}")
        return out.stdout.decode("utf-8", errors="replace").strip()


def _data(out: bytearray, text: str) -> None:
    raw = text.encode("utf-8")
    out += b"data %d\n" % len(raw) + raw + b"\n"


def _body(path: str, rev: int, lines: int = 40) -> str:
    return "".join(f"{path} line {k} rev {rev}\n" for k in range(1, lines + 1))


def history_stream(
    commits: int, files: int, branches: int, *, files_per_commit_one: bool = True, prefix: str = "f"
) -> bytes:
    """A fast-import stream of `commits` commits over `files` files with `branches` extra refs.

    The first commit adds every file; every later commit rewrites one file. Subjects are
    `commit no. NNNN` so a screen read can tell which commit is selected.
    """
    out = bytearray()
    names = [f"{prefix}{i:05d}.txt" for i in range(files)]
    marks: list[int] = []
    for n in range(1, commits + 1):
        out += b"commit refs/heads/main\n"
        out += b"mark :%d\n" % n
        out += f"committer {AUTHOR} {EPOCH + n * 60} +0000\n".encode()
        _data(out, f"commit no. {n:04d}")
        if n > 1:
            out += b"from :%d\n" % (n - 1)
        if n == 1:
            for name in names:
                out += f"M 100644 inline {name}\n".encode()
                _data(out, _body(name, 0))
        else:
            name = names[n % files]
            out += f"M 100644 inline {name}\n".encode()
            _data(out, _body(name, n))
        marks.append(n)
    for b in range(branches):
        mark = marks[(b * max(1, commits // (branches + 1)) + 1) % len(marks)]
        out += f"reset refs/heads/branch-{b:03d}\nfrom :{mark}\n\n".encode()
    return bytes(out)


def build_repo(root: Path, env: dict[str, str], *, commits: int, files: int, branches: int, changed: int) -> Path:
    """Create a repository like the shapes of git_speed.rs and leave `changed` files modified."""
    if root.exists():
        shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True, exist_ok=True)
    git = Git(root, env)
    git.run("init", "-q", "-b", "main")
    git.run("config", "core.autocrlf", "false")
    git.run("fast-import", "--quiet", input=history_stream(commits, files, branches))
    git.run("reset", "-q", "--hard", "main")
    for i in range(changed):
        name = f"f{(i * 7) % files:05d}.txt"
        path = root / name
        path.write_text(path.read_text(encoding="utf-8").replace("line 3 ", "line 3 changed ", 1), encoding="utf-8", newline="")
    for i in range(min(20, changed)):
        (root / f"new-{i:02d}.txt").write_text("untracked\n", encoding="utf-8", newline="")
    return root


# ----------------------------------------------------------------------------- lazygit setup

LAZYGIT_CONFIG = """\
disableStartupPopups: true
notARepository: skip
confirmOnQuit: false
promptToReturnFromSubprocess: false
update:
  method: never
git:
  autoFetch: false
gui:
  showRandomTip: false
  showCommandLog: false
  nerdFontsVersion: ""
  language: en
"""


def lazygit_env(home: Path) -> tuple[dict[str, str], list[str]]:
    home.mkdir(parents=True, exist_ok=True)
    config = home / "lazygit.yml"
    config.write_text(LAZYGIT_CONFIG, encoding="utf-8")
    env = git_env(home)
    env["LG_CONFIG_DIR"] = str(home)
    return env, ["lazygit", "--use-config-file", str(config)]


def workdir() -> Path:
    return Path(tempfile.mkdtemp(prefix="gall-lazygit-bench-"))
