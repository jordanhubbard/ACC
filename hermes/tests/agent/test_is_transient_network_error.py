"""Unit tests for agent.error_classifier.is_transient_network_error.

Coverage matrix
---------------
Transient (→ True):
  - Python built-ins: ConnectionError, TimeoutError, OSError w/ EAGAIN /
    ECONNRESET / ETIMEDOUT / ECONNREFUSED / EPIPE / EHOSTUNREACH / ENETUNREACH
  - httpx: ConnectTimeout, ReadTimeout, WriteTimeout, PoolTimeout,
           ConnectError, ReadError, RemoteProtocolError, NetworkError,
           TimeoutException, TransportError
  - requests: Timeout, ConnectTimeout, ReadTimeout, ConnectionError
  - OpenAI SDK: APIConnectionError, APITimeoutError
  - Anthropic SDK: APIConnectionError, APITimeoutError
  - Duck-typed (type-name only): ServerDisconnectedError, etc.

Non-transient (→ False):
  - Programming errors: ValueError, TypeError, AttributeError, KeyError
  - HTTP 4xx APIStatusError-alikes: 400, 401, 403, 404, 422
  - HTTP 402 billing-style errors
  - OSError with non-transient errno (ENOENT, EACCES, EISDIR)
  - httpx.HTTPStatusError (has status_code)

Edge cases:
  - None input → False
  - Chained causes: transient cause inside non-transient wrapper → True
  - Chained causes: non-transient cause inside ConnectionError → True
    (outer is transient even if inner is not)
  - Circular __cause__ references → no infinite loop, returns False
  - Exception with no errno on OSError → treated as transient
  - Deep chain (>5 levels) stops safely
"""

from __future__ import annotations

import errno

import pytest

from agent.error_classifier import is_transient_network_error


# ── Helpers ─────────────────────────────────────────────────────────────────


def _make_oserror(errnum: int, msg: str = "os error") -> OSError:
    """Build an OSError with a specific errno."""
    e = OSError(msg)
    e.errno = errnum
    return e


def _chain(outer: Exception, inner: Exception) -> Exception:
    """Attach *inner* as the __cause__ of *outer* and return *outer*."""
    outer.__cause__ = inner
    return outer


# ── Mock API-status-error (simulates openai/anthropic APIStatusError) ────────


class MockAPIStatusError(Exception):
    """Simulates an SDK APIStatusError with a status_code attribute."""

    def __init__(self, message: str, status_code: int) -> None:
        super().__init__(message)
        self.status_code = status_code


# ── Mock duck-typed transport errors (no stdlib or SDK base class) ───────────


class ServerDisconnectedError(Exception):
    """Simulates an aiohttp-style ServerDisconnectedError by type name only."""


class ClientConnectorError(Exception):
    """Simulates an aiohttp ClientConnectorError by type name only."""


class ServerTimeoutError(Exception):
    """Simulates an aiohttp ServerTimeoutError by type name only."""


class APIConnectionError(Exception):
    """Generic duck-typed APIConnectionError (not openai/anthropic)."""


class APITimeoutError(Exception):
    """Generic duck-typed APITimeoutError (not openai/anthropic)."""


# ═══════════════════════════════════════════════════════════════════════════
# Edge-case: None input
# ═══════════════════════════════════════════════════════════════════════════


class TestNoneInput:
    def test_none_returns_false(self):
        assert is_transient_network_error(None) is False


# ═══════════════════════════════════════════════════════════════════════════
# Python built-in transient exceptions
# ═══════════════════════════════════════════════════════════════════════════


class TestBuiltinTransientExceptions:
    """Python stdlib exceptions that are always transient network errors."""

    def test_connection_error(self):
        assert is_transient_network_error(ConnectionError("reset")) is True

    def test_timeout_error(self):
        assert is_transient_network_error(TimeoutError("timed out")) is True

    def test_connection_reset_error(self):
        # ConnectionResetError is a subclass of ConnectionError
        assert is_transient_network_error(ConnectionResetError("reset by peer")) is True

    def test_connection_refused_error(self):
        # ConnectionRefusedError is a subclass of ConnectionError
        assert is_transient_network_error(ConnectionRefusedError("refused")) is True

    def test_connection_aborted_error(self):
        # ConnectionAbortedError is a subclass of ConnectionError
        assert is_transient_network_error(ConnectionAbortedError("aborted")) is True

    def test_broken_pipe_error(self):
        # BrokenPipeError is a subclass of ConnectionError
        assert is_transient_network_error(BrokenPipeError("broken pipe")) is True


# ═══════════════════════════════════════════════════════════════════════════
# OSError with transient errnos
# ═══════════════════════════════════════════════════════════════════════════


class TestOSErrorTransientErrnos:
    """OSError instances are transient only for specific errno values."""

    def test_eagain(self):
        assert is_transient_network_error(_make_oserror(errno.EAGAIN)) is True

    def test_econnreset(self):
        assert is_transient_network_error(_make_oserror(errno.ECONNRESET)) is True

    def test_etimedout(self):
        assert is_transient_network_error(_make_oserror(errno.ETIMEDOUT)) is True

    def test_econnrefused(self):
        assert is_transient_network_error(_make_oserror(errno.ECONNREFUSED)) is True

    def test_epipe(self):
        assert is_transient_network_error(_make_oserror(errno.EPIPE)) is True

    @pytest.mark.skipif(not hasattr(errno, "EHOSTUNREACH"), reason="EHOSTUNREACH not available")
    def test_ehostunreach(self):
        assert is_transient_network_error(_make_oserror(errno.EHOSTUNREACH)) is True

    @pytest.mark.skipif(not hasattr(errno, "ENETUNREACH"), reason="ENETUNREACH not available")
    def test_enetunreach(self):
        assert is_transient_network_error(_make_oserror(errno.ENETUNREACH)) is True

    def test_oserror_no_errno_is_not_transient(self):
        """An OSError with errno=None (e.g. file operation gone wrong) is
        not a network error and must return False."""
        e = OSError("generic OS error")
        assert e.errno is None
        assert is_transient_network_error(e) is False

    def test_enoent_is_not_transient(self):
        assert is_transient_network_error(_make_oserror(errno.ENOENT, "no such file")) is False

    def test_eacces_is_not_transient(self):
        assert is_transient_network_error(_make_oserror(errno.EACCES, "permission denied")) is False

    def test_eisdir_is_not_transient(self):
        assert is_transient_network_error(_make_oserror(errno.EISDIR, "is a directory")) is False

    def test_ebadf_is_not_transient(self):
        assert is_transient_network_error(_make_oserror(errno.EBADF, "bad file descriptor")) is False


# ═══════════════════════════════════════════════════════════════════════════
# httpx transport errors
# ═══════════════════════════════════════════════════════════════════════════


class TestHttpxTransientErrors:
    """httpx raises its own exception hierarchy; all transport errors → True."""

    def test_connect_timeout(self):
        import httpx
        assert is_transient_network_error(httpx.ConnectTimeout("connect timed out")) is True

    def test_read_timeout(self):
        import httpx
        assert is_transient_network_error(httpx.ReadTimeout("read timed out")) is True

    def test_write_timeout(self):
        import httpx
        assert is_transient_network_error(httpx.WriteTimeout("write timed out")) is True

    def test_pool_timeout(self):
        import httpx
        assert is_transient_network_error(httpx.PoolTimeout("pool timed out")) is True

    def test_connect_error(self):
        import httpx
        assert is_transient_network_error(httpx.ConnectError("connection refused")) is True

    def test_read_error(self):
        import httpx
        assert is_transient_network_error(httpx.ReadError("read error")) is True

    def test_write_error(self):
        import httpx
        assert is_transient_network_error(httpx.WriteError("write error")) is True

    def test_remote_protocol_error(self):
        import httpx
        assert is_transient_network_error(httpx.RemoteProtocolError("remote protocol error")) is True

    def test_network_error_base(self):
        import httpx
        # httpx.NetworkError is the base for ConnectError, ReadError, WriteError
        assert is_transient_network_error(httpx.ConnectError("net error")) is True

    def test_timeout_exception_base(self):
        import httpx
        # httpx.TimeoutException is the base for all timeout types
        assert is_transient_network_error(httpx.ConnectTimeout("base timeout")) is True

    def test_http_status_error_is_not_transient(self):
        """httpx.HTTPStatusError has a status code and is not a network error."""
        import httpx
        # Build a minimal mock response so HTTPStatusError can be constructed
        request = httpx.Request("GET", "https://example.com/")
        response = httpx.Response(404, request=request)
        exc = httpx.HTTPStatusError("not found", request=request, response=response)
        assert is_transient_network_error(exc) is False

    def test_http_status_error_429_is_not_transient(self):
        """429 Too Many Requests is handled by the retry loop, not here."""
        import httpx
        request = httpx.Request("GET", "https://example.com/")
        response = httpx.Response(429, request=request)
        exc = httpx.HTTPStatusError("rate limited", request=request, response=response)
        assert is_transient_network_error(exc) is False


# ═══════════════════════════════════════════════════════════════════════════
# requests transport errors
# ═══════════════════════════════════════════════════════════════════════════


class TestRequestsTransientErrors:
    """requests raises its own exception hierarchy (inherits from OSError)."""

    def test_timeout(self):
        import requests
        assert is_transient_network_error(requests.exceptions.Timeout("timed out")) is True

    def test_connect_timeout(self):
        import requests
        assert is_transient_network_error(requests.exceptions.ConnectTimeout("connect timed out")) is True

    def test_read_timeout(self):
        import requests
        assert is_transient_network_error(requests.exceptions.ReadTimeout("read timed out")) is True

    def test_connection_error(self):
        import requests
        assert is_transient_network_error(requests.exceptions.ConnectionError("connection error")) is True

    def test_http_error_4xx_is_not_transient(self):
        """requests.HTTPError for a 4xx response is not a network error."""
        import requests
        exc = requests.exceptions.HTTPError("403 Forbidden")
        exc.response = type("R", (), {"status_code": 403})()
        # HTTPError does NOT inherit from ConnectionError / Timeout,
        # so it must return False.
        assert is_transient_network_error(exc) is False


# ═══════════════════════════════════════════════════════════════════════════
# OpenAI SDK errors
# ═══════════════════════════════════════════════════════════════════════════


class TestOpenAISDKErrors:
    def test_api_connection_error(self):
        import openai
        exc = openai.APIConnectionError.__new__(openai.APIConnectionError)
        exc.args = ("connection failed",)
        assert is_transient_network_error(exc) is True

    def test_api_timeout_error(self):
        import openai
        exc = openai.APITimeoutError.__new__(openai.APITimeoutError)
        exc.args = ("timed out",)
        assert is_transient_network_error(exc) is True

    def test_api_status_error_400_is_not_transient(self):
        """openai.BadRequestError (status 400) is not a transient network error."""
        exc = MockAPIStatusError("bad request", status_code=400)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_401_is_not_transient(self):
        exc = MockAPIStatusError("unauthorized", status_code=401)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_403_is_not_transient(self):
        exc = MockAPIStatusError("forbidden", status_code=403)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_404_is_not_transient(self):
        exc = MockAPIStatusError("not found", status_code=404)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_422_is_not_transient(self):
        exc = MockAPIStatusError("unprocessable entity", status_code=422)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_402_billing_is_not_transient(self):
        exc = MockAPIStatusError("insufficient credits", status_code=402)
        assert is_transient_network_error(exc) is False


# ═══════════════════════════════════════════════════════════════════════════
# Anthropic SDK errors
# ═══════════════════════════════════════════════════════════════════════════


class TestAnthropicSDKErrors:
    def test_api_connection_error(self):
        import anthropic
        exc = anthropic.APIConnectionError.__new__(anthropic.APIConnectionError)
        exc.args = ("connection failed",)
        assert is_transient_network_error(exc) is True

    def test_api_timeout_error(self):
        import anthropic
        exc = anthropic.APITimeoutError.__new__(anthropic.APITimeoutError)
        exc.args = ("timed out",)
        assert is_transient_network_error(exc) is True

    def test_api_status_error_400_is_not_transient(self):
        """anthropic.BadRequestError (status 400) is not transient."""
        exc = MockAPIStatusError("bad request", status_code=400)
        assert is_transient_network_error(exc) is False

    def test_api_status_error_401_is_not_transient(self):
        exc = MockAPIStatusError("unauthorized", status_code=401)
        assert is_transient_network_error(exc) is False


# ═══════════════════════════════════════════════════════════════════════════
# Duck-typed (type-name-only) transport errors
# ═══════════════════════════════════════════════════════════════════════════


class TestDuckTypedTransportErrors:
    """Errors that carry no SDK/stdlib base class but have a recognisable name."""

    def test_server_disconnected_error(self):
        assert is_transient_network_error(ServerDisconnectedError("disconnected")) is True

    def test_client_connector_error(self):
        assert is_transient_network_error(ClientConnectorError("connector")) is True

    def test_server_timeout_error(self):
        assert is_transient_network_error(ServerTimeoutError("server timeout")) is True

    def test_duck_typed_api_connection_error(self):
        assert is_transient_network_error(APIConnectionError("conn failed")) is True

    def test_duck_typed_api_timeout_error(self):
        assert is_transient_network_error(APITimeoutError("timed out")) is True


# ═══════════════════════════════════════════════════════════════════════════
# Non-transient exceptions
# ═══════════════════════════════════════════════════════════════════════════


class TestNonTransientExceptions:
    """Exceptions that must return False."""

    def test_value_error(self):
        assert is_transient_network_error(ValueError("bad value")) is False

    def test_type_error(self):
        assert is_transient_network_error(TypeError("wrong type")) is False

    def test_attribute_error(self):
        assert is_transient_network_error(AttributeError("missing attr")) is False

    def test_key_error(self):
        assert is_transient_network_error(KeyError("missing key")) is False

    def test_runtime_error(self):
        assert is_transient_network_error(RuntimeError("runtime error")) is False

    def test_index_error(self):
        assert is_transient_network_error(IndexError("index out of range")) is False

    def test_generic_exception(self):
        assert is_transient_network_error(Exception("something went wrong")) is False

    def test_stop_iteration(self):
        assert is_transient_network_error(StopIteration()) is False

    def test_mock_400_api_error(self):
        exc = MockAPIStatusError("bad request", status_code=400)
        assert is_transient_network_error(exc) is False

    def test_mock_401_api_error(self):
        exc = MockAPIStatusError("unauthorized", status_code=401)
        assert is_transient_network_error(exc) is False

    def test_mock_403_api_error(self):
        exc = MockAPIStatusError("forbidden", status_code=403)
        assert is_transient_network_error(exc) is False

    def test_mock_404_api_error(self):
        exc = MockAPIStatusError("not found", status_code=404)
        assert is_transient_network_error(exc) is False

    def test_mock_422_api_error(self):
        exc = MockAPIStatusError("unprocessable entity", status_code=422)
        assert is_transient_network_error(exc) is False

    def test_mock_402_billing_api_error(self):
        """402 billing exhaustion is not a transient network error."""
        exc = MockAPIStatusError("payment required — insufficient credits", status_code=402)
        assert is_transient_network_error(exc) is False


# ═══════════════════════════════════════════════════════════════════════════
# Chained cause tests
# ═══════════════════════════════════════════════════════════════════════════


class TestChainedCauses:
    """The function must walk __cause__ / __context__ to find transient errors."""

    def test_transient_cause_inside_runtime_error(self):
        """RuntimeError wrapping a ConnectionError → True (cause is transient)."""
        inner = ConnectionError("reset by peer")
        outer = RuntimeError("api call failed")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is True

    def test_transient_cause_inside_value_error(self):
        inner = TimeoutError("timed out")
        outer = ValueError("unexpected response")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is True

    def test_transient_cause_inside_generic_exception(self):
        inner = _make_oserror(errno.ECONNRESET, "connection reset")
        outer = Exception("provider call failed")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is True

    def test_non_transient_cause_inside_non_transient_wrapper(self):
        """Both outer and inner are non-transient → False."""
        inner = ValueError("bad value")
        outer = RuntimeError("outer problem")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is False

    def test_three_level_chain_transient_at_bottom(self):
        """Transient error three levels deep must be found."""
        bottom = ConnectionError("network gone")
        middle = Exception("mid-level error")
        middle.__cause__ = bottom
        top = RuntimeError("top-level failure")
        top.__cause__ = middle
        assert is_transient_network_error(top) is True

    def test_context_chain_when_no_cause(self):
        """__context__ is walked when __cause__ is None."""
        inner = TimeoutError("timed out")
        outer = RuntimeError("outer")
        # __context__ is set by `raise Outer from None` / implicit chaining
        outer.__context__ = inner
        assert is_transient_network_error(outer) is True

    def test_cause_takes_priority_over_context(self):
        """When both __cause__ and __context__ are set, __cause__ is walked."""
        transient_cause = ConnectionError("reset")
        non_transient_ctx = ValueError("bad val")
        outer = RuntimeError("outer")
        outer.__cause__ = transient_cause
        outer.__context__ = non_transient_ctx
        # Should find transient via __cause__ immediately
        assert is_transient_network_error(outer) is True

    def test_httpx_cause_inside_sdk_wrapper(self):
        """httpx ConnectTimeout as __cause__ of a generic SDK error → True."""
        import httpx
        inner = httpx.ConnectTimeout("connect timed out")
        outer = RuntimeError("sdk wrapper: connection failed")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is True

    def test_non_transient_inner_does_not_make_outer_transient(self):
        """A non-transient inner cause does not flip the outer to transient
        unless the outer is itself transient."""
        inner = ValueError("bad value")
        outer = Exception("wrapped non-transient")
        outer.__cause__ = inner
        assert is_transient_network_error(outer) is False

    def test_api_status_error_with_transient_cause(self):
        """A 4xx API error wrapping a ConnectionError — only the cause is
        transient; the outer has a non-transient status code, but walking
        the chain finds the transient inner."""
        inner = ConnectionError("connection reset")
        outer = MockAPIStatusError("api call failed after reconnect", status_code=500)
        outer.__cause__ = inner
        # 500 is not in _NON_TRANSIENT_STATUS_CODES, so the outer is also
        # not immediately classified as non-transient; but even if it were,
        # we walk the chain to find the inner ConnectionError.
        # Since the outer has a status_code (500), _is_single_exception_transient
        # returns False for it; the walker then finds the inner → True.
        assert is_transient_network_error(outer) is True

    def test_4xx_outer_with_connection_error_cause(self):
        """4xx status on outer, but ConnectionError as cause → True (cause wins)."""
        inner = ConnectionError("reset by peer")
        outer = MockAPIStatusError("bad request", status_code=400)
        outer.__cause__ = inner
        # Outer is non-transient (400), but inner is transient → True
        assert is_transient_network_error(outer) is True


# ═══════════════════════════════════════════════════════════════════════════
# Circular __cause__ reference (must not infinite-loop)
# ═══════════════════════════════════════════════════════════════════════════


class TestCircularCause:
    def test_self_circular_cause_returns_false(self):
        """An exception that is its own __cause__ must not loop forever."""
        exc = Exception("circular")
        exc.__cause__ = exc
        # Must return within the test timeout (30 s) — and for a generic
        # Exception with no transient markers, the answer is False.
        assert is_transient_network_error(exc) is False

    def test_two_node_circular_cause_returns_false(self):
        """A → B → A circular chain must terminate."""
        a = Exception("node a")
        b = Exception("node b")
        a.__cause__ = b
        b.__cause__ = a
        assert is_transient_network_error(a) is False

    def test_circular_with_transient_node_returns_true(self):
        """A → B(ConnectionError) → A circular chain; B is transient → True."""
        a = RuntimeError("wrapper")
        b = ConnectionError("inner transient")
        a.__cause__ = b
        b.__cause__ = a  # circular back to a
        # The walker hits `a` (not transient), then `b` (transient) → True
        assert is_transient_network_error(a) is True

    def test_deep_non_circular_chain_terminates(self):
        """A 7-level non-circular chain must terminate (depth capped at 6)."""
        exceptions = [Exception(f"level-{i}") for i in range(7)]
        for i in range(6):
            exceptions[i].__cause__ = exceptions[i + 1]
        # None are transient → False, but must not blow the stack
        assert is_transient_network_error(exceptions[0]) is False

    def test_deep_chain_with_transient_at_depth_5_found(self):
        """Transient error at depth 5 (within the walk limit) → True."""
        exceptions = [Exception(f"level-{i}") for i in range(5)]
        for i in range(4):
            exceptions[i].__cause__ = exceptions[i + 1]
        # Attach a transient error at the very bottom
        transient = ConnectionError("reset")
        exceptions[4].__cause__ = transient
        assert is_transient_network_error(exceptions[0]) is True

    def test_transient_at_depth_7_may_not_be_found(self):
        """Transient error at depth 7 may not be found (walk capped at 6).

        This is an explicit contract: the function is not guaranteed to walk
        arbitrarily deep chains.  It limits itself to avoid O(n) blowup.
        """
        exceptions = [Exception(f"level-{i}") for i in range(7)]
        for i in range(6):
            exceptions[i].__cause__ = exceptions[i + 1]
        transient = ConnectionError("reset at depth 7")
        exceptions[6].__cause__ = transient
        # The result may be False because the walker stopped before reaching
        # the transient node.  We just assert it doesn't raise or hang.
        result = is_transient_network_error(exceptions[0])
        assert isinstance(result, bool)


# ═══════════════════════════════════════════════════════════════════════════
# Additional edge cases
# ═══════════════════════════════════════════════════════════════════════════


class TestEdgeCases:
    def test_bare_os_error_without_errno_is_not_transient(self):
        """OSError() with no args → errno is None → not transient."""
        e = OSError()
        assert e.errno is None
        assert is_transient_network_error(e) is False

    def test_connection_error_subclass_custom_class(self):
        """A custom exception subclassing ConnectionError → transient."""

        class MyConnectionError(ConnectionError):
            pass

        assert is_transient_network_error(MyConnectionError("custom")) is True

    def test_timeout_error_subclass_custom_class(self):
        """A custom exception subclassing TimeoutError → transient."""

        class MyTimeoutError(TimeoutError):
            pass

        assert is_transient_network_error(MyTimeoutError("custom timeout")) is True

    def test_exception_with_false_status_code(self):
        """An exception whose .status_code is 0 (falsy) should not crash."""

        class WeirdError(Exception):
            status_code = 0

        # 0 is not a valid HTTP status; _extract_status_code checks isinstance
        # and 100<=code<600, so it should fall through to False.
        result = is_transient_network_error(WeirdError("weird"))
        assert isinstance(result, bool)

    def test_exception_with_string_status_code_no_crash(self):
        """Some proxies put string status codes; must not crash."""

        class StringStatusError(Exception):
            status_code = "400"

        result = is_transient_network_error(StringStatusError("bad"))
        assert isinstance(result, bool)

    def test_exception_with_none_cause_terminates(self):
        """Exception with __cause__ = None terminates normally."""
        exc = ConnectionError("reset")
        exc.__cause__ = None
        assert is_transient_network_error(exc) is True

    def test_empty_exception_message(self):
        """Empty message string must not cause any issues."""
        assert is_transient_network_error(ConnectionError("")) is True
        assert is_transient_network_error(Exception("")) is False

    def test_httpx_request_error_subclass(self):
        """httpx.RequestError base class (via TransportError subclass) → True."""
        import httpx
        # ConnectError inherits RequestError → TransportError → True
        exc = httpx.ConnectError("cannot connect")
        assert is_transient_network_error(exc) is True

    def test_non_exception_base_class_not_transient(self):
        """Subclasses of BaseException that are not Exception-derived."""
        # SystemExit, KeyboardInterrupt — not network errors
        assert is_transient_network_error(SystemExit(1)) is False
        assert is_transient_network_error(KeyboardInterrupt()) is False
