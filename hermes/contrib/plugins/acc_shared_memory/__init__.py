"""
acc_shared_memory — peer-to-peer working memory via ACC's Qdrant store.

Gives each hermes agent instance access to the fleet's shared semantic
memory, enabling peer agents to build on each other's work without
explicit handoffs.

Hooks:
  pre_llm_call    → query ACC /api/memory/search for fleet context
                    relevant to the current turn; inject as user context
  post_llm_call   → store assistant response summaries in shared memory
                    after every N calls (configurable, default 5)
  on_session_end  → store final response + session metadata for
                    cross-session recall

Query strategy follows the WorkingMemoryAssembler tiered pattern
(from docs/instar-studies/04-working-memory-assembler.md):
  - top 3 results: full content
  - next 7 results: first sentence + score
  - rest: names only
  Total budget: ~1200 tokens injected into the user message.

Environment variables:
  ACC_URL             Hub base URL (e.g. http://100.89.199.14:8789)
  CCC_AGENT_TOKEN     Bearer token (shared with ccc_integration plugin)
  ACC_AGENT_NAME      Agent identity tag on stored memories
  ACC_MEMORY_STORE_EVERY_N   How often to store mid-session (default 5)
  ACC_MEMORY_SEARCH_LIMIT    Max results to fetch (default 10)
  ACC_MEMORY_ENABLED  Set to "0" to disable without removing plugin
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
import time
import re

logger = logging.getLogger(__name__)

_STORE_EVERY_N = int(os.environ.get("ACC_MEMORY_STORE_EVERY_N", "5"))
_SEARCH_LIMIT = int(os.environ.get("ACC_MEMORY_SEARCH_LIMIT", "10"))
_TOKEN_BUDGET = 1200  # approximate tokens to inject per turn

# Stop words that pollute memory queries (from instar-studies/04)
_STOP_WORDS = frozenset({
    "a", "an", "the", "is", "it", "its", "in", "on", "at", "to", "for",
    "of", "and", "or", "but", "with", "this", "that", "these", "those",
    "be", "are", "was", "were", "has", "have", "had", "do", "does", "did",
    "will", "would", "could", "should", "may", "might", "can",
    "please", "implement", "build", "check", "create", "update", "add",
    "fix", "test", "run", "make", "get", "set", "use", "show", "find",
    "help", "need", "want", "just", "also", "now", "then", "here", "there",
    "i", "me", "my", "we", "our", "you", "your",
})

_call_counter = 0
_session_user_message = ""  # captured from first pre_llm_call


def _enabled() -> bool:
    return os.environ.get("ACC_MEMORY_ENABLED", "1") not in ("0", "false", "no")


def _cfg() -> tuple[str, str, str]:
    """Return (acc_url, token, agent_name). Raises if not configured."""
    url = os.environ.get("ACC_URL", os.environ.get("CCC_URL", "")).rstrip("/")
    token = os.environ.get("CCC_AGENT_TOKEN", "")
    name = (
        os.environ.get("ACC_AGENT_NAME")
        or os.environ.get("CCC_AGENT_NAME")
        or os.environ.get("AGENT_NAME")
        or os.uname().nodename.split(".")[0]
    )
    if not url or not token:
        raise RuntimeError("ACC_URL (or CCC_URL) and CCC_AGENT_TOKEN must be set")
    return url, token, name


def _http(method: str, path: str, body: dict | None = None) -> dict | None:
    """Synchronous curl to ACC hub. Returns parsed JSON or None."""
    try:
        url, token, _ = _cfg()
    except RuntimeError:
        return None

    cmd = [
        "curl", "-sf", "--max-time", "12",
        "-X", method,
        "-H", f"Authorization: Bearer {token}",
        "-H", "Content-Type: application/json",
    ]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    cmd.append(f"{url}{path}")

    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        if r.returncode == 0 and r.stdout.strip():
            return json.loads(r.stdout)
    except Exception as e:
        logger.debug("acc_shared_memory request failed: %s", e)
    return None


# ── Query term extraction ──────────────────────────────────────────────────────

def _extract_terms(text: str, max_terms: int = 8) -> list[str]:
    """Extract significant query terms, stripping stop words."""
    words = re.findall(r"[a-zA-Z][a-zA-Z0-9_-]{2,}", text.lower())
    seen: dict[str, int] = {}
    for w in words:
        if w not in _STOP_WORDS:
            seen[w] = seen.get(w, 0) + 1
    # Sort by frequency desc, dedupe, take top N
    return [w for w, _ in sorted(seen.items(), key=lambda x: -x[1])][:max_terms]


# ── Tiered rendering (WorkingMemoryAssembler pattern) ─────────────────────────

def _approx_tokens(text: str) -> int:
    return max(1, len(text) // 4)


def _render_results(results: list[dict], budget: int = _TOKEN_BUDGET) -> str:
    """
    Tiered render: top 3 full, next 7 compact (first sentence + score),
    rest as names only.  Stops when budget is exhausted.
    """
    if not results:
        return ""

    parts: list[str] = []
    used = 0

    for i, r in enumerate(results):
        text = r.get("text", "").strip()
        agent = r.get("agent", "?")
        score = r.get("score", 0.0)
        ts = r.get("timestamp", "")[:10]

        if i < 3:
            # Full content
            chunk = f"[{agent} {ts} score={score:.2f}]\n{text}"
        elif i < 10:
            # First sentence only
            first = (text.split(".")[0] + ".") if "." in text else text[:120]
            chunk = f"[{agent} {ts} score={score:.2f}] {first}"
        else:
            # Name only
            first_line = text.splitlines()[0][:80] if text else "…"
            chunk = f"• {agent}: {first_line}"

        tok = _approx_tokens(chunk)
        if used + tok > budget:
            if i >= 10:
                # Already in name-only mode; safe to skip rest
                break
            # Truncate to fit
            remaining = budget - used
            chunk = chunk[: remaining * 4]
            if chunk:
                parts.append(chunk)
            break

        parts.append(chunk)
        used += tok

    return "\n\n".join(parts)


# ── Memory search ──────────────────────────────────────────────────────────────

def _search_fleet_memory(query: str) -> str:
    """Query ACC /api/memory/search and return tiered rendered context."""
    payload = {
        "query": query,
        "limit": _SEARCH_LIMIT,
        "collection": "acc_memory",
    }
    resp = _http("POST", "/api/memory/search", payload)
    if not resp:
        return ""

    results = resp.get("results", [])
    if not results:
        return ""

    rendered = _render_results(results)
    if not rendered:
        return ""

    try:
        _, _, name = _cfg()
    except RuntimeError:
        name = "?"

    logger.debug("acc_shared_memory: injecting %d results (%d chars)",
                 len(results), len(rendered))
    return (
        f"## Fleet Working Memory (from peer agents)\n\n"
        f"{rendered}\n\n"
        f"---\n"
        f"*(Retrieved by {name} for this turn — use as background context)*"
    )


# ── Memory store ───────────────────────────────────────────────────────────────

def _store_memory(text: str, tags: list[str] | None = None, session_id: str = "") -> None:
    """Store text in ACC shared memory with agent provenance."""
    if not text.strip():
        return
    try:
        _, _, name = _cfg()
    except RuntimeError:
        return

    payload = {
        "text": text[:6000],  # Qdrant payload size limit
        "metadata": {
            "agent": name,
            "session_id": session_id,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "tags": tags or [],
            "source": "hermes_session",
        },
    }
    ok = _http("POST", "/api/memory/store", payload)
    if ok:
        logger.debug("acc_shared_memory: stored %d chars (agent=%s)", len(text), name)


# ── Hook handlers ──────────────────────────────────────────────────────────────

def _pre_llm_call(**kwargs) -> dict | None:
    """Inject fleet memory context before each LLM call."""
    global _session_user_message

    user_message = kwargs.get("user_message", "")
    if not user_message:
        return None

    # Capture first user message as session anchor for later storage
    if not _session_user_message:
        _session_user_message = user_message

    # Extract meaningful query terms to avoid noise in embedding search
    terms = _extract_terms(user_message)
    if not terms:
        return None

    query = " ".join(terms[:6])
    context = _search_fleet_memory(query)
    if not context:
        return None

    return {"context": context}


def _post_llm_call(**kwargs) -> None:
    """Periodically store assistant response in shared memory."""
    global _call_counter
    _call_counter += 1

    if _call_counter % _STORE_EVERY_N != 0:
        return

    # Get the assistant response from this call
    final_response = kwargs.get("final_response") or kwargs.get("assistant_response", "")
    if not final_response:
        return

    session_id = kwargs.get("session_id", "")
    api_call_count = kwargs.get("api_call_count", _call_counter)

    # Build a compact summary to store: task anchor + current response
    summary_parts = []
    if _session_user_message:
        anchor = _session_user_message[:200]
        summary_parts.append(f"Task: {anchor}")
    summary_parts.append(f"Progress (call {api_call_count}): {final_response[:800]}")
    summary = "\n\n".join(summary_parts)

    _store_memory(summary, tags=["progress", "mid_session"], session_id=session_id)


def _on_session_end(**kwargs) -> None:
    """Store final session output in shared memory for peer recall."""
    completed = kwargs.get("completed", False)
    final_response = kwargs.get("final_response") or ""
    session_id = kwargs.get("session_id", "")
    exit_reason = kwargs.get("exit_reason", "unknown")

    if not final_response:
        return

    parts = []
    if _session_user_message:
        parts.append(f"Task: {_session_user_message[:300]}")
    parts.append(f"Result ({exit_reason}): {final_response[:2000]}")
    summary = "\n\n".join(parts)

    tags = ["completed" if completed else "incomplete", "session_end"]
    _store_memory(summary, tags=tags, session_id=session_id)
    logger.info("acc_shared_memory: stored session result (completed=%s)", completed)


# ── Plugin registration ────────────────────────────────────────────────────────

def register(ctx) -> None:
    """Called by hermes plugin loader at startup."""
    if not _enabled():
        logger.info("acc_shared_memory: disabled via ACC_MEMORY_ENABLED=0")
        return

    try:
        url, _, name = _cfg()
    except RuntimeError as e:
        logger.info("acc_shared_memory: skipping — %s", e)
        return

    logger.info("acc_shared_memory: active (hub=%s agent=%s store_every=%d)",
                url, name, _STORE_EVERY_N)

    ctx.register_hook("pre_llm_call", _pre_llm_call)
    ctx.register_hook("post_llm_call", _post_llm_call)
    ctx.register_hook("on_session_end", _on_session_end)
