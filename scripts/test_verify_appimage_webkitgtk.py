#!/usr/bin/env python3
"""Regression tests for scripts/verify-appimage-webkitgtk.sh."""

from __future__ import annotations

import os
import stat
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "verify-appimage-webkitgtk.sh"


def write_executable(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def write_fake_elf(path: Path, build_id: str, glibc: str, extra: str = "") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\x7fELF" + f"\nBUILD_ID={build_id}\n{glibc}\n{extra}".encode("utf-8"))


class VerifyAppImageWebKitGtkTest(unittest.TestCase):
    def make_env(
        self,
        tmp: Path,
        wk_ver: str,
        jsc_ver: str,
        arch: str = "amd64",
        missing_pkg: bool = False,
    ) -> tuple[dict[str, str], dict[str, Path], dict[str, str]]:
        bin_dir = tmp / "bin"
        lib_dir = tmp / "usr" / "lib"
        helper_dir = lib_dir / "webkit2gtk-4.1"
        bin_dir.mkdir(parents=True, exist_ok=True)
        helper_dir.mkdir(parents=True, exist_ok=True)

        ids = {
            "wk": "aa11bb22cc33",
            "jsc": "dd44ee55ff66",
            "webproc": "112233445566",
            "netproc": "665544332211",
            "bundle": "abcdef123456",
        }
        host_files = {
            "wk": lib_dir / "libwebkit2gtk-4.1.so.0.18.0",
            "jsc": lib_dir / "libjavascriptcoregtk-4.1.so.0.18.0",
            "webproc": helper_dir / "WebKitWebProcess",
            "netproc": helper_dir / "WebKitNetworkProcess",
            "bundle": helper_dir / "libwebkit2gtkinjectedbundle.so",
        }
        for key, path in host_files.items():
            write_fake_elf(path, ids[key], "GLIBC_2.38")

        if missing_pkg:
            dpkg_query_body = "#!/usr/bin/env bash\nexit 1\n"
        else:
            dpkg_query_body = f"""#!/usr/bin/env bash
pkg="${{@: -1}}"
case "$pkg" in
  libwebkit2gtk-4.1-0:{arch})
    printf 'install ok installed\\tlibwebkit2gtk-4.1-0\\t{arch}\\t{wk_ver}\\twebkit2gtk ({wk_ver})\\n'
    ;;
  libjavascriptcoregtk-4.1-0:{arch})
    printf 'install ok installed\\tlibjavascriptcoregtk-4.1-0\\t{arch}\\t{jsc_ver}\\twebkit2gtk ({jsc_ver})\\n'
    ;;
  *)
    exit 1
    ;;
esac
"""
        write_executable(bin_dir / "dpkg-query", dpkg_query_body)

        write_executable(
            bin_dir / "dpkg",
            f"""#!/usr/bin/env python3
import sys

def parse_ver(v):
    return tuple(int(p) for p in v.split("."))

if sys.argv[1] == "--print-architecture":
    print("{arch}")
elif sys.argv[1] == "-L":
    if sys.argv[2] == "libwebkit2gtk-4.1-0:{arch}":
        print("{host_files['wk']}")
        print("{host_files['webproc']}")
        print("{host_files['netproc']}")
        print("{host_files['bundle']}")
    elif sys.argv[2] == "libjavascriptcoregtk-4.1-0:{arch}":
        print("{host_files['jsc']}")
    else:
        sys.exit(1)
elif sys.argv[1] == "--compare-versions" and sys.argv[3] == "ge":
    if not sys.argv[2] or not sys.argv[4]:
        sys.exit(1)
    sys.exit(0 if parse_ver(sys.argv[2]) >= parse_ver(sys.argv[4]) else 1)
else:
    sys.exit(2)
""",
        )

        write_executable(
            bin_dir / "readelf",
            """#!/usr/bin/env python3
import sys

if sys.argv[1] == "-n":
    data = open(sys.argv[2], "rb").read().decode("utf-8", "replace")
    for line in data.splitlines():
        if line.startswith("BUILD_ID="):
            print("    Build ID:", line.split("=", 1)[1])
            sys.exit(0)
    sys.exit(1)
elif sys.argv[1] == "--version-info":
    data = open(sys.argv[3], "rb").read().decode("utf-8", "replace")
    print(data)
else:
    sys.exit(2)
""",
        )

        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"
        env.pop("GITHUB_STEP_SUMMARY", None)
        return env, host_files, ids

    def populate_valid_appdir(self, appdir: Path, ids: dict[str, str]) -> None:
        lib = appdir / "usr" / "lib"
        helpers = lib / "webkit2gtk-4.1"
        write_fake_elf(lib / "libwebkit2gtk-4.1.so.0.18.0", ids["wk"], "GLIBC_2.38", "RPATH=$ORIGIN\n")
        (lib / "libwebkit2gtk-4.1.so.0").symlink_to("libwebkit2gtk-4.1.so.0.18.0")
        write_fake_elf(lib / "libjavascriptcoregtk-4.1.so.0.18.0", ids["jsc"], "GLIBC_2.38", "RPATH=$ORIGIN\n")
        (lib / "libjavascriptcoregtk-4.1.so.0").symlink_to("libjavascriptcoregtk-4.1.so.0.18.0")
        write_fake_elf(helpers / "WebKitWebProcess", ids["webproc"], "GLIBC_2.38")
        write_fake_elf(helpers / "WebKitNetworkProcess", ids["netproc"], "GLIBC_2.38")
        write_fake_elf(helpers / "libwebkit2gtkinjectedbundle.so", ids["bundle"], "GLIBC_2.38")
        write_fake_elf(appdir / "usr" / "bin" / "gall", "000000000001", "GLIBC_2.39")
        write_fake_elf(lib / "libextra.so.1", "000000000002", "GLIBC_2.41")

    def run_check(self, args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_accepts_patched_debian_trixie_host_and_appdir_and_scans_all_elf_glibc(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            env, _, ids = self.make_env(tmp, "2.54.3-1~deb13u1", "2.54.3-1~deb13u1")
            appdir = tmp / "AppDir"
            self.populate_valid_appdir(appdir, ids)

            out_file = tmp / "provenance.txt"
            res = self.run_check(["--check-appimage", str(appdir), "--output", str(out_file)], env)
            self.assertEqual(res.returncode, 0, res.stderr)
            text = out_file.read_text(encoding="utf-8")
            self.assertIn("WebKitGTK security verification: PASSED", text)
            self.assertIn("2.54.3-1~deb13u1", text)
            self.assertIn("aa11bb22cc33 (matches vendor)", text)
            self.assertIn("dd44ee55ff66 (matches vendor)", text)
            self.assertIn("Bundled WebKitWebProcess:", text)
            self.assertIn("AppImage max GLIBC symbol (all bundled ELF files): GLIBC_2.41", text)

    def test_extracts_real_appimage_offset_into_nonexistent_child_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            env, _, ids = self.make_env(tmp, "2.54.3-1~deb13u1", "2.54.3-1~deb13u1")
            bin_dir = tmp / "bin"

            # Build a fake Type-2 AppImage: ELF64 header (e_shoff=64, 1 section of 64B -> end=128),
            # a false-positive "hsqs" at offset 128 with invalid version, and a valid SquashFS v4
            # superblock at offset 256.
            elf = bytearray(256)
            elf[0:4] = b"\x7fELF"
            elf[4] = 2  # ELFCLASS64
            elf[5] = 1  # ELFDATA2LSB
            struct.pack_into("<QQ", elf, 32, 0, 64)
            struct.pack_into("<HHHH", elf, 54, 0, 0, 64, 1)
            elf[128:132] = b"hsqs"  # false positive (s_major == 0)

            sqfs = bytearray(96)
            sqfs[0:4] = b"hsqs"
            struct.pack_into("<IIHHHHHH", sqfs, 12, 131072, 0, 1, 17, 0x00C0, 1, 4, 0)

            appimage = tmp / "Gall_0.3.2_amd64.AppImage"
            appimage.write_bytes(bytes(elf) + bytes(sqfs))

            fixture = tmp / "fixture"
            self.populate_valid_appdir(fixture, ids)

            # Fake unsquashfs asserts -d points to a non-existent dir and -o is 256.
            write_executable(
                bin_dir / "unsquashfs",
                f"""#!/usr/bin/env python3
import shutil, sys
from pathlib import Path
args = sys.argv[1:]
off = args[args.index("-o") + 1]
dest = Path(args[args.index("-d") + 1])
if off != "256":
    sys.stderr.write(f"unexpected offset: {{off}}\\n")
    sys.exit(1)
if dest.exists():
    sys.stderr.write(f"destination already exists: {{dest}}\\n")
    sys.exit(1)
shutil.copytree("{fixture}", dest, symlinks=True)
""",
            )

            res = self.run_check(["--check-appimage", str(appimage)], env)
            self.assertEqual(res.returncode, 0, res.stderr)
            self.assertIn("AppImage: Gall_0.3.2_amd64.AppImage", res.stdout)

    def test_rejects_vulnerable_ubuntu_versions_and_missing_packages(self) -> None:
        for bad_ver in ("2.50.4-0ubuntu0.22.04.1", "2.52.6-0ubuntu0.24.04.2"):
            with tempfile.TemporaryDirectory() as td:
                env, _, _ = self.make_env(Path(td), bad_ver, bad_ver)
                res = self.run_check(["--check-host"], env)
                self.assertNotEqual(res.returncode, 0, bad_ver)
                self.assertIn("older than required minimum 2.54.3", res.stderr)

        with tempfile.TemporaryDirectory() as td:
            env, _, _ = self.make_env(Path(td), "2.54.3-1~deb13u1", "2.54.3-1~deb13u1", missing_pkg=True)
            res = self.run_check(["--check-host"], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("package is not installed", res.stderr)

        with tempfile.TemporaryDirectory() as td:
            env, _, _ = self.make_env(Path(td), "2.54.3-1~deb13u1", "2.54.3-1~deb13u1")
            res = self.run_check(["--check-host", "--min-version", ""], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("--min-version must not be empty", res.stderr)

    def test_rejects_duplicate_stale_copy_escaping_symlink_and_missing_soname(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            env, host_files, ids = self.make_env(tmp, "2.54.3-1~deb13u1", "2.54.3-1~deb13u1")

            # Duplicate stale copy alongside a valid copy must fail.
            dup = tmp / "dup"
            self.populate_valid_appdir(dup, ids)
            write_fake_elf(
                dup / "usr" / "lib" / "x86_64-linux-gnu" / "libwebkit2gtk-4.1.so.0.14.0",
                "deadbeef0000",
                "GLIBC_2.35",
            )
            res = self.run_check(["--check-appimage", str(dup)], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("does not match verified vendor binary", res.stderr)

            # Absolute symlink escaping to host library must fail.
            esc = tmp / "esc"
            self.populate_valid_appdir(esc, ids)
            (esc / "usr" / "lib" / "libwebkit2gtk-4.1.so.0").unlink()
            (esc / "usr" / "lib" / "libwebkit2gtk-4.1.so.0").symlink_to(host_files["wk"])
            res = self.run_check(["--check-appimage", str(esc)], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("absolute host path", res.stderr)

            # Missing runtime SONAME symlink must fail.
            noso = tmp / "noso"
            self.populate_valid_appdir(noso, ids)
            (noso / "usr" / "lib" / "libwebkit2gtk-4.1.so.0").unlink()
            res = self.run_check(["--check-appimage", str(noso)], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("missing required runtime entry libwebkit2gtk-4.1.so.0", res.stderr)

            # Wrong WebKitWebProcess Build ID must fail.
            badproc = tmp / "badproc"
            self.populate_valid_appdir(badproc, ids)
            write_fake_elf(
                badproc / "usr" / "lib" / "webkit2gtk-4.1" / "WebKitWebProcess",
                "bad000000000",
                "GLIBC_2.38",
            )
            res = self.run_check(["--check-appimage", str(badproc)], env)
            self.assertNotEqual(res.returncode, 0)
            self.assertIn("WebKitWebProcess", res.stderr)


if __name__ == "__main__":
    unittest.main()
