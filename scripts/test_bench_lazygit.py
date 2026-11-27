"""Regression test for the fixture cleanup of scripts/bench-lazygit.py.

    python scripts/test_bench_lazygit.py
"""
from __future__ import annotations

import importlib.util
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
SPEC = importlib.util.spec_from_file_location("bench_lazygit", HERE / "bench-lazygit.py")
BENCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCH)


class RemoveTreeTest(unittest.TestCase):
    def test_read_only_pack_files_are_removed(self) -> None:
        # Git writes pack and index files read-only; a full run once failed at its end with
        # `Access is denied` on Windows because shutil.rmtree cannot delete such a file.
        base = Path(tempfile.mkdtemp(prefix="gall-lazygit-bench-test-"))
        pack = base / ".git" / "objects" / "pack"
        pack.mkdir(parents=True)
        for name in ("pack-x.pack", "pack-x.idx"):
            file = pack / name
            file.write_bytes(b"x")
            os.chmod(file, stat.S_IREAD)
        BENCH.remove_tree(base)
        self.assertFalse(base.exists())

    def test_a_missing_folder_is_fine(self) -> None:
        BENCH.remove_tree(Path(tempfile.gettempdir()) / "gall-lazygit-bench-does-not-exist")


if __name__ == "__main__":
    unittest.main()
