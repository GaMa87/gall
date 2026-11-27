#!/usr/bin/env python3
"""Measure lazygit on the fixtures of crates/gall-core/tests/git_speed.rs (WN-61 Windows native, WN-52 Linux, WSL, macOS).

    python scripts/bench-lazygit.py --scenarios all --runs 3

Measures, in a 200x50 terminal and labelled with the environment it ran in:
  start      lazygit start to the first rendered commit list AND file list (first run, then warm median),
             next to the plain `git` baseline of the same data so lazygit's own cost is separable
  keys       key press to updated screen: moving the commit selection, opening a commit's files, staging a file
  scenarios  the 7 benchmark scenarios of WN-30, executed for real with the keys counted and the resulting
             Git state asserted; a scenario that lazygit cannot do is recorded as unsupported, not as a number
  driver     the driver's own overhead, so it can be subtracted by the reader

Numbers are never compared across environments: every row says where it ran (Windows native, WSL, Linux, macOS).
Windows drives lazygit through a ConPTY, the other hosts through a detached tmux session. In WSL the fixtures live on
the Linux filesystem (/tmp), never on /mnt/c.
The terminal driver is in bench_lazygit_core.py (shared layout for WN-52). Nothing of lazygit is copied.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import re
import shutil
import signal
import subprocess
import tempfile
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_lazygit_core as core  # noqa: E402
from bench_lazygit_core import Git, Terminal  # noqa: E402

MS = 1000.0


# ----------------------------------------------------------------------------- driver overhead

HELPER_SH = "echo READY; n=0; while IFS= read -r -n1 ch; do [ \"$ch\" = q ] && break; n=$((n+1)); echo ACK$n; done"

HELPER = (
    "import msvcrt,sys\n"
    "sys.stdout.write('READY\\n');sys.stdout.flush()\n"
    "n=0\n"
    "while True:\n"
    "    ch=msvcrt.getwch()\n"
    "    if ch=='q':break\n"
    "    n+=1;sys.stdout.write('ACK%d\\n'%n);sys.stdout.flush()\n"
)


def driver_overhead(env: dict[str, str], cwd: str, rounds: int = 20) -> dict:
    """Cost of the driver itself: process start to first output (cmd echo) and key to screen (echo helper)."""
    starts = []
    for _ in range(rounds):
        t0 = time.perf_counter()
        term = core.open_terminal(["cmd", "/c", "echo READY"] if os.name == "nt" else ["sh", "-c", "echo READY; sleep 5"], cwd, env)
        try:
            term.wait_for(term.contains("READY"), 10)
            starts.append(time.perf_counter() - t0)
        finally:
            term.close()
    if os.name == "nt":
        helper = Path(cwd) / "echo_helper.py"
        helper.write_text(HELPER, encoding="utf-8")
        helper_argv = [sys.executable, str(helper)]
    else:
        helper_argv = ["bash", "-c", HELPER_SH]
    term = core.open_terminal(helper_argv, cwd, env)
    keys = []
    try:
        term.wait_for(term.contains("READY"), 10)
        for i in range(1, rounds + 1):
            t0 = time.perf_counter()
            term.press("x")
            term.wait_for(term.contains(f"ACK{i}"), 5)
            keys.append(time.perf_counter() - t0)
        term.press("q")
    finally:
        term.close()
    return {
        "spawn_to_output_ms": round(core.median(starts) * MS, 1),
        "key_to_screen_ms": round(core.median(keys) * MS, 1),
        "poll_ms": core.POLL * MS,
    }


# ----------------------------------------------------------------------------- start and key latency


def baseline(git: Git, runs: int) -> float:
    """Plain git doing the work lazygit does for its first screen, one process after another.
    hyperfine times it when it is installed (WN-52 hosts), else this script does."""
    if shutil.which("hyperfine"):
        script = "git status --porcelain=v1 -uall; git log --oneline --decorate -n 300; git branch --list; git stash list"
        out = Path(tempfile.mkdtemp()) / "hf.json"
        subprocess.run(
            ["hyperfine", "--shell", "sh", "--warmup", "2", "--runs", str(max(runs, 10)), "--export-json", str(out),
             f"cd {shlex.quote(str(git.repo))} && ({script}) >/dev/null"],
            env=git.env, capture_output=True, check=True,
        )
        result = json.loads(out.read_text())["results"][0]
        # hyperfine 1.x lists `times`; 2.x lists `measurements` with wall-clock values
        times = result.get("times") or [m["time_wall_clock"]["value"] for m in result["measurements"]]
        return core.median(times)
    samples = []
    for _ in range(runs):
        t0 = time.perf_counter()
        git.run("status", "--porcelain=v1", "-uall")
        git.run("log", "--oneline", "--decorate", "-n", "300")
        git.run("branch", "--list")
        git.run("stash", "list")
        samples.append(time.perf_counter() - t0)
    return core.median(samples)


def start_lazygit(argv, repo: Path, env, expect: list[str]) -> tuple[Terminal, float]:
    t0 = time.perf_counter()
    term = core.open_terminal(argv, str(repo), env)
    try:
        term.wait_for(lambda t: all(e in t.text() for e in expect), 60, f"first screen {expect}")
    except Exception:
        term.close()
        raise
    return term, time.perf_counter() - t0


def quit_lazygit(term: Terminal) -> None:
    term.close()
    time.sleep(0.15)


def measure_start(name: str, repo: Path, env, argv, runs: int) -> dict:
    git = Git(repo, env)
    head = git.run("log", "-1", "--format=%s")
    changed = git.run("status", "--porcelain=v1").splitlines()
    expect = [head] + ([changed[0][3:].strip()] if changed else [])
    samples = []
    for _ in range(runs + 1):
        term, took = start_lazygit(argv, repo, env, expect)
        samples.append(took)
        quit_lazygit(term)
    base = baseline(git, max(runs, 3))
    warm = core.median(samples[1:])
    return {
        "fixture": name,
        "first_run_ms": round(samples[0] * MS),
        "warm_median_ms": round(warm * MS),
        "git_baseline_ms": round(base * MS),
        "lazygit_on_top_ms": round((warm - base) * MS),
        "all_runs_ms": [round(s * MS) for s in samples],
    }


def measure_keys(repo: Path, env, argv, runs: int) -> dict:
    """Latency of single key presses on the 2000-commit repository."""
    git = Git(repo, env)
    head = git.run("log", "-1", "--format=%s")
    term, _ = start_lazygit(argv, repo, env, [head])
    out: dict[str, list[float]] = {"move_selection": [], "open_commit_files": [], "stage_file": []}
    try:
        term.press("4")
        time.sleep(0.3)
        total = int(head.split()[-1])
        for step in range(1, 4 * runs + 1):
            want = f"commit no. {total - step:04d}"
            t0 = time.perf_counter()
            term.press("j")
            term.wait_for(term.contains(want, left=False), 10, want)
            out["move_selection"].append(time.perf_counter() - t0)
        for _ in range(runs):
            t0 = time.perf_counter()
            term.press("enter")
            term.wait_for(term.contains("included in patch"), 10, "commit files panel")
            out["open_commit_files"].append(time.perf_counter() - t0)
            term.press("esc")
            term.wait_for(lambda t: "included in patch" not in t.text(), 10, "back to commits")
        term.press("2")
        time.sleep(0.3)
        files = git.run("diff", "--name-only").splitlines()
        for i in range(runs):
            if i:
                term.press("down")
            name = files[i]
            t0 = time.perf_counter()
            term.press("space")
            term.wait_for(lambda t: git.run("diff", "--cached", "--name-only").count(name) > 0, 10, "staged")
            out["stage_file"].append(time.perf_counter() - t0)
            time.sleep(0.2)
    finally:
        quit_lazygit(term)
    return {k: round(core.median(v) * MS, 1) for k, v in out.items()} | {
        "note": "stage_file is key to the index change seen by `git diff --cached` polled by the driver "
        "(a plain git process per poll, ~tens of ms of its own), not to a repainted screen"
    }


# ----------------------------------------------------------------------------- scenarios

FEATURE_TIP = "feature: add feat-3"
WIP_STASH = "wip: invoice export tweaks"
NEW_MESSAGE = "add three things"


def lf(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="")


def scenario_repo(root: Path, env) -> Path:
    """200 commits, a feature branch to cherry-pick from, a rival branch that conflicts with main's tip, stashes."""
    core.build_repo(root, env, commits=200, files=20, branches=0, changed=0)
    git = Git(root, env)
    lf(root / "work.txt", "".join(f"work line {k}\n" for k in range(1, 41)))
    lf(root / "edit.txt", "".join(f"edit line {k}\n" for k in range(1, 41)))
    git.run("add", "-A")
    git.run("commit", "-qm", "add work and edit")
    git.run("branch", "rival")  # rival starts here, main's tip then edits work.txt line 20
    git.run("checkout", "-q", "-b", "feature", "main~5")
    for n in (1, 2, 3):
        lf(root / f"feat-{n}.txt", f"feature {n}\n")
        git.run("add", "-A")
        git.run("commit", "-qm", f"feature: add feat-{n}")
    git.run("checkout", "-q", "main")
    lines = (root / "work.txt").read_text().splitlines()
    lines[19] = "work line 20 main side"
    lf(root / "work.txt", "\n".join(lines) + "\n")
    git.run("commit", "-qam", "tip: edit work line 20")
    git.run("checkout", "-q", "rival")
    lines[19] = "work line 20 rival side"
    lf(root / "work.txt", "\n".join(lines) + "\n")
    git.run("commit", "-qam", "rival: edit work line 20")
    git.run("checkout", "-q", "main")
    old = int(time.time()) - core.WEEK
    for i, name in enumerate(["wip: a", "wip: b", WIP_STASH, "wip: c"]):
        lf(root / "edit.txt", "".join(f"edit line {k} stash {i}\n" for k in range(1, 41)))
        stamped = dict(env, GIT_COMMITTER_DATE=f"{old + i * 3600} +0000", GIT_AUTHOR_DATE=f"{old + i * 3600} +0000")
        git.run("stash", "push", "-m", name, env=stamped)
    return root


class Unsupported(Exception):
    pass


class Scenario:
    name = ""
    summary = ""

    def prepare(self, git: Git, root: Path) -> None:
        pass

    def drive(self, term: Terminal, git: Git, root: Path) -> None:
        raise NotImplementedError

    def verify(self, git: Git, root: Path, before: dict) -> str:
        """Return a description of the verified state, raise AssertionError otherwise."""
        raise NotImplementedError


def show(term: Terminal, text: str, left: bool | None = None, timeout: float = 10.0) -> None:
    term.wait_for(term.contains(text, left), timeout, repr(text))


def move_to(term: Terminal, label: str) -> None:
    """Press down as often as the list needs to get from the first row to the row showing `label`."""
    rows = [i for i, line in enumerate(term.lines()) if label in line[: core.LEFT]]
    first = [i for i, line in enumerate(term.lines()) if "* main" in line[: core.LEFT]]
    assert rows and first, f"{label} not listed"
    term.press(*["down"] * (rows[0] - first[0]))


class StageTwoLines(Scenario):
    name = "1 stage 2 lines and commit"
    summary = "stage two of four added lines of one file in the staging view, commit with a message"

    def prepare(self, git, root):
        lines = (root / "edit.txt").read_text().splitlines()
        lines[9:9] = ["added a", "added b", "added c", "added d"]
        lf(root / "edit.txt", "\n".join(lines) + "\n")

    def drive(self, term, git, root):
        term.press("2")
        show(term, "edit.txt", left=True)
        term.press("enter")  # the single changed file is selected: open the staging view
        show(term, "added a")
        term.press("a")  # staging view: toggle hunk mode off so single lines can be picked
        term.press("space", "down", "space")
        term.press("esc")
        show(term, "Commit: c")  # back in the side panel
        term.press("c")
        show(term, "Commit summary")
        term.type_text("stage two lines")
        term.press("enter")
        term.wait_for(lambda t: git.run("log", "-1", "--format=%s") == "stage two lines", 10, "commit created")

    def verify(self, git, root, before):
        assert git.run("log", "-1", "--format=%s") == "stage two lines", git.run("log", "-2", "--oneline")
        numstat = git.run("show", "--numstat", "--format=", "HEAD")
        assert numstat.split()[:2] == ["2", "0"] and "edit.txt" in numstat, numstat
        rest = git.run("diff", "--numstat")
        assert rest.split()[:2] == ["2", "0"], rest
        return "HEAD adds 2 lines to edit.txt, 2 lines stay unstaged"


class SquashThree(Scenario):
    name = "2 squash last 3 with a new message"
    summary = "range-select the top 3 commits, squash, reword the result"

    def drive(self, term, git, root):
        term.press("4")
        show(term, "tip: edit work line 20", left=True)
        term.press("v", "down")  # the two top commits are squashed into the third
        term.press("s")
        show(term, "squash the selected")
        term.press("enter")
        term.wait_for(lambda t: "tip: edit work line 20" not in t.region(True), 10, "squashed")
        term.press("r")
        show(term, "Reword commit")
        term.press("ctrl+u")  # clear the prefilled subject
        term.type_text(NEW_MESSAGE)
        term.press("enter")
        term.wait_for(term.contains(NEW_MESSAGE, left=True), 10, "new subject in list")

    def verify(self, git, root, before):
        assert git.run("log", "-1", "--format=%s") == NEW_MESSAGE
        assert int(git.run("rev-list", "--count", "HEAD")) == before["count"] - 2
        assert git.run("rev-parse", "HEAD^{tree}") == before["tree"], "tree changed"
        return "3 commits became 1 with the new subject, tree unchanged"


class CherryPick(Scenario):
    name = "3 cherry-pick from another branch"
    summary = "copy the tip of `feature` in the branch's commit list, paste on main"

    def drive(self, term, git, root):
        term.press("3")
        show(term, "feature", left=True)
        move_to(term, "feature")
        term.press("enter")
        show(term, FEATURE_TIP, left=True)
        term.press("C")
        time.sleep(0.4)  # lazygit marks the copied commit asynchronously
        term.press("esc")
        term.wait_for(lambda t: "Commits (feature)" not in t.region(True), 5, "left the branch commits")
        term.press("4")
        show(term, "Squash: s", timeout=5)
        term.press("V")
        show(term, "Are you sure you want to cherry-pick", timeout=5)
        term.press("enter")
        term.wait_for(lambda t: git.run("log", "-1", "--format=%s") == FEATURE_TIP, 10, "pasted commit")

    def verify(self, git, root, before):
        assert git.run("log", "-1", "--format=%s") == FEATURE_TIP, git.run("log", "-3", "--oneline")
        assert int(git.run("rev-list", "--count", "HEAD")) == before["count"] + 1, git.run("status", "-sb")
        assert (root / "feat-3.txt").exists()
        return "main gained the feature tip, feat-3.txt present"


class BlameJump(Scenario):
    name = "4 who changed line X, jump to its commit in the graph"
    summary = "needs a blame view"

    def drive(self, term, git, root):
        def menu_has(word: str) -> bool:
            term.press("?")
            time.sleep(0.4)
            term.type_text(word)  # the keybinding menu filters as you type
            time.sleep(0.4)
            rows = [l[50:] for l in term.lines() if word.lower() in l.lower() and "Filter" not in l]
            term.press("esc", "esc")
            time.sleep(0.3)
            return bool(rows)

        term.press("4")
        assert menu_has("squash"), "control failed: the menu filter finds nothing even for a known action"
        for panel in ("2", "4"):
            term.press(panel)
            if menu_has("blame"):
                raise Unsupported("a blame entry exists: this scenario needs a manual key sequence")
        raise Unsupported("no entry matching 'blame' in the keybinding menus of the Files and Commits panels (filter checked with a control word)")

    def verify(self, git, root, before):
        return ""


class ResolveRebase(Scenario):
    name = "5 resolve one conflict and continue rebase"
    summary = "rebase main onto `rival`, pick one side of the conflict, continue"

    def drive(self, term, git, root):
        term.press("3")
        show(term, "rival", left=True)
        move_to(term, "rival")
        term.press("r")
        show(term, "Rebase 'main'", timeout=5)
        term.press("enter")
        show(term, "Conflicts!", timeout=15)
        term.press("enter")  # "Conflicts!" dialog: view conflicts, focus moves to the conflicted file
        show(term, "UU work.txt", left=True)
        term.press("enter")  # open the merge-conflict view
        time.sleep(0.3)
        term.press("down")  # second section: main's side of the conflict
        term.press("space")
        show(term, "Continue the rebase", timeout=10)
        term.press("enter")
        term.wait_for(lambda t: not (root / ".git" / "rebase-merge").exists(), 15, "rebase finished")

    def verify(self, git, root, before):
        assert not (root / ".git" / "rebase-merge").exists(), "rebase still running"
        assert git.run("rev-parse", "HEAD~1") == before["rival"], git.run("log", "-3", "--oneline")
        text = (root / "work.txt").read_text()
        assert "<<<<<<<" not in text and ">>>>>>>" not in text
        assert "main side" in text, "main side was not chosen"
        return "main's commit replayed on rival, work.txt has main's side and no markers"


class UndoAmend(Scenario):
    name = "6 undo a bad amend"
    summary = "an amend is made outside, lazygit's undo restores the previous HEAD"

    def prepare(self, git, root):
        git.run("commit", "--amend", "-q", "-m", "oops bad amend", "--allow-empty")

    def drive(self, term, git, root):
        term.press("z")
        show(term, "Undo", timeout=5)
        term.press("enter")
        term.wait_for(lambda t: "oops bad amend" not in t.region(True), 10, "undone")

    def verify(self, git, root, before):
        assert git.run("rev-parse", "HEAD") == before["head"], "HEAD not restored"
        return "HEAD is the commit before the bad amend"


class FindStash(Scenario):
    name = "7 find a named stash a week later and apply it"
    summary = "filter the stash list by name and apply"

    def drive(self, term, git, root):
        term.press("5")
        show(term, WIP_STASH, left=True)
        term.press("/")
        term.type_text("invoice")
        term.press("enter")
        time.sleep(0.3)
        term.press("space")
        show(term, "Stash apply")
        term.press("enter")
        term.wait_for(lambda t: git.run("status", "--porcelain").strip() != "", 10, "applied")

    def verify(self, git, root, before):
        assert "stash 2" in (root / "edit.txt").read_text()
        assert len(git.run("stash", "list").splitlines()) == 4, "apply must keep the stash"
        return "edit.txt carries the named stash, 4 stashes remain"


SCENARIOS = [StageTwoLines, SquashThree, CherryPick, BlameJump, ResolveRebase, UndoAmend, FindStash]


def run_scenario(cls, base: Path, env, argv, run: int) -> dict:
    root = scenario_repo(base / f"scenario-{run}", env)
    git = Git(root, env)
    sc = cls()
    before = {
        "head": git.run("rev-parse", "HEAD"),
        "count": int(git.run("rev-list", "--count", "HEAD")),
        "tree": git.run("rev-parse", "HEAD^{tree}"),
        "rival": git.run("rev-parse", "rival"),
    }
    sc.prepare(git, root)
    term, _ = start_lazygit(argv, root, env, [git.run("log", "-1", "--format=%s")])
    time.sleep(0.4)
    result = {"scenario": sc.name}
    try:
        t0 = time.perf_counter()
        sc.drive(term, git, root)
        took = time.perf_counter() - t0
        result.update(
            status="ok",
            seconds=round(took, 2),
            keys=term.keys,
            typed=term.typed,
            non_text_keys=term.keys - term.typed,
        )
    except Unsupported as why:
        result.update(status="unsupported", reason=str(why), keys_probed=term.keys)
        term.close()
        return result
    except Exception as err:  # noqa: BLE001
        result.update(status="failed", reason=str(err)[:3000])
        term.close()
        return result
    term.close()
    time.sleep(0.2)
    try:
        result["verified"] = sc.verify(git, root, before)
    except AssertionError as err:
        result.update(status="failed", reason=f"state check: {err}")
    return result


def run_scenarios(names: list[str], runs: int, base: Path, env, argv) -> list[dict]:
    out = []
    for cls in SCENARIOS:
        if names != ["all"] and cls.name.split()[0] not in names:
            continue
        results = [run_scenario(cls, base, env, argv, i) for i in range(runs)]
        ok = [r for r in results if r["status"] == "ok"]
        row = dict(results[0])
        if ok:
            row = dict(ok[0])
            row["seconds_median"] = round(core.median([r["seconds"] for r in ok]), 2)
            row["runs_ok"] = f"{len(ok)}/{runs}"
        else:
            row["runs_ok"] = f"0/{runs}"
        out.append(row)
    return out


# ----------------------------------------------------------------------------- main


# A program that starts a child of its own, like lazygit starts Git: the probe of --check-orphans.
PROBE_PROGRAM = "import subprocess, sys, time; p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(300)']); print('GRANDCHILD', p.pid, flush=True); time.sleep(300)"


def probe(mode: str) -> int:
    """Runs the probe program in the ConPTY driver. `close`: closes the driver. `abort`: waits to be killed without closing it."""
    term = core.ConptyTerminal([sys.executable, "-c", PROBE_PROGRAM], os.getcwd(), dict(os.environ))
    term.wait_for(lambda t: "GRANDCHILD" in t.text(), 30, "the child of the probe")
    found = re.search(r"GRANDCHILD (\d+)", term.text())
    print("PID", found.group(1) if found else 0, flush=True)
    if mode == "close":
        term.close()
        print("CLOSED", flush=True)
        return 0
    print("READY", flush=True)
    time.sleep(300)
    return 0


def check_orphans() -> int:
    """After a closed and after a killed run the child of the driven program must be gone (with the old driver it stayed)."""
    if os.name != "nt":
        print("check-orphans: the job object is Windows only; tmux kill-server ends the whole session on the other hosts")
        return 0
    failed = 0
    for mode, marker in (("close", "CLOSED"), ("abort", "READY")):
        run = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--probe", mode], stdout=subprocess.PIPE, text=True)
        pid = 0
        assert run.stdout is not None
        for line in run.stdout:
            if line.startswith("PID "):
                pid = int(line.split()[1])
            if line.startswith(marker):
                break
        if mode == "abort":
            run.kill()  # the run is cut short without any cleanup of its own
        run.wait(timeout=60)
        deadline = time.time() + 10
        while pid and core.pid_alive(pid) and time.time() < deadline:
            time.sleep(0.2)
        left = bool(pid) and core.pid_alive(pid)
        print(f"check-orphans {mode}: {'FAILED, child ' + str(pid) + ' is still running' if left or not pid else 'ok, the child is gone'}", flush=True)
        if left or not pid:
            failed += 1
            if left:
                subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True, check=False)
    return 1 if failed else 0


def _unlock_and_retry(function, path, _error) -> None:
    """Git writes pack and index files read-only; on Windows such a file cannot be deleted until the attribute is cleared."""
    import stat

    os.chmod(path, stat.S_IWRITE)
    function(path)


def remove_tree(base: Path) -> None:
    """Removes the fixtures; on Windows a folder that a process was just stopped in needs a moment to be released, and
    Git's read-only pack files need their attribute cleared first (a full run failed at its end with `Access is denied`)."""
    handler = {"onexc": _unlock_and_retry} if sys.version_info >= (3, 12) else {"onerror": _unlock_and_retry}
    for attempt in range(5):
        try:
            shutil.rmtree(base, **handler)
        except OSError:
            if attempt == 4:
                raise
        if not base.exists():
            return
        time.sleep(0.5)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scenarios", default="all", help="all, none, or comma list of scenario numbers 1-7")
    ap.add_argument("--runs", type=int, default=3, help="runs of each scenario and key measurement")
    ap.add_argument("--start-runs", type=int, default=10, help="warm starts per fixture, the median is reported")
    ap.add_argument("--only", default="start,keys,driver", help="comma list of start, keys, driver; 'none' to skip")
    ap.add_argument("--tree-files", type=int, default=5000)
    ap.add_argument("--tree-changed", type=int, default=500)
    ap.add_argument("--json", help="write the raw result here")
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--check-orphans", action="store_true", help="only check that a closed and a killed run leave no child process (Windows)")
    ap.add_argument("--probe", choices=["close", "abort"], help=argparse.SUPPRESS)
    args = ap.parse_args()
    if args.probe:
        return probe(args.probe)
    if args.check_orphans:
        return check_orphans()
    # A terminated run still runs its cleanup (closing the terminals, removing the fixtures).
    for name in ("SIGTERM", "SIGBREAK"):
        if hasattr(signal, name):
            signal.signal(getattr(signal, name), lambda *_: sys.exit(143))

    env_name = core.environment()
    base = core.workdir()
    if env_name == "WSL" and str(base).startswith("/mnt/"):
        raise SystemExit("in WSL the fixtures must be on the Linux filesystem, not " + str(base))
    home = base / "home"
    env, argv = core.lazygit_env(home)
    report: dict = {
        "environment": env_name,
        "lazygit": core.version_line(["lazygit", "--version"]),
        "git": core.version_line(["git", "--version"]),
        **({} if os.name == "nt" else {"tmux": core.version_line(["tmux", "-V"]), "hyperfine": core.version_line(["hyperfine", "--version"])}),
        "python": sys.version.split()[0],
        "terminal": f"{(core.ConptyTerminal if os.name == 'nt' else core.TmuxTerminal).name}, {core.COLS}x{core.ROWS}, poll {core.POLL * MS:.0f} ms",
        "os": core.platform.platform(),
        "filesystem": (f"native NTFS folder ({base})" if os.name == "nt" else f"Linux filesystem ({base})"),
        "git_config": "core.autocrlf=false, no system or user config, hooks off",
    }
    only = [] if args.only == "none" else args.only.split(",")
    try:
        if "driver" in only:
            report["driver_overhead"] = driver_overhead(env, str(base))
        if "start" in only:
            shapes = {
                "200 commits": dict(commits=200, files=20, branches=5, changed=3),
                "2000 commits": dict(commits=2000, files=20, branches=5, changed=3),
                "large working tree": dict(commits=50, files=args.tree_files, branches=5, changed=args.tree_changed),
                "300 branches": dict(commits=200, files=20, branches=300, changed=3),
            }
            starts = []
            for name, shape in shapes.items():
                repo = core.build_repo(base / name.replace(" ", "-"), env, **shape)
                starts.append(measure_start(name, repo, env, argv, args.start_runs))
                print(f"start {name}: {starts[-1]['warm_median_ms']} ms", flush=True)
            report["start"] = starts
        if "keys" in only:
            repo = core.build_repo(base / "keys", env, commits=2000, files=20, branches=5, changed=3)
            report["keys_2000_commits_ms"] = measure_keys(repo, env, argv, args.runs)
        if args.scenarios != "none":
            names = ["all"] if args.scenarios == "all" else args.scenarios.split(",")
            report["scenarios"] = run_scenarios(names, args.runs, base, env, argv)
    finally:
        if not args.keep:
            remove_tree(base)

    print(json.dumps(report, indent=2, ensure_ascii=False))
    if args.json:
        Path(args.json).write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    bad = [r for r in report.get("scenarios", []) if r["status"] == "failed"]
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
