"""Synchronous ACC API client.

Mirrors the shape of the Rust `acc_client::Client`. Resource APIs are
attached as attributes (``client.tasks``, ``client.memory``, ...) so the
mental model matches.
"""
from __future__ import annotations

from typing import Any

import httpx

from ._auth import resolve_base_url, resolve_token
from ._errors import from_response


# Default per-request timeout. Individual calls that need more (streaming,
# long server-side ops) can override this by passing a per-call client.
DEFAULT_TIMEOUT = 30.0


class Client:
    """Synchronous ACC client.

    Obtain one per process (or per thread) and share it — the underlying
    ``httpx.Client`` pools connections. Always call ``close()`` when done,
    or use the client as a context manager.

    The client is cheap to construct but not cheap to leak — ``close()``
    or ``with`` ensures the underlying HTTP pool shuts down cleanly.
    """

    def __init__(
        self,
        *,
        base_url: str | None = None,
        token: str | None = None,
        timeout: float = DEFAULT_TIMEOUT,
    ):
        self._base = resolve_base_url(base_url)
        self._token = resolve_token(token)
        self._http = httpx.Client(
            base_url=self._base,
            headers={"Authorization": f"Bearer {self._token}"},
            timeout=timeout,
        )

        self.tasks = _TasksApi(self)
        self.queue = _QueueApi(self)
        self.items = _ItemsApi(self)
        self.bus = _BusApi(self)
        self.memory = _MemoryApi(self)
        self.agents = _AgentsApi(self)

    @classmethod
    def from_env(cls, *, timeout: float = DEFAULT_TIMEOUT) -> "Client":
        """Construct a client using env/dotenv for base URL and token."""
        return cls(timeout=timeout)

    @property
    def base_url(self) -> str:
        return self._base

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    # ── internal helpers shared by sub-APIs ────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, Any] | None = None,
        json: Any | None = None,
    ) -> Any:
        """Issue a request, decode JSON, raise on non-2xx."""
        resp = self._http.request(method, path, params=params, json=json)
        if not (200 <= resp.status_code < 300):
            body: dict[str, Any] | None = None
            try:
                body = resp.json()
                if not isinstance(body, dict):
                    body = {"error": f"http_{resp.status_code}", "message": str(body)}
            except ValueError:
                body = {"error": f"http_{resp.status_code}", "message": resp.text}
            raise from_response(resp.status_code, body)
        if not resp.content:
            return None
        try:
            return resp.json()
        except ValueError:
            return resp.text


# ── Sub-API classes ────────────────────────────────────────────────────


class _SubApi:
    def __init__(self, client: Client):
        self._c = client


class _TasksApi(_SubApi):
    """Operations on ``/api/tasks``."""

    def list(
        self,
        *,
        status: str | None = None,
        task_type: str | None = None,
        project: str | None = None,
        agent: str | None = None,
        limit: int | None = None,
    ) -> list[dict[str, Any]]:
        params: dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        if task_type is not None:
            params["task_type"] = task_type
        if project is not None:
            params["project"] = project
        if agent is not None:
            params["agent"] = agent
        if limit is not None:
            params["limit"] = limit
        resp = self._c._request("GET", "/api/tasks", params=params) or {}
        return resp.get("tasks", [])

    def get(self, task_id: str) -> dict[str, Any]:
        resp = self._c._request("GET", f"/api/tasks/{task_id}") or {}
        return resp.get("task", resp)

    def create(self, **fields: Any) -> dict[str, Any]:
        resp = self._c._request("POST", "/api/tasks", json=fields) or {}
        return resp.get("task", resp)

    def claim(self, task_id: str, agent: str) -> dict[str, Any]:
        resp = self._c._request(
            "PUT", f"/api/tasks/{task_id}/claim", json={"agent": agent}
        ) or {}
        return resp.get("task", resp)

    def unclaim(self, task_id: str, agent: str | None = None) -> None:
        body: dict[str, Any] = {}
        if agent is not None:
            body["agent"] = agent
        self._c._request("PUT", f"/api/tasks/{task_id}/unclaim", json=body)

    def complete(
        self,
        task_id: str,
        agent: str | None = None,
        output: str | None = None,
    ) -> None:
        body: dict[str, Any] = {}
        if agent is not None:
            body["agent"] = agent
        if output is not None:
            body["output"] = output
        self._c._request("PUT", f"/api/tasks/{task_id}/complete", json=body)

    def review_result(
        self,
        task_id: str,
        result: str,
        *,
        agent: str | None = None,
        notes: str | None = None,
    ) -> None:
        body: dict[str, Any] = {"result": result}
        if agent is not None:
            body["agent"] = agent
        if notes is not None:
            body["notes"] = notes
        self._c._request("PUT", f"/api/tasks/{task_id}/review-result", json=body)

    def cancel(self, task_id: str) -> None:
        self._c._request("DELETE", f"/api/tasks/{task_id}")


class _QueueApi(_SubApi):
    """Operations on ``/api/queue`` and ``/api/item/{id}`` reads."""

    def list(self) -> list[dict[str, Any]]:
        resp = self._c._request("GET", "/api/queue")
        if isinstance(resp, list):
            return resp
        if isinstance(resp, dict):
            return resp.get("items", [])
        return []

    def get(self, item_id: str) -> dict[str, Any]:
        resp = self._c._request("GET", f"/api/item/{item_id}") or {}
        return resp.get("item", resp)


class _ItemsApi(_SubApi):
    """Per-item mutations on ``/api/item/{id}/*`` plus heartbeat."""

    def claim(self, item_id: str, agent: str, note: str | None = None) -> None:
        body: dict[str, Any] = {"agent": agent}
        if note is not None:
            body["note"] = note
        self._c._request("POST", f"/api/item/{item_id}/claim", json=body)

    def complete(
        self,
        item_id: str,
        agent: str,
        *,
        result: str | None = None,
        resolution: str | None = None,
    ) -> None:
        body: dict[str, Any] = {"agent": agent}
        if result is not None:
            body["result"] = result
        if resolution is not None:
            body["resolution"] = resolution
        self._c._request("POST", f"/api/item/{item_id}/complete", json=body)

    def fail(self, item_id: str, agent: str, reason: str) -> None:
        self._c._request(
            "POST",
            f"/api/item/{item_id}/fail",
            json={"agent": agent, "reason": reason},
        )

    def comment(self, item_id: str, agent: str, comment: str) -> None:
        self._c._request(
            "POST",
            f"/api/item/{item_id}/comment",
            json={"agent": agent, "comment": comment},
        )

    def keepalive(self, item_id: str, agent: str, note: str | None = None) -> None:
        body: dict[str, Any] = {"agent": agent}
        if note is not None:
            body["note"] = note
        self._c._request("POST", f"/api/item/{item_id}/keepalive", json=body)

    def heartbeat(
        self,
        agent: str,
        *,
        status: str | None = None,
        note: str | None = None,
        host: str | None = None,
        ssh_user: str | None = None,
        ssh_host: str | None = None,
        ssh_port: int | None = None,
        ts: str | None = None,
    ) -> None:
        body: dict[str, Any] = {}
        for k, v in (
            ("ts", ts),
            ("status", status),
            ("note", note),
            ("host", host),
            ("ssh_user", ssh_user),
            ("ssh_host", ssh_host),
            ("ssh_port", ssh_port),
        ):
            if v is not None:
                body[k] = v
        self._c._request("POST", f"/api/heartbeat/{agent}", json=body)


class _BusApi(_SubApi):
    """Bus send + recent-messages query. SSE streaming deferred to v2."""

    def send(self, kind: str, **fields: Any) -> None:
        body: dict[str, Any] = {"type": kind}
        body.update({k: v for k, v in fields.items() if v is not None})
        self._c._request("POST", "/api/bus/send", json=body)

    def messages(
        self,
        *,
        kind: str | None = None,
        limit: int | None = None,
    ) -> list[dict[str, Any]]:
        params: dict[str, Any] = {}
        if kind is not None:
            params["type"] = kind
        if limit is not None:
            params["limit"] = limit
        resp = self._c._request("GET", "/api/bus/messages", params=params)
        if isinstance(resp, list):
            return resp
        if isinstance(resp, dict):
            return resp.get("messages", [])
        return []


class _MemoryApi(_SubApi):
    """Semantic search + store on ``/api/memory/*``."""

    def search(
        self,
        query: str,
        *,
        limit: int | None = None,
        collection: str | None = None,
    ) -> list[dict[str, Any]]:
        body: dict[str, Any] = {"query": query}
        if limit is not None:
            body["limit"] = limit
        if collection is not None:
            body["collection"] = collection
        resp = self._c._request("POST", "/api/memory/search", json=body)
        if isinstance(resp, dict):
            # Server may return {results: [...]} or {hits: [...]}
            return resp.get("results", resp.get("hits", []))
        if isinstance(resp, list):
            return resp
        return []

    def store(
        self,
        text: str,
        *,
        metadata: dict[str, Any] | None = None,
        collection: str | None = None,
    ) -> None:
        body: dict[str, Any] = {"text": text}
        if metadata is not None:
            body["metadata"] = metadata
        if collection is not None:
            body["collection"] = collection
        self._c._request("POST", "/api/memory/store", json=body)


class _AgentsApi(_SubApi):
    """Agent registry reads on ``/api/agents``."""

    def list(self, *, online: bool | None = None) -> list[dict[str, Any]]:
        params: dict[str, Any] = {}
        if online is not None:
            params["online"] = "true" if online else "false"
        resp = self._c._request("GET", "/api/agents", params=params)
        if isinstance(resp, list):
            return resp
        if isinstance(resp, dict):
            return resp.get("agents", [])
        return []

    def names(self, *, online: bool = False) -> list[str]:
        params: dict[str, Any] = {}
        if online:
            params["online"] = "true"
        resp = self._c._request("GET", "/api/agents/names", params=params)
        if isinstance(resp, list):
            return [n for n in resp if isinstance(n, str)]
        if isinstance(resp, dict):
            names = resp.get("names", [])
            return [n for n in names if isinstance(n, str)]
        return []

    def get(self, name: str) -> dict[str, Any]:
        resp = self._c._request("GET", f"/api/agents/{name}") or {}
        return resp.get("agent", resp)
