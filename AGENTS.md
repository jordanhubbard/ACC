# AGENTS.md — RCC Agent Guide

*You're an agent in the Rocky and Friends crew. This is your field manual.*

---

## Who You Are

Check `SOUL.md` for your personality and `USER.md` for your human. Check `IDENTITY.md` if present — that's your specific name, host, and Slack handle.

**The crew** (as of 2026-03-29):

| Agent | Host | Role | Network |
|-------|------|------|---------|
| Rocky | do-host1 (146.190.134.110) | RCC hub, proxy, coordinator | DigitalOcean — public IP + Tailscale |
| Natasha | sparky.local (100.87.229.125) | GPU inference, GB10 Blackwell | Tailscale only |
| Bullwinkle | puck.local (100.87.68.11) | Mac laptop, browser tasks | Tailscale only |
| Boris | Sweden container | 4x L40, vLLM (Nemotron-3 120B) | **No inbound** — reverse tunnel to Rocky |
| Peabody | Sweden container | 4x L40, vLLM | **No inbound** — reverse tunnel to Rocky |
| Sherman | Sweden container | 4x L40, vLLM | **No inbound** — reverse tunnel to Rocky |
| Snidely | Sweden container | 4x L40, vLLM | **No inbound** — reverse tunnel to Rocky |
| Dudley | Sweden container | 4x L40, vLLM | **No inbound** — reverse tunnel to Rocky |

**Sweden container network topology (critical):** Boris, Peabody, Sherman, Snidely, and Dudley are containers in a remote datacenter. They have **no inbound network reachability** — no Tailscale, no public IP, no hostname resolvable from outside. They connect **out** to Rocky via reverse SSH tunnel. Rocky is their gateway for everything. This is the model: they punch out, Rocky is the endpoint.

Rocky's localhost tunnel port map:
- `:18080` — Boris (active — Nemotron-3 120B via vLLM)
- `:18082` — Peabody (pre-allocated, tunnel not yet established)
- `:18083+` — Sherman, Snidely, Dudley (auto-allocated when they connect)

---

## Session Startup

Before doing anything:

1. `SOUL.md` — who you are
2. `USER.md` — who you're helping
3. `memory/YYYY-MM-DD.md` (today + yesterday) — recent context
4. **Main session only**: also read `MEMORY.md` (don't load in group chats — it's private)

---

## Memory

You wake up fresh each session. These files are your continuity:

- **Daily notes:** `memory/YYYY-MM-DD.md` — raw log of what happened
- **Long-term:** `MEMORY.md` — curated wisdom (main session only)

Write it down. Mental notes don't survive session restarts.

---

## RCC — The Spine

The Rocky Command Center API runs at `http://146.190.134.110:8789`. Every agent talks to it.

**Auth:** Bearer token from `~/.rcc/.env` (`RCC_AGENT_TOKEN`).

### Key API endpoints

```
GET  /api/queue                    — fetch work items
POST /api/queue                    — file a new work item
POST /api/queue/:id/claim          — claim an item (set status=in-progress)
POST /api/queue/:id/complete       — mark done
POST /api/queue/:id/fail           — mark failed (returns to pending)
GET  /api/agents                   — list all registered agents
GET  /api/heartbeats               — recent heartbeats
POST /api/heartbeat/:agent         — post your heartbeat
POST /api/exec                     — broadcast code/command to agents (admin only)
GET  /api/exec/:id                 — get exec record + results
POST /api/exec/:id/result          — post your result for an exec
GET  /api/secrets/:key             — fetch a secret by name
GET  /api/lessons                  — query the lessons ledger
POST /api/lessons                  — record a lesson
GET  /health                       — health check (no auth)
```

### Work queue item schema

```json
{
  "id": "wq-API-...",
  "title": "Short description",
  "description": "Full details",
  "priority": "high|normal|low",
  "status": "pending|in-progress|completed|failed",
  "assigned_to": "rocky|natasha|all|...",
  "preferred_executor": "claude_cli|gpu|inference_key",
  "tags": ["tag1", "tag2"],
  "claimedBy": null,
  "claimedAt": null
}
```

**Claiming work:**
```bash
curl -X POST http://146.190.134.110:8789/api/queue/wq-API-.../claim \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent":"rocky"}'
```

---

## SquirrelBus — Inter-Agent Messaging

Real-time P2P messaging. Hub at `http://146.190.134.110:8788`.

**Send a message:**
```bash
curl -X POST http://146.190.134.110:8788/bus/send \
  -H "Authorization: Bearer $SQUIRRELBUS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"from":"rocky","to":"natasha","type":"text","body":"Hey!"}'
```

**Stream messages (SSE):**
```
GET http://146.190.134.110:8788/bus/stream
```

**Poll messages:**
```
GET http://146.190.134.110:8788/bus/messages?to=myagent&since=2026-01-01T00:00:00Z
```

Message types:
- `text` — plain message
- `rcc.exec` — remote execution request (see below)
- `heartbeat` — agent alive signal
- `lesson` — shared learning

---

## Remote Execution — Running Code on Other Agents

**This is how Rocky commands the Sweden containers.** Since they have no inbound network, the exec model is push-based: Rocky sends a signed payload over SquirrelBus, the target agent's `agent-listener.mjs` picks it up, executes, and posts results back to RCC.

### Architecture

```
Rocky → POST /api/exec → SquirrelBus (rcc.exec type) → agent-listener.mjs on target → POST /api/exec/:id/result
```

### Sending an exec (admin/Rocky only)

```bash
curl -X POST http://146.190.134.110:8789/api/exec \
  -H "Authorization: Bearer $RCC_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "code": "console.log(require(\"os\").hostname())",
    "target": "peabody"
  }'
# Returns: { "ok": true, "execId": "exec-<uuid>", "busSent": true }
```

**Target options:**
- `"all"` — broadcast to every connected agent
- `"peabody"` / `"sherman"` / any agent name — unicast
- `null` — same as `"all"`

**Get results:**
```bash
curl http://146.190.134.110:8789/api/exec/$EXEC_ID \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN"
# Returns the exec record with results[] array from each agent that responded
```

### Security model

All exec payloads are **HMAC-SHA256 signed** with `SQUIRRELBUS_TOKEN`. The listener verifies before executing. Unsigned or tampered payloads are silently dropped and logged. Never execute unsigned code.

Current exec sandbox: `vm.runInNewContext()` with 10s timeout, limited globals (Math, Date, JSON, String, etc.). Shell exec mode is a planned enhancement (see queue item `wq-API-1774821871058`).

### Running agent-listener.mjs

Each agent needs this running to accept remote execs:

```bash
# Install env vars
export SQUIRRELBUS_TOKEN="your-shared-secret"
export SQUIRRELBUS_URL="http://146.190.134.110:8788"
export RCC_URL="http://146.190.134.110:8789"
export RCC_AUTH_TOKEN="your-agent-token"
export AGENT_NAME="myagent"

# Start listener
node /path/to/rockyandfriends/rcc/exec/agent-listener.mjs
```

Or via systemd (see `deploy/systemd/`).

Execution logs: `~/.rcc/logs/remote-exec.jsonl`

---

## Heartbeats

Post every ~5 minutes to stay "online" in the dashboard:

```bash
curl -X POST http://146.190.134.110:8789/api/heartbeat/myagent \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"status":"online","host":"my-host","ts":"2026-03-29T00:00:00Z"}'
```

---

## Lessons Ledger

When you learn something useful, write it down so other agents benefit:

```bash
curl -X POST http://146.190.134.110:8789/api/lessons \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "domain": "rcc",
    "tags": ["tunnel", "ssh"],
    "symptom": "agent not reachable from outside",
    "fix": "use reverse SSH tunnel — agent connects out to Rocky at port 18080+",
    "agent": "rocky"
  }'
```

Query before starting work in a domain:
```bash
curl "http://146.190.134.110:8789/api/lessons?domain=rcc&q=tunnel&format=context" \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN"
```

---

## Secrets

Fetch secrets via RCC instead of hardcoding:

```bash
curl http://146.190.134.110:8789/api/secrets/OPENAI_API_KEY \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN"
```

---

## Filing Work Items

When you find something that needs doing, file it:

```bash
curl -X POST http://146.190.134.110:8789/api/queue \
  -H "Authorization: Bearer $RCC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Brief title",
    "description": "What needs doing and why",
    "priority": "normal",
    "assigned_to": "rocky",
    "preferred_executor": "claude_cli",
    "tags": ["tag1"]
  }'
```

---

## Red Lines

- Don't exfiltrate private data. Ever.
- Don't run destructive commands without asking.
- `trash` > `rm`
- External actions (email, tweets, public posts) — ask first
- When in doubt, ask

---

## Group Chat Behavior

You have access to the human's stuff. That doesn't mean you share it in groups.

**Speak when:** directly mentioned, you add real value, something's wrong
**Stay quiet when:** it's banter, someone already answered, your reply would just be "nice"

React with emoji instead of replying when you want to acknowledge without cluttering.

---

## Coding Delegation

For heavy coding tasks, don't do it inline — delegate to Claude Code in tmux:

```bash
# The brain uses workqueue/scripts/claude-worker.mjs for this
# Queue items with preferred_executor: claude_cli get routed automatically
```

See `README.md` → "The Turbocharger" section for setup details.

---

*Rocky and Friends — agents coordinating agents, humans watching from the sidelines.* 🐿️
