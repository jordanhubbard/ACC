"""Exception hierarchy mirroring the Rust acc-client Error enum.

Status-specific subclasses exist for the codes callers actually branch on
(401/404/409/423/429); other statuses surface as ApiError.
"""
from __future__ import annotations

from typing import Any


class NoToken(RuntimeError):
    """No API token found in environment or ~/.acc/.env."""


class ApiError(RuntimeError):
    """HTTP error from the ACC server.

    `code` is the server's error code string (from the JSON body) if
    present, else a synthetic `http_<status>` marker. `extra` carries any
    endpoint-specific fields the server included alongside the code
    (e.g. `pending` on 423, `active`/`max` on 429).
    """

    status: int
    code: str
    extra: dict[str, Any]

    def __init__(self, status: int, body: dict[str, Any] | None = None):
        body = body or {}
        self.status = status
        self.code = body.get("error") or f"http_{status}"
        message = body.get("message") or self.code
        self.extra = {k: v for k, v in body.items() if k not in ("error", "message")}
        super().__init__(f"HTTP {status}: {message}")


class Unauthorized(ApiError):
    """HTTP 401."""


class NotFound(ApiError):
    """HTTP 404."""


class Conflict(ApiError):
    """HTTP 409 (resource conflict — e.g. task already claimed)."""


class Locked(ApiError):
    """HTTP 423 (resource blocked by unfulfilled dependencies)."""


class AtCapacity(ApiError):
    """HTTP 429 (agent at concurrent-work capacity)."""


_STATUS_TO_CLASS: dict[int, type[ApiError]] = {
    401: Unauthorized,
    404: NotFound,
    409: Conflict,
    423: Locked,
    429: AtCapacity,
}


def from_response(status: int, body: dict[str, Any] | None) -> ApiError:
    """Construct the most-specific ApiError subclass for this status."""
    cls = _STATUS_TO_CLASS.get(status, ApiError)
    return cls(status, body)
