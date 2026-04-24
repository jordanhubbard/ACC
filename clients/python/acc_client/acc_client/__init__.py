"""HTTP client for the ACC fleet API.

Public API:
    Client          — synchronous client; the common case
    ApiError        — HTTP 4xx/5xx wrapped in a typed exception
    Conflict        — HTTP 409 (claim race etc.)
    Locked          — HTTP 423 (task blocked by dependencies)
    NotFound        — HTTP 404
    Unauthorized    — HTTP 401
    AtCapacity      — HTTP 429

Mirrors the Rust `acc-client` crate in shape and behavior.
"""

from ._errors import (
    AtCapacity,
    ApiError,
    Conflict,
    Locked,
    NotFound,
    NoToken,
    Unauthorized,
)
from .client import Client

__all__ = [
    "Client",
    "ApiError",
    "Conflict",
    "Locked",
    "NotFound",
    "Unauthorized",
    "AtCapacity",
    "NoToken",
]

__version__ = "0.1.0"
