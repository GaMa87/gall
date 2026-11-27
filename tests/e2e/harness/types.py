"""Core types for the Gall opaque-box E2E test harness."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable

if TYPE_CHECKING:
    from .env_probe import EnvProbe


class Status(str, Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    PENDING_IMPL = "PENDING_IMPL"
    ENV_UNAVAILABLE = "ENV_UNAVAILABLE"


@dataclass
class E2EResult:
    status: Status
    message: str = ""
    details: dict[str, Any] | None = None
    duration_ms: float = 0.0

    def to_dict(self) -> dict[str, Any]:
        return {
            "status": self.status.value,
            "message": self.message,
            "details": self.details,
            "duration_ms": round(self.duration_ms, 2),
        }


def _unimplemented_case(_ctx: "RunContext") -> E2EResult:
    return E2EResult(Status.PENDING_IMPL, "Test case function is not bound")


@dataclass
class E2ECase:
    id: str
    tier: int
    features: tuple[str, ...]
    title: str
    requires_binary: bool = False
    requires_gui: bool = False
    requires_dbus: bool = False
    fn: Callable[["RunContext"], E2EResult] = _unimplemented_case


@dataclass
class RunContext:
    repo_root: Path
    binary_path: Path | None
    env_probe: "EnvProbe"
    verbose: bool = False
