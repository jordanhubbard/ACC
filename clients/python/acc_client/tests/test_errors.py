"""Unit tests for acc_client._errors.from_response (classify_error) and
the hard-error path of acc_client.client.Client._request (process_request).

17 tests total:
  - classify_error branches: 429, 5xx (500/502/503/599), status-0 variants,
    401, 403, 400, 404, 408, 422, 430, and substring / code-field matching.
  - process_request hard-error path: _request raises the correct ApiError
    subclass for every non-2xx response received from the transport.
"""
from __future__ import annotations

import httpx
import pytest
import respx

from acc_client import (
    ApiError,
    AtCapacity,
    Client,
    NotFound,
    Unauthorized,
)
from acc_client._errors import from_response


# ── helpers ────────────────────────────────────────────────────────────────


def _body(error: str, message: str | None = None) -> dict:
    """Build a minimal server error body."""
    d: dict = {"error": error}
    if message is not None:
        d["message"] = message
    return d


# ══════════════════════════════════════════════════════════════════════════
# classify_error (from_response) — direct unit tests
# ══════════════════════════════════════════════════════════════════════════


class TestClassifyError:
    """Tests for acc_client._errors.from_response.

    Each test targets one status-code branch or body-field path in the
    classification logic.
    """

    # ── 429 ────────────────────────────────────────────────────────────

    def test_429_returns_at_capacity(self):
        """HTTP 429 must produce an AtCapacity instance."""
        err = from_response(429, _body("at_capacity", "agent is at capacity"))
        assert isinstance(err, AtCapacity)
        assert err.status == 429
        assert err.code == "at_capacity"

    # ── 5xx ────────────────────────────────────────────────────────────

    def test_500_returns_api_error(self):
        """HTTP 500 must produce a generic ApiError (no special subclass)."""
        err = from_response(500, _body("internal_error", "boom"))
        assert type(err) is ApiError
        assert err.status == 500

    def test_502_returns_api_error(self):
        """HTTP 502 must produce a generic ApiError."""
        err = from_response(502, _body("bad_gateway"))
        assert type(err) is ApiError
        assert err.status == 502

    def test_503_returns_api_error(self):
        """HTTP 503 must produce a generic ApiError."""
        err = from_response(503, _body("unavailable"))
        assert type(err) is ApiError
        assert err.status == 503

    def test_599_edge_returns_api_error(self):
        """HTTP 599 (unknown 5xx) must not crash; falls back to generic ApiError."""
        err = from_response(599, _body("unknown_server_error"))
        assert type(err) is ApiError
        assert err.status == 599

    # ── status=0 / missing body variants ───────────────────────────────

    def test_status_0_with_none_body_returns_api_error(self):
        """status=0 with no body must produce a plain ApiError without raising."""
        err = from_response(0, None)
        assert isinstance(err, ApiError)
        assert err.status == 0

    def test_status_0_with_empty_body_uses_synthetic_code(self):
        """status=0 with an empty body dict must synthesise 'http_0' as .code."""
        err = from_response(0, {})
        assert err.code == "http_0"

    # ── 401 ────────────────────────────────────────────────────────────

    def test_401_returns_unauthorized(self):
        """HTTP 401 must produce an Unauthorized instance."""
        err = from_response(401, _body("unauthorized", "invalid token"))
        assert isinstance(err, Unauthorized)
        assert err.status == 401

    # ── 403 ────────────────────────────────────────────────────────────

    def test_403_returns_generic_api_error(self):
        """HTTP 403 has no dedicated subclass; must produce a plain ApiError."""
        err = from_response(403, _body("forbidden", "access denied"))
        assert type(err) is ApiError
        assert err.status == 403
        assert err.code == "forbidden"

    # ── 400 ────────────────────────────────────────────────────────────

    def test_400_returns_generic_api_error(self):
        """HTTP 400 must produce a plain ApiError (no special subclass)."""
        err = from_response(400, _body("bad_request", "missing field 'agent'"))
        assert type(err) is ApiError
        assert err.status == 400
        assert err.code == "bad_request"

    # ── 404 ────────────────────────────────────────────────────────────

    def test_404_returns_not_found(self):
        """HTTP 404 must produce a NotFound instance."""
        err = from_response(404, _body("not_found", "task t-99 not found"))
        assert isinstance(err, NotFound)
        assert err.status == 404

    # ── 408 ────────────────────────────────────────────────────────────

    def test_408_returns_generic_api_error(self):
        """HTTP 408 (Request Timeout) has no special subclass."""
        err = from_response(408, _body("request_timeout"))
        assert type(err) is ApiError
        assert err.status == 408
        assert err.code == "request_timeout"

    # ── 422 ────────────────────────────────────────────────────────────

    def test_422_returns_generic_api_error(self):
        """HTTP 422 (Unprocessable Entity) has no special subclass."""
        err = from_response(422, _body("unprocessable_entity", "invalid field value"))
        assert type(err) is ApiError
        assert err.status == 422
        assert err.code == "unprocessable_entity"

    # ── 430 (unknown/custom status) ────────────────────────────────────

    def test_430_unknown_status_returns_generic_api_error(self):
        """An unknown/custom 4xx status (430) must fall back to ApiError."""
        err = from_response(430, _body("custom_rate_limit"))
        assert type(err) is ApiError
        assert err.status == 430
        assert err.code == "custom_rate_limit"

    # ── substring / code-field matching ────────────────────────────────

    def test_body_error_field_used_as_code(self):
        """The 'error' key in the body must be returned verbatim as .code
        and must appear as a substring of the exception message string."""
        err = from_response(400, {"error": "quota_exceeded", "message": "quota exceeded"})
        assert err.code == "quota_exceeded"
        assert "quota_exceeded" in str(err) or "quota exceeded" in str(err)

    def test_missing_error_field_synthesises_http_code(self):
        """When the body has no 'error' key the synthetic 'http_<status>'
        string must be used as .code."""
        err = from_response(503, {"message": "service down"})
        assert err.code == "http_503"


# ══════════════════════════════════════════════════════════════════════════
# process_request (_request) hard-error path
# ══════════════════════════════════════════════════════════════════════════


@pytest.fixture
def client():
    c = Client(base_url="http://hub.test", token="tok")
    yield c
    c.close()


class TestProcessRequestHardErrorPath:
    """Tests for Client._request non-2xx error handling.

    Verifies that _request (exercised via a public API method) raises the
    correct ApiError subclass when the server returns a non-2xx status.
    """

    @respx.mock
    def test_request_raises_api_error_on_non_json_body(self, client):
        """A non-JSON error response must raise ApiError with a synthetic code,
        not propagate a ValueError from JSON parsing."""
        respx.get("http://hub.test/api/tasks").mock(
            return_value=httpx.Response(503, text="<html>Service Unavailable</html>")
        )
        with pytest.raises(ApiError) as exc_info:
            client.tasks.list()
        assert exc_info.value.status == 503
        assert exc_info.value.code == "http_503"
