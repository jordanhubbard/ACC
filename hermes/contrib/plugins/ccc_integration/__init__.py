"""
ccc_integration — Hermes plugin for CCC cluster integration.

Keeps the CCC hub informed of hermes agent activity so queue items are
tracked properly and the fleet health dashboard reflects hermes sessions.

Hooks registered:
  on_session_start  → POST heartbeat (agent alive, task starting)
  post_llm_call     → POST heartbeat every N LLM calls (keepalive during work)
  on_session_end    → POST complete or fail to queue item (if CCC_QUEUE_ITEM_ID set)

Environment variables consumed:
  CCC_URL             Hub base URL (e.g. http://100.89.199.14:8789)
  CCC_AGENT_TOKEN     Bearer token for this agent
  CCC_AGENT_NAME      Agent name (defaults to $AGENT_NAME or hostname)
  CCC_QUEUE_ITEM_ID   Optional — queue item to mark complete/fail on exit
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
import threading
import time

logger = logging.getLogger(__name__)

# How often to post a heartbeat (every N post_llm_call invocations).
_HEARTBEAT_EVERY_N_CALLS = 3
_call_counter = 0
_last_heartbeat_ts = 0.0
_HEARTBEAT_MIN_INTERVAL = 60.0  # never more than once per minute


def _env() -> tuple[str, str, str]:
    """Return (ccc_url, token, agent_name) or raise if not configured."""
    url = os.environ.get("CCC_URL", "").rstrip("/")
    token = os.environ.get("CCC_AGENT_TOKEN", "")
    name = (
        os.environ.get("CCC_AGENT_NAME")
        or os.environ.get("AGENT_NAME")
        or os.uname().nodename.split(".")[0]
    )
    if not url or not token:
        raise RuntimeError("CCC_URL and CCC_AGENT_TOKEN must be set")
    return url, token, name


def _curl(method: str, path: str, body: dict | None = None) -> bool:
    """Fire-and-forget curl to CCC hub. Returns True on success."""
    try:
        url, token, _ = _env()
    except RuntimeError:
        return False

    cmd = [
        "curl", "-sf", "--max-time", "8",
        "-X", method,
        "-H", f"Authorization: Bearer {token}",
        "-H", "Content-Type: application/json",
    ]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    cmd.append(f"{url}{path}")

    try:
        result = subprocess.run(cmd, capture_output=True, timeout=10)
        return result.returncode == 0
    except Exception as e:
        logger.debug("CCC heartbeat failed: %s", e)
        return False


def _post_heartbeat(status: str = "ok", note: str = "") -> None:
    global _last_heartbeat_ts
    now = time.time()
    if now - _last_heartbeat_ts < _HEARTBEAT_MIN_INTERVAL:
        return
    try:
        _, _, name = _env()
    except RuntimeError:
        return
    body: dict = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "status": status}
    if note:
        body["note"] = note[:200]
    ok = _curl("POST", f"/api/heartbeat/{name}", body)
    if ok:
        _last_heartbeat_ts = now
        logger.debug("CCC heartbeat posted (status=%s)", status)


def _post_queue_result(success: bool, result_text: str) -> None:
    item_id = os.environ.get("CCC_QUEUE_ITEM_ID", "").strip()
    if not item_id:
        return
    try:
        _, _, name = _env()
    except RuntimeError:
        return

    if success:
        _curl("POST", f"/api/item/{item_id}/complete", {
            "agent": name,
            "result": result_text[:4000],
            "resolution": result_text[:4000],
        })
        logger.info("CCC queue item %s marked complete", item_id)
    else:
        _curl("POST", f"/api/item/{item_id}/fail", {
            "agent": name,
            "reason": result_text[:2000],
        })
        logger.info("CCC queue item %s marked failed", item_id)


# ── Hook handlers ─────────────────────────────────────────────────────────────

def _on_session_start(**kwargs):
    """Fire initial heartbeat when hermes starts a session."""
    session_id = kwargs.get("session_id", "?")
    note = f"hermes session {session_id} started"
    # Don't enforce min-interval on startup heartbeat
    global _last_heartbeat_ts
    _last_heartbeat_ts = 0.0
    _post_heartbeat(status="ok", note=note)


def _post_llm_call(**kwargs):
    """Post a keepalive heartbeat every N LLM calls so CCC sees activity."""
    global _call_counter
    _call_counter += 1
    if _call_counter % _HEARTBEAT_EVERY_N_CALLS == 0:
        api_calls = kwargs.get("api_call_count", _call_counter)
        max_iter = kwargs.get("max_iterations", "?")
        note = f"hermes working (call {api_calls}/{max_iter})"
        _post_heartbeat(status="ok", note=note)


def _on_session_end(**kwargs):
    """Mark the queue item complete or failed when the session ends."""
    completed = kwargs.get("completed", False)
    final_response = kwargs.get("final_response") or ""
    exit_reason = kwargs.get("exit_reason", "unknown")

    # Final heartbeat
    _last_heartbeat_ts_bak = globals().get("_last_heartbeat_ts", 0.0)
    globals()["_last_heartbeat_ts"] = 0.0
    status = "ok" if completed else "degraded"
    _post_heartbeat(status=status, note=f"session ended: {exit_reason}")

    # Queue item result
    if completed:
        _post_queue_result(success=True, result_text=final_response)
    else:
        _post_queue_result(
            success=False,
            result_text=f"Session ended without completing (exit_reason={exit_reason}). "
                        f"Summary: {final_response[:500]}" if final_response else
                        f"Session ended without completing (exit_reason={exit_reason})",
        )


# ── Plugin registration ───────────────────────────────────────────────────────

def register(ctx) -> None:
    """Called by hermes plugin loader at startup."""
    # Verify env is reachable before registering hooks
    try:
        url, _, name = _env()
    except RuntimeError as e:
        logger.info("ccc_integration: skipping — %s", e)
        return

    logger.info("ccc_integration: active (hub=%s agent=%s)", url, name)

    ctx.register_hook("on_session_start", _on_session_start)
    ctx.register_hook("post_llm_call", _post_llm_call)
    ctx.register_hook("on_session_end", _on_session_end)
