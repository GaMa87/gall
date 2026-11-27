"""Shared E2E harness modules."""

from .assertions import (
    assert_desktop_valid,
    assert_no_forbidden_refs,
    assert_xml_valid,
    parse_toml,
    read_mock_calls,
)
from .env_probe import EnvProbe, probe_env
from .mocks import HyprSocket2Server, MockRegistry
from .sandbox import SandboxEnv
from .types import E2ECase, E2EResult, RunContext, Status

__all__ = [
    "E2ECase",
    "E2EResult",
    "EnvProbe",
    "HyprSocket2Server",
    "MockRegistry",
    "RunContext",
    "SandboxEnv",
    "Status",
    "assert_desktop_valid",
    "assert_no_forbidden_refs",
    "assert_xml_valid",
    "parse_toml",
    "probe_env",
    "read_mock_calls",
]
