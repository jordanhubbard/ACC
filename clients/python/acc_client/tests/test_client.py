"""Integration tests against a mocked HTTP transport."""
from __future__ import annotations

import httpx
import pytest
import respx

from acc_client import (
    ApiError,
    Client,
    Conflict,
    Locked,
    NotFound,
    Unauthorized,
)


@pytest.fixture
def client():
    c = Client(base_url="http://hub.test", token="t")
    yield c
    c.close()


# ── tasks ─────────────────────────────────────────────────────────────


@respx.mock
def test_tasks_list_filters_by_status(client):
    respx.get("http://hub.test/api/tasks", params={"status": "open", "limit": 5}).mock(
        return_value=httpx.Response(200, json={"tasks": [{"id": "t-1"}], "count": 1})
    )
    tasks = client.tasks.list(status="open", limit=5)
    assert tasks == [{"id": "t-1"}]


@respx.mock
def test_tasks_get_unwraps_task_envelope(client):
    respx.get("http://hub.test/api/tasks/t-1").mock(
        return_value=httpx.Response(200, json={"task": {"id": "t-1", "title": "x"}})
    )
    assert client.tasks.get("t-1")["title"] == "x"


@respx.mock
def test_tasks_claim_409_raises_conflict(client):
    respx.put("http://hub.test/api/tasks/t-9/claim").mock(
        return_value=httpx.Response(409, json={"error": "already_claimed"})
    )
    with pytest.raises(Conflict) as exc:
        client.tasks.claim("t-9", agent="a")
    assert exc.value.code == "already_claimed"
    assert exc.value.status == 409


@respx.mock
def test_tasks_claim_423_preserves_pending_field(client):
    respx.put("http://hub.test/api/tasks/t-9/claim").mock(
        return_value=httpx.Response(
            423, json={"error": "blocked", "pending": "t-1"}
        )
    )
    with pytest.raises(Locked) as exc:
        client.tasks.claim("t-9", agent="a")
    assert exc.value.extra["pending"] == "t-1"


@respx.mock
def test_tasks_complete_sends_body(client):
    route = respx.put("http://hub.test/api/tasks/t-1/complete").mock(
        return_value=httpx.Response(200, json={"ok": True})
    )
    client.tasks.complete("t-1", agent="a", output="done")
    assert route.calls.last.request.read() == b'{"agent":"a","output":"done"}'


@respx.mock
def test_non_json_error_body_still_maps(client):
    respx.put("http://hub.test/api/tasks/t-1/complete").mock(
        return_value=httpx.Response(500, text="internal error")
    )
    with pytest.raises(ApiError) as exc:
        client.tasks.complete("t-1", agent="a")
    assert exc.value.status == 500
    assert exc.value.code == "http_500"


# ── memory (the hermes plugin's path) ─────────────────────────────────


@respx.mock
def test_memory_search_accepts_results_envelope(client):
    respx.post("http://hub.test/api/memory/search").mock(
        return_value=httpx.Response(
            200,
            json={
                "results": [
                    {"text": "hit 1", "score": 0.9},
                    {"text": "hit 2", "score": 0.8},
                ]
            },
        )
    )
    hits = client.memory.search("buffer overflow", limit=10, collection="acc_memory")
    assert len(hits) == 2
    assert hits[0]["score"] == 0.9


@respx.mock
def test_memory_search_accepts_hits_envelope(client):
    respx.post("http://hub.test/api/memory/search").mock(
        return_value=httpx.Response(200, json={"hits": [{"text": "h"}]})
    )
    assert client.memory.search("q") == [{"text": "h"}]


@respx.mock
def test_memory_store_sends_metadata(client):
    route = respx.post("http://hub.test/api/memory/store").mock(
        return_value=httpx.Response(200, json={"ok": True})
    )
    client.memory.store("some text", metadata={"agent": "boris", "tags": ["done"]})
    body = route.calls.last.request.read()
    assert b'"text":"some text"' in body
    assert b'"agent":"boris"' in body


# ── items / heartbeat ─────────────────────────────────────────────────


@respx.mock
def test_item_claim_409_raises_conflict(client):
    respx.post("http://hub.test/api/item/wq-9/claim").mock(
        return_value=httpx.Response(409, json={"error": "already_claimed"})
    )
    with pytest.raises(Conflict):
        client.items.claim("wq-9", agent="a")


@respx.mock
def test_heartbeat_posts_to_named_agent(client):
    route = respx.post("http://hub.test/api/heartbeat/boris").mock(
        return_value=httpx.Response(200, json={"ok": True})
    )
    client.items.heartbeat("boris", status="ok", note="cycle 1")
    assert route.calls.call_count == 1


# ── bus ───────────────────────────────────────────────────────────────


@respx.mock
def test_bus_send_uses_type_field_on_wire(client):
    route = respx.post("http://hub.test/api/bus/send").mock(
        return_value=httpx.Response(200, json={"ok": True})
    )
    client.bus.send("hello", from_="tester", body="hi")
    body = route.calls.last.request.read()
    assert b'"type":"hello"' in body


@respx.mock
def test_bus_messages_filters_by_kind(client):
    respx.get("http://hub.test/api/bus/messages", params={"type": "tasks:claimed"}).mock(
        return_value=httpx.Response(
            200,
            json={"messages": [{"id": "m-1", "type": "tasks:claimed"}]},
        )
    )
    msgs = client.bus.messages(kind="tasks:claimed")
    assert msgs == [{"id": "m-1", "type": "tasks:claimed"}]


# ── misc ──────────────────────────────────────────────────────────────


@respx.mock
def test_404_raises_notfound(client):
    respx.get("http://hub.test/api/tasks/nope").mock(
        return_value=httpx.Response(404, json={"error": "not_found"})
    )
    with pytest.raises(NotFound):
        client.tasks.get("nope")


@respx.mock
def test_401_raises_unauthorized(client):
    respx.get("http://hub.test/api/tasks").mock(
        return_value=httpx.Response(401, json={"error": "unauthorized"})
    )
    with pytest.raises(Unauthorized):
        client.tasks.list()


def test_context_manager_closes_http(monkeypatch):
    monkeypatch.setenv("ACC_TOKEN", "t")
    with Client(base_url="http://hub.test") as c:
        assert c.base_url == "http://hub.test"
