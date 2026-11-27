#!/usr/bin/env python3
"""Repositories of the git-baseline self-test: `python3 selftest_git_baseline_fixture.py <folder>`.

The seven benchmark scenarios run on the repository that scripts/bench-lazygit.py drives lazygit on, so the
steps Gall needs can be compared with the ones lazygit needs. Every scenario gets a copy of its own for the
keyboard (`<n>k`) and for the mouse (`<n>m`). `c200` and `c2000` are the repositories of the latency rows.
"""

from __future__ import annotations

import importlib.util
import shutil
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import bench_lazygit_core as core  # noqa: E402

spec = importlib.util.spec_from_file_location("bench_lazygit", HERE / "bench-lazygit.py")
assert spec and spec.loader
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

CLOCK = int(time.time()) - 7200
plain_run = core.Git.run


def dated_run(self, *args, env=None, **options):
    """Commits made within one second have no order, and then neither the branch list nor the graph has one."""
    global CLOCK
    if args[0] == "commit" and env is None:
        CLOCK += 60
        env = dict(self.env, GIT_AUTHOR_DATE=f"{CLOCK} +0000", GIT_COMMITTER_DATE=f"{CLOCK} +0000")
    return plain_run(self, *args, env=env, **options)


core.Git.run = dated_run


def local_config(root: Path) -> None:
    """What the private configuration of the build sets, kept in the repository for the application."""
    excludes = root / ".git" / "empty-excludes"
    excludes.write_text("", encoding="utf-8")
    git = core.Git(root, ENV)
    for key, value in (
        ("user.name", "Bench"),
        ("user.email", "bench@example.com"),
        ("core.autocrlf", "false"),
        ("core.eol", "lf"),
        ("core.excludesFile", excludes.as_posix()),
        ("commit.gpgSign", "false"),
        ("gc.auto", "0"),
    ):
        git.run("config", key, value)


def remove(root: Path) -> None:
    # Git's objects are read-only, which Windows refuses to delete.
    for path in root.rglob("*"):
        if not path.is_symlink():
            path.chmod(0o700 if path.is_dir() else 0o600)
    shutil.rmtree(root)


def main(target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    base = target / "base"
    bench.scenario_repo(base, ENV)
    local_config(base)
    for n, cls in enumerate(bench.SCENARIOS, 1):
        for route in "km":
            root = target / f"{n}{route}"
            shutil.copytree(base, root, symlinks=True)
            core.Git(root, ENV).run("config", "core.excludesFile", (root / ".git" / "empty-excludes").as_posix())
            cls().prepare(core.Git(root, ENV), root)
    # Scenario 6 again, undone without the journal of Gall, which does not know the amend.
    for route in "km":
        root = target / f"10{route}"
        shutil.copytree(base, root, symlinks=True)
        core.Git(root, ENV).run("config", "core.excludesFile", (root / ".git" / "empty-excludes").as_posix())
        bench.UndoAmend().prepare(core.Git(root, ENV), root)
    # And for a bad rebase, which the lazygit yardstick does not cover.
    for route in "km":
        root = target / f"8{route}"
        shutil.copytree(base, root, symlinks=True)
        git = core.Git(root, ENV)
        git.run("config", "core.excludesFile", (root / ".git" / "empty-excludes").as_posix())
        git.run("rebase", "-q", "-X", "theirs", "rival")
    # Three commits at once, on the untouched repository.
    for route in "km":
        root = target / f"9{route}"
        shutil.copytree(base, root, symlinks=True)
        core.Git(root, ENV).run("config", "core.excludesFile", (root / ".git" / "empty-excludes").as_posix())
    remove(base)
    for commits in (200, 2000):
        root = target / f"c{commits}"
        core.build_repo(root, ENV, commits=commits, files=20, branches=5, changed=3)
        local_config(root)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with tempfile.TemporaryDirectory(prefix="gall-baseline-home-") as home:
        ENV = core.git_env(Path(home))
        main(Path(sys.argv[1]).resolve())
