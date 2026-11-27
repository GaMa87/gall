"""Opaque-box validation helpers and self-audit checks."""

from __future__ import annotations

import configparser
import json
import shutil
import subprocess
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from .env_probe import EnvProbe
    from .sandbox import SandboxEnv

# Constructed dynamically so this module passes its own static opaque-box scan.
_FORBIDDEN_SUBSTRINGS: tuple[str, ...] = (
    "crates" + "/",
    "src-tauri" + "/",
    "ui/" + "src/",
)


def read_mock_calls(sb: "SandboxEnv", cmd: str | None = None) -> list[dict[str, Any]]:
    if not sb.mock_log.exists():
        return []
    calls: list[dict[str, Any]] = []
    for line in sb.mock_log.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        rec = json.loads(line)
        if cmd is None or rec.get("cmd") == cmd:
            calls.append(rec)
    return calls


def assert_desktop_valid(
    path: Path,
    env_probe: "EnvProbe | None" = None,
    warn_kde: bool = False,
) -> configparser.RawConfigParser:
    if not path.is_file():
        raise AssertionError(f"Desktop entry file does not exist: {path}")

    validator = shutil.which("desktop-file-validate")
    if validator:
        cmd = [validator]
        if warn_kde:
            cmd.append("--warn-kde")
        cmd.append(str(path))
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=10, check=False)
        if res.returncode != 0:
            msg = (res.stdout + "\n" + res.stderr).strip()
            raise AssertionError(f"desktop-file-validate failed for {path}: {msg}")

    parser = configparser.RawConfigParser(interpolation=None)
    parser.optionxform = str  # preserve case sensitivity of desktop entry keys
    parser.read(path, encoding="utf-8")
    if not parser.has_section("Desktop Entry"):
        raise AssertionError(f"Missing [Desktop Entry] section in {path}")
    return parser


def assert_xml_valid(path: Path) -> ET.Element:
    if not path.is_file():
        raise AssertionError(f"XML file does not exist: {path}")

    xmllint = shutil.which("xmllint")
    if xmllint:
        res = subprocess.run(
            [xmllint, "--noout", str(path)],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if res.returncode != 0:
            msg = (res.stdout + "\n" + res.stderr).strip()
            raise AssertionError(f"xmllint failed for {path}: {msg}")

    try:
        tree = ET.parse(path)
    except ET.ParseError as exc:
        raise AssertionError(f"XML parse error in {path}: {exc}") from exc
    return tree.getroot()


def parse_toml(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise AssertionError(f"TOML file does not exist: {path}")
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise AssertionError(f"Invalid TOML in {path}: {exc}") from exc


def assert_no_forbidden_refs(e2e_dir: Path) -> list[str]:
    e2e_dir = Path(e2e_dir).resolve()
    if not e2e_dir.is_dir():
        raise AssertionError(f"E2E directory not found: {e2e_dir}")

    audited: list[str] = []
    violations: list[str] = []

    for py_file in sorted(e2e_dir.rglob("*.py")):
        if "__pycache__" in py_file.parts:
            continue
        rel = str(py_file.relative_to(e2e_dir))
        audited.append(rel)
        text = py_file.read_text(encoding="utf-8", errors="replace")
        for idx, line in enumerate(text.splitlines(), start=1):
            for pat in _FORBIDDEN_SUBSTRINGS:
                if pat in line:
                    violations.append(f"{rel}:{idx}: forbidden internal reference {pat!r}")

    mocks_py = e2e_dir / "harness" / "mocks.py"
    if not mocks_py.is_file():
        violations.append("harness/mocks.py missing")
    else:
        mocks_text = mocks_py.read_text(encoding="utf-8", errors="replace")
        if "GALL_E2E_MOCK_LOG" not in mocks_text or "_log_call" not in mocks_text:
            violations.append("harness/mocks.py does not enforce GALL_E2E_MOCK_LOG recording")

    if violations:
        raise AssertionError("Opaque-box self-audit failed:\n- " + "\n- ".join(violations))
    return audited
