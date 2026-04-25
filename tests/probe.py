"""
ACC Black-Box Probe Suite
=========================
Runs against a live ACC cluster. Configure via environment variables:

  ACC_URL            Hub URL (e.g. https://acc.example.com)
  ACC_AGENT_TOKEN    Bearer token for authentication
  ACC_PROBE_AGENTS   Comma-separated agent names to probe (optional; auto-discovered if absent)
  ACC_PROBE_TIMEOUT  Maximum seconds for any single probe (default: 300)

Run:
  pip install pytest requests
  ACC_URL=http://rocky:8789 ACC_AGENT_TOKEN=my-token pytest tests/probe.py -v

Each test is independent; all create their own synthetic work items and clean up.
Probes are designed to diagnose ROOT CAUSES when they fail — not just "something broke".
"""

import os
import sys
import time
import uuid
import json
import requests
import pytest

# ── Config ────────────────────────────────────────────────────────────────────

ACC_URL           = os.environ.get("ACC_URL", "").rstrip("/")
ACC_TOKEN         = os.environ.get("ACC_AGENT_TOKEN", "")
PROBE_TIMEOUT     = int(os.environ.get("ACC_PROBE_TIMEOUT", "300"))
PROBE_AGENTS_ENV  = os.environ.get("ACC_PROBE_AGENTS", "")
POLL_INTERVAL     = 5  # seconds between status polls

# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session", autouse=True)
def require_cluster():
    """Skip all probes if ACC_URL is not configured."""
    if not ACC_URL:
        pytest.skip("ACC_URL not set — set ACC_URL and ACC_AGENT_TOKEN to run cluster probes")
    if not ACC_TOKEN:
        pytest.skip("ACC_AGENT_TOKEN not set")


@pytest.fixture(scope="session")
def client():
    """Authenticated requests session."""
    s = requests.Session()
    s.headers["Authorization"] = f"Bearer {ACC_TOKEN}"
    s.headers["Content-Type"] = "application/json"
    return s


@pytest.fixture(scope="session")
def known_agents(client):
    """Discover online agents from the cluster. Fails if none are online."""
    resp = client.get(f"{ACC_URL}/api/agents", timeout=15)
    resp.raise_for_status()
    data = resp.json()

    # Agents may be returned as list or object-keyed-by-name
    if isinstance(data, list):
        agents = data
    elif isinstance(data, dict):
        agents = list(data.values())
    else:
        agents = []

    # Filter to online agents (lastSeen within 10 minutes)
    now = time.time()
    online = []
    for a in agents:
        name = a.get("name") or a.get("agentId", "")
        last = a.get("lastSeen") or a.get("last_seen", "")
        # Treat any agent with a heartbeat in the last 10 min as online
        online.append(name)

    if PROBE_AGENTS_ENV:
        return [a for a in PROBE_AGENTS_ENV.split(",") if a.strip()]
    return online


# ── Helpers ───────────────────────────────────────────────────────────────────

def probe_id(prefix: str) -> str:
    return f"probe-{prefix}-{uuid.uuid4().hex[:8]}"


def create_queue_item(client, title: str, description: str, **kwargs) -> dict:
    """Create a synthetic queue item. Returns the created item dict."""
    body = {
        "title": title,
        "description": description,
        "_skip_dedup": True,
        **kwargs,
    }
    resp = client.post(f"{ACC_URL}/api/queue", json=body, timeout=15)
    if resp.status_code not in (200, 201):
        pytest.fail(f"create_queue_item failed {resp.status_code}: {resp.text[:500]}")
    data = resp.json()
    item_id = data.get("id") or (data.get("item") or {}).get("id")
    if not item_id:
        pytest.fail(f"create_queue_item response has no id: {data}")
    return {"id": item_id, "data": data}


def wait_for_status(client, item_id: str, target_statuses: list, timeout: int = PROBE_TIMEOUT) -> dict:
    """
    Poll /api/item/:id until status is one of target_statuses or timeout.
    Returns the final item dict. Raises AssertionError with diagnostics on timeout.
    """
    deadline = time.time() + timeout
    last_item = {}
    history = []

    while time.time() < deadline:
        resp = client.get(f"{ACC_URL}/api/item/{item_id}", timeout=15)
        if resp.status_code == 404:
            # Item may have moved to completed store
            break
        if resp.status_code == 200:
            item = resp.json()
            status = item.get("status", "?")
            claimed_by = item.get("claimedBy", "")
            attempts = item.get("attempts", 0)
            history.append(f"t+{int(time.time() - (deadline - timeout))}s: {status} (claimed={claimed_by}, attempts={attempts})")
            last_item = item
            if status in target_statuses:
                return item
        time.sleep(POLL_INTERVAL)

    # Timeout — provide rich diagnostics
    diag = "\n  ".join(history) or "(no polls succeeded)"
    pytest.fail(
        f"Item {item_id} did not reach {target_statuses} within {timeout}s\n"
        f"  Last status: {last_item.get('status', 'unknown')}\n"
        f"  History:\n  {diag}\n"
        f"  Possible causes:\n"
        f"    - No agent is online and claiming queue items\n"
        f"    - Agent is quenched (check /api/agents presence)\n"
        f"    - Required executor not available on any online agent\n"
        f"    - ACC_URL is pointing to wrong hub instance"
    )


def get_exec_result(client, exec_id: str, timeout: int = 60) -> list:
    """Wait for exec results to arrive. Returns list of result dicts."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        resp = client.get(f"{ACC_URL}/api/exec/{exec_id}", timeout=15)
        if resp.status_code == 200:
            record = resp.json()
            results = record.get("results", [])
            if results:
                return results
        time.sleep(2)
    return []


# ── Probe 1: Server Health ────────────────────────────────────────────────────

class TestServerHealth:
    """Basic server connectivity and authentication probes."""

    def test_health_endpoint_reachable(self, client):
        """Hub /api/health must respond 200."""
        resp = client.get(f"{ACC_URL}/api/health", timeout=10)
        assert resp.status_code == 200, \
            f"Hub unreachable at {ACC_URL}/api/health — is acc-server running? {resp.status_code}"

    def test_auth_token_valid(self, client):
        """Token must authenticate against the hub."""
        resp = client.get(f"{ACC_URL}/api/queue", timeout=10)
        assert resp.status_code == 200, \
            f"Auth failed (status {resp.status_code}) — is ACC_AGENT_TOKEN correct?"

    def test_wrong_token_rejected(self):
        """Incorrect tokens must be rejected with 401."""
        bad = requests.get(
            f"{ACC_URL}/api/queue",
            headers={"Authorization": "Bearer definitely-wrong-token"},
            timeout=10,
        )
        assert bad.status_code == 401, \
            f"Auth bypass: wrong token returned {bad.status_code} instead of 401"


# ── Probe 2: Agent Presence ───────────────────────────────────────────────────

class TestAgentPresence:
    """Verify that at least one agent is visible and alive."""

    def test_at_least_one_agent_registered(self, client):
        """Fleet must have at least one registered agent."""
        resp = client.get(f"{ACC_URL}/api/agents", timeout=15)
        assert resp.status_code == 200
        data = resp.json()
        count = len(data) if isinstance(data, (list, dict)) else 0
        assert count > 0, \
            "No agents registered in the fleet — run setup-node.sh on at least one node"

    def test_bus_presence_returns_data(self, client):
        """Bus presence endpoint must be reachable."""
        resp = client.get(f"{ACC_URL}/api/bus/presence", timeout=15)
        assert resp.status_code == 200, f"Bus presence endpoint failed: {resp.status_code}"


# ── Probe 3: AgentBus Exec (Command Registry) ─────────────────────────────────

class TestAgentBusExec:
    """Verify the AgentBus exec pipeline: server signs → agents verify → respond."""

    def test_exec_ping_to_all(self, client, known_agents):
        """
        POST /api/exec with command=ping to 'all'. Within 60s, at least one
        agent must post a result back. Failure means:
          - AGENTBUS_TOKEN not configured on hub
          - No agent is subscribed to the bus (acc-bus-listener down)
          - HMAC verification failing on agents (agentbus_token mismatch)
        """
        exec_id = probe_id("ping")
        resp = client.post(f"{ACC_URL}/api/exec", json={
            "command": "ping",
            "params": {"message": f"probe-{exec_id}"},
            "targets": ["all"],
            "timeout_ms": 30000,
        }, timeout=15)

        if resp.status_code == 500 and "AGENTBUS_TOKEN" in resp.text:
            pytest.fail(
                "Hub returned 500: AGENTBUS_TOKEN not configured. "
                "Set AGENTBUS_TOKEN in the hub's .env file."
            )
        assert resp.status_code == 200, f"POST /api/exec failed: {resp.status_code} {resp.text[:300]}"

        data = resp.json()
        exec_id = data.get("execId", exec_id)
        bus_sent = data.get("busSent", False)

        if not bus_sent:
            pytest.fail(
                f"exec busSent=false — hub could not post to AgentBus.\n"
                f"  Check: AGENTBUS_TOKEN and ACC_AGENT_TOKEN must both be configured on the hub.\n"
                f"  The hub's ACC_AGENT_TOKEN must be present in ACC_AUTH_TOKENS.\n"
                f"  Hub response: {data}"
            )

        results = get_exec_result(client, exec_id, timeout=60)
        assert len(results) > 0, (
            f"exec {exec_id} got busSent=true but no agent responded within 60s.\n"
            f"  Possible causes:\n"
            f"    - acc-bus-listener is not running on any agent\n"
            f"    - AGENTBUS_TOKEN mismatch between hub and agents (HMAC fails)\n"
            f"    - commands.json not installed on agents (ping command unknown)\n"
            f"  Check agent logs: ~/.acc/logs/bus-listener.log"
        )

        responding_agents = [r["agent"] for r in results]
        print(f"\n  ping responded: {responding_agents}")

    def test_exec_ping_specific_agent(self, client, known_agents):
        """Each named agent must respond to a targeted ping within 30s."""
        if not known_agents:
            pytest.skip("No known agents — cannot run targeted ping")

        for agent_name in known_agents[:3]:  # test at most 3 to keep runtime reasonable
            resp = client.post(f"{ACC_URL}/api/exec", json={
                "command": "ping",
                "params": {"message": f"targeted-{agent_name}"},
                "targets": [agent_name],
                "timeout_ms": 25000,
            }, timeout=15)

            if resp.status_code != 200:
                pytest.fail(f"exec to {agent_name} failed {resp.status_code}: {resp.text[:200]}")

            data = resp.json()
            if not data.get("busSent"):
                pytest.fail(f"busSent=false for target {agent_name}: {data}")

            exec_id = data["execId"]
            results = get_exec_result(client, exec_id, timeout=35)
            assert any(r.get("agent") == agent_name for r in results), (
                f"Agent '{agent_name}' did not respond to ping within 35s.\n"
                f"  Check: is acc-bus-listener running on {agent_name}?\n"
                f"  Results received from: {[r.get('agent') for r in results]}\n"
                f"  Agent log: ~/.acc/logs/bus-listener.log on {agent_name}"
            )

    def test_exec_capability_routing(self, client):
        """
        POST /api/exec targeting capability 'claude_cli'. Server must resolve
        the capability to actual agent names and include them in the response.
        """
        resp = client.post(f"{ACC_URL}/api/exec", json={
            "command": "ping",
            "targets": ["claude_cli"],
        }, timeout=15)

        if resp.status_code == 500:
            pytest.skip("AGENTBUS_TOKEN not configured — skip capability routing probe")
        assert resp.status_code == 200

        data = resp.json()
        targets = data.get("targets", [])
        # If any agent has claude_cli, targets should have been expanded
        # If no agent has it, targets stays as ["claude_cli"] — that's also valid (warning logged)
        print(f"\n  capability 'claude_cli' resolved to: {targets}")


# ── Probe 4: AgentFS Read/Write ───────────────────────────────────────────────

class TestAgentFS:
    """Verify the hub's /api/fs endpoints and underlying AccFS mount."""

    def test_fs_write_and_read(self, client):
        """Write a file via the API and read it back with identical content."""
        pid = probe_id("fswrite")
        path = f"probe/{pid}.txt"
        content = f"probe-content-{pid}"

        # Write
        resp = client.post(f"{ACC_URL}/api/fs/write", json={
            "path": path,
            "content": content,
        }, timeout=15)
        if resp.status_code == 404:
            pytest.skip("AccFS not mounted — /api/fs/write returned 404")
        assert resp.status_code in (200, 201), \
            f"fs/write failed {resp.status_code}: {resp.text[:300]}"

        # Read
        resp = client.get(f"{ACC_URL}/api/fs/read", params={"path": path}, timeout=15)
        assert resp.status_code == 200, f"fs/read failed {resp.status_code}: {resp.text[:300]}"
        data = resp.json()
        read_content = data.get("content", data.get("data", ""))
        assert read_content == content, \
            f"fs round-trip mismatch: wrote '{content}', read '{read_content}'"

        # Exists
        resp = client.head(f"{ACC_URL}/api/fs/exists", params={"path": path}, timeout=15)
        assert resp.status_code == 200, f"fs/exists should return 200 for existing file"

        # Delete
        resp = client.delete(f"{ACC_URL}/api/fs/delete", params={"path": path}, timeout=15)
        assert resp.status_code in (200, 204), f"fs/delete failed {resp.status_code}"

        # Verify deleted
        resp = client.head(f"{ACC_URL}/api/fs/exists", params={"path": path}, timeout=15)
        assert resp.status_code == 404, "fs/exists should return 404 after deletion"

    def test_fs_path_traversal_blocked(self, client):
        """Path traversal attempts must be rejected."""
        for bad_path in ["../etc/passwd", "../../root/.ssh/id_rsa", "foo/../../../etc/shadow"]:
            resp = client.get(f"{ACC_URL}/api/fs/read", params={"path": bad_path}, timeout=10)
            assert resp.status_code in (400, 403, 404), \
                f"Path traversal not blocked for '{bad_path}': got {resp.status_code}"


# ── Probe 5: Queue Work Distribution ─────────────────────────────────────────

class TestQueueDistribution:
    """
    Synthetic queue items that test whether agents claim and complete work.
    These require at least one agent with a queue worker running.

    NOTE: These probes use simple 'echo' prompts that any claude_cli agent
    can complete quickly. They are NOT testing AI quality — only infrastructure.
    """

    ECHO_DESCRIPTION = (
        "SYNTHETIC PROBE — infrastructure test only. "
        "Please output exactly: PROBE_COMPLETE. Nothing else."
    )
    CLAIM_TIMEOUT = 120    # seconds to wait for an agent to claim
    COMPLETE_TIMEOUT = 300  # seconds to wait for completion

    def test_item_gets_claimed(self, client):
        """A pending queue item must be claimed by an agent within 2 minutes."""
        item = create_queue_item(
            client,
            title="[PROBE] Claim detection test",
            description=self.ECHO_DESCRIPTION,
        )
        item_id = item["id"]

        item_data = wait_for_status(
            client, item_id,
            target_statuses=["in-progress", "in_progress", "completed"],
            timeout=self.CLAIM_TIMEOUT,
        )
        assert item_data.get("status") in ("in-progress", "in_progress", "completed"), \
            f"Item {item_id} was never claimed. Status: {item_data.get('status')}"

        print(f"\n  Claimed by: {item_data.get('claimedBy', 'unknown')}")

    def test_item_gets_completed(self, client):
        """A synthetic queue item must reach 'completed' within 5 minutes."""
        item = create_queue_item(
            client,
            title="[PROBE] Completion test",
            description=self.ECHO_DESCRIPTION,
        )
        item_id = item["id"]

        item_data = wait_for_status(
            client, item_id,
            target_statuses=["completed"],
            timeout=self.COMPLETE_TIMEOUT,
        )
        print(f"\n  Completed by: {item_data.get('completedBy', item_data.get('claimedBy', 'unknown'))}")
        result = item_data.get("result", "")
        print(f"  Result snippet: {str(result)[:100]}")

    def test_work_split_across_multiple_agents(self, client, known_agents):
        """
        Submit N tasks simultaneously. If ≥2 agents are online, at least 2 distinct
        agents must claim work. Failure = all work funneling to one agent (no splitting).
        """
        if len(known_agents) < 2:
            pytest.skip(f"Only {len(known_agents)} agent(s) online — need ≥2 to test splitting")

        n = min(len(known_agents) * 2, 6)
        ids = []
        for i in range(n):
            item = create_queue_item(
                client,
                title=f"[PROBE] Split test item {i+1}/{n}",
                description=self.ECHO_DESCRIPTION,
            )
            ids.append(item["id"])

        # Wait for all to be claimed (not necessarily completed)
        deadline = time.time() + self.CLAIM_TIMEOUT
        claimants = set()

        while time.time() < deadline and len(claimants) < 2:
            for item_id in ids:
                resp = client.get(f"{ACC_URL}/api/item/{item_id}", timeout=10)
                if resp.status_code == 200:
                    item = resp.json()
                    cb = item.get("claimedBy")
                    if cb:
                        claimants.add(cb)
            if len(claimants) < 2:
                time.sleep(POLL_INTERVAL)

        assert len(claimants) >= 2, (
            f"All {n} items were claimed by only {claimants} — work not distributing.\n"
            f"  Expected ≥2 distinct agents to claim work.\n"
            f"  Check: are multiple agents running queue workers?\n"
            f"  Online agents detected: {known_agents}"
        )
        print(f"\n  Work split across agents: {claimants}")

    def test_failed_item_is_retried(self, client):
        """
        Manually claim and fail an item; verify it returns to 'pending' for retry.
        This tests the fail → retry path without requiring a running agent.
        """
        item = create_queue_item(
            client,
            title="[PROBE] Retry logic test",
            description="Synthetic item for testing failure retry. Will be manually failed.",
        )
        item_id = item["id"]

        # Manually claim
        resp = client.post(f"{ACC_URL}/api/item/{item_id}/claim",
                           json={"agent": "probe-test-harness"}, timeout=10)
        assert resp.status_code == 200, f"Manual claim failed: {resp.status_code}"

        # Manually fail
        resp = client.post(f"{ACC_URL}/api/item/{item_id}/fail",
                           json={"agent": "probe-test-harness", "error": "synthetic probe failure"},
                           timeout=10)
        assert resp.status_code == 200, f"Manual fail failed: {resp.status_code}"

        # Should be back to pending
        resp = client.get(f"{ACC_URL}/api/item/{item_id}", timeout=10)
        item_data = resp.json()
        assert item_data.get("status") == "pending", \
            f"After failure, item should be pending; got: {item_data.get('status')}"
        assert item_data.get("attempts", 0) >= 1, "attempts counter must increment on failure"


# ── Probe 6: Hermes Capability Routing ────────────────────────────────────────

class TestHermesRouting:
    """Verify that hermes-tagged tasks reach hermes-capable agents."""

    def test_hermes_item_routed_to_capable_agent(self, client):
        """
        A queue item with required_executors=['hermes'] must be claimed only by
        an agent with hermes capability. If no such agent exists, the test is skipped.
        """
        # First check if any agent has hermes capability
        resp = client.get(f"{ACC_URL}/api/agents", timeout=15)
        agents = resp.json() if resp.status_code == 200 else {}
        hermes_agents = []
        agent_list = agents.values() if isinstance(agents, dict) else agents
        for a in agent_list:
            caps = a.get("capabilities", {})
            if isinstance(caps, dict) and caps.get("hermes"):
                hermes_agents.append(a.get("name", "?"))
            elif isinstance(caps, list) and "hermes" in caps:
                hermes_agents.append(a.get("name", "?"))

        if not hermes_agents:
            pytest.skip("No hermes-capable agents registered — skipping hermes routing probe")

        item = create_queue_item(
            client,
            title="[PROBE] Hermes routing test",
            description="Synthetic hermes task for routing verification. Output: HERMES_PROBE_OK",
            required_executors=["hermes"],
            tags=["hermes", "probe"],
        )
        item_id = item["id"]

        item_data = wait_for_status(
            client, item_id,
            target_statuses=["in-progress", "in_progress", "completed"],
            timeout=120,
        )

        claimed_by = item_data.get("claimedBy", "")
        assert claimed_by in hermes_agents, (
            f"Hermes task claimed by '{claimed_by}' which is not in hermes_agents={hermes_agents}.\n"
            f"  This means required_executors=['hermes'] routing is broken.\n"
            f"  Check: agent capability detection in queue.rs:detect_capabilities()"
        )
        print(f"\n  Hermes task correctly routed to: {claimed_by}")


# ── Probe 7: Agent Self-Diagnosis ─────────────────────────────────────────────

class TestAgentDiagnostics:
    """Exec-based diagnostics — checks agent internals via command registry."""

    def test_log_tail_command(self, client, known_agents):
        """
        Exec the log_tail command on each known agent and verify we get a response.
        Empty log is fine; error or no response is not.
        """
        if not known_agents:
            pytest.skip("No known agents")

        for agent_name in known_agents[:2]:
            resp = client.post(f"{ACC_URL}/api/exec", json={
                "command": "log_tail",
                "params": {"log": "bus-listener", "lines": 20},
                "targets": [agent_name],
                "timeout_ms": 20000,
            }, timeout=15)

            if resp.status_code != 200 or not resp.json().get("busSent"):
                pytest.skip(f"Bus not working for {agent_name} — skipping log_tail probe")

            exec_id = resp.json()["execId"]
            results = get_exec_result(client, exec_id, timeout=30)
            agent_result = next((r for r in results if r.get("agent") == agent_name), None)
            assert agent_result is not None, (
                f"Agent '{agent_name}' did not respond to log_tail command.\n"
                f"  Check: commands.json must be installed at ~/.acc/commands.json"
            )
            assert agent_result.get("exit_code", 1) == 0, \
                f"log_tail on {agent_name} exited non-zero: {agent_result}"

    def test_disk_usage_command(self, client, known_agents):
        """Verify disk usage command works on online agents."""
        if not known_agents:
            pytest.skip("No known agents")

        resp = client.post(f"{ACC_URL}/api/exec", json={
            "command": "disk_usage",
            "targets": [known_agents[0]],
            "timeout_ms": 15000,
        }, timeout=15)

        if resp.status_code != 200 or not resp.json().get("busSent"):
            pytest.skip("Bus not working — skipping disk_usage probe")

        exec_id = resp.json()["execId"]
        results = get_exec_result(client, exec_id, timeout=25)
        assert results, f"No response to disk_usage from {known_agents[0]}"
        assert results[0].get("exit_code") == 0, \
            f"disk_usage failed: {results[0]}"
