# RFC-001: CCC Publishing Layer

**Author:** Bullwinkle  
**Date:** 2026-04-04  
**Status:** APPROVED — v2, incorporating fleet review  
**Triggered by:** jkh asked "How does CCC publish results in a distributed system?"  
**Reviewers:** Dudley (infra audit), Snidely (Sweden perspective), Sherman (lifecycle), Peabody (API contract), Natasha (sequencing), Rocky (daemon owner)

---

## Review Decisions (v2)

All open questions from v1 have been resolved by fleet review in #rockyandfriends:

| # | Question | Decision | Decided by | Rationale |
|---|----------|----------|------------|-----------|
| 1 | URL namespace | **Path-based** (`/agents/{agent}/{name}/`) | Dudley (unanimous) | One cert, one vhost, one Caddy config block. Subdomain opt-in per-publish if root-path isolation needed (e.g. WebSockets at `/`). |
| 2 | Tunnel tech | **Reverse SSH** (`ssh -R`) | Dudley (unanimous) | Already proven by Sweden fleet (5 containers). Boring and correct. WireGuard = mesh overkill, frp = another binary. Need: keepalive + auto-restart. |
| 3 | Sweden fleet tunnels | **Publisher/Host model** | Snidely (framing), Sherman (+1) | Sweden agents are *publishers*, Rocky is the *host*. They push content; Rocky exposes it. Not peers in the serving graph. `via: rocky` is implicit, not a flag. |
| 4 | Static expiry | **7-day TTL default for artifacts, never-expire for named/versioned** | Dudley | Auto-expire-everything breaks shared links; keep-forever-default fills MinIO with junk. Surprising link rot is worse than stale content. |
| 5 | Catalog storage | **SQLite on Rocky, single writer** | Dudley (unanimous) | Agents POST to `/api/publish`, Rocky owns the catalog. JSON files + distributed writes = race condition. |
| 6 | Auto-publish workqueue | **Opt-in per item** via `publish: true` flag | Dudley | Auto-publishing everything exposes half-baked intermediate results. Must be intentional. |
| 7 | Caddy config | **`snippets.d/` directory, one file per publish** | Dudley | Main Caddyfile imports `snippets.d/*.caddy`. Hand-edited 145-line base config stays untouched. Generated routes are isolated. |
| 8 | Auth tiers (v1) | **`public` = open, `fleet` = Tailscale IP range, `private` = explicit allowlist** | Dudley, Snidely | Don't overengineer. No fleet-level auth layer exists today. Tailscale IP check for v1. Token-based auth is v2. |
| 9 | Port allocation | **Dynamic from pool** (19100-19169) | Bullwinkle | Static per-agent ranges don't scale when one agent publishes 12 things and another publishes 1. Rocky's daemon allocates next free port, reclaims on unpublish/death. |
| 10 | Timeout defaults | **Three-tier:** RFC spec < daemon config < publish request | Bullwinkle, Natasha | RFC specifies field and semantics. Daemon config sets operational defaults. Individual publish requests can override. Values tunable without RFC amendment. |
| 11 | Publish type field | **`service` vs `artifact`** | Sherman | Different lifecycle semantics: dedicated SSH-R for long-lived services, ClawBus/POST for artifact dumps. |
| 12 | Supervisord coupling | **Separate units, shared restart policy** | Peabody (Natasha +1) | vLLM tunnel is infrastructure; publish tunnel is optional/user-controlled. Unpublishing a service shouldn't restart vLLM as a side effect. |

---

## Problem Statement

Agents generate artifacts (reports, dashboards, rendered images, status pages, logs, interactive UIs) but have **no standard way to make them accessible** to humans or other agents outside the local network.

Today's reality:
- **Bullwinkle** runs on a Mac mini behind NAT. Anything on `localhost:18789` is invisible outside jkh's LAN.
- **Rocky** has a public IP + Caddy, so can serve things — but only if content is manually pushed there.
- **Sweden fleet** (Boris, Peabody, Sherman, Snidely, Dudley) are outbound-only containers. They can't serve anything directly.
- **Natasha** is on Tailscale Serve (tailnet-only) — reachable within the tailnet but not from the public internet.

When an agent creates something worth sharing, we currently do one of:
1. Upload to Azure Blob (public, static only, no auth)
2. Upload to MinIO on Rocky (internal, needs S3 auth)
3. Ad-hoc reverse SSH tunnels + Caddy patches (fragile, manual)
4. Paste content into Slack/Mattermost (lossy, no structure)

**None of these are a system.** They're duct tape.

---

## Design Goals

1. **Any agent can publish, regardless of network topology** (NAT'd, outbound-only, full VM)
2. **Published artifacts are reachable via a stable URL** that works from anywhere (phone, browser, another agent)
3. **Supports both static and dynamic content** (files, pages, live proxies to running services)
4. **Auth-aware** — some content is public, some is fleet-gated (Tailscale IP allowlist for v1)
5. **Discovery** — there's a catalog of what's been published and by whom
6. **Zero config for the common case** — an agent says "publish this" and gets back a URL
7. **Leverages existing infra** — Rocky's Caddy, Azure Blob, MinIO, Tailscale, ClawBus

---

## Proposed Architecture

### Publisher/Host Model

**Core principle (from fleet review):** Sweden agents are *publishers*, Rocky is the *host*. Publishing from a box with zero inbound is fundamentally `git push`, not `serve`. The RFC makes this contract explicit:

- **Publishers** (all agents): create content, open tunnels, POST to `/api/publish`
- **Host** (Rocky): serves content, manages Caddy routes, runs liveness probes, owns the catalog
- Sweden agents are always Rocky-proxied by definition — `via: rocky` is implicit, not a flag

### Two Publication Types

#### 1. `artifact` — Fire-and-forget content

For rendered images, reports, HTML pages, PDFs, logs, one-shot outputs.

**Transport:** ClawBus POST or SCP — no tunnel required.  
**Content host:** MinIO (survives Rocky downtime — URL is direct to storage).  
**Expiry:** 7-day TTL default. Named/versioned publishes never expire.  
**Port check timeout:** 1s (if MinIO isn't responding in 1s, it's broken, not warming up).

**Flow:**
```
Agent → POST /api/publish {type: "artifact", file, name, visibility}
Rocky uploads to appropriate storage tier:
  - visibility: "public"  → Azure Blob (internet-readable, no auth)
  - visibility: "fleet"   → MinIO agents/ bucket (fleet-readable, S3 auth)
  - visibility: "private" → MinIO agents/{agent}/ (agent-only)
Rocky returns: {url, id, published_at, expires_at}
Rocky indexes in publish catalog (SQLite)
```

**This already mostly works.** Azure Blob and MinIO are operational. We just need the API endpoint and catalog.

#### 2. `service` — Agent-hosted dynamic content via reverse proxy

For live dashboards, interactive UIs, status pages, diagnostic views during training runs.

**Transport:** Dedicated SSH-R tunnel (long-lived).  
**Content host:** Rocky proxy (route goes dark if Rocky is down).  
**Expiry:** Never (explicit unpublish required).  
**Port check timeout:** 30s default (configurable — heavy services may need longer startup).

**Flow:**
```
Agent → opens SSH-R tunnel to Rocky
Agent → POST /api/publish {type: "service", name, port, agent, timeout_s?, visibility?}
Rocky daemon verifies tunnel port (see Sequencing below)
Rocky returns: {status: "registered", url, id, live_at: <estimated_ms>}
Rocky generates snippets.d/{id}.caddy, enqueues Caddy reload
Agent maintains tunnel keepalive
```

### Stream Publishing (via ClawBus)

For live logs, build output, task progress — **this IS ClawBus.** No separate streaming mechanism. Publish to a ClawBus topic, consumers subscribe via existing `/bus/stream` SSE endpoint with topic filtering. Zero new infrastructure.

---

## § Sequencing — Publish/Unpublish Lifecycle

_Merged from Natasha and Peabody's review drafts, with Snidely's timeout budget and Sherman's ordering fix._

### Publish Sequence (`service` type — ordered, blocking)

1. Agent opens SSH-R tunnel to Rocky (port from dynamic pool or pre-allocated)
2. Agent POSTs to `/api/publish` with `{type: "service", port, name, agent, timeout_s}`
3. Rocky daemon verifies tunnel port is accepting connections:
   - Timeout per `timeout_s` field (default: 30s for `service`, overridable per-request)
   - Three-tier precedence: RFC default < daemon config < publish request
   - **Fail-fast** on timeout with clear error: `{"error": "tunnel_port_not_ready", "port": 19100, "timeout_s": 30}`
   - Agent retries explicitly — no silent hangs (Snidely's timeout budget requirement)
4. Port confirmed ready → write catalog entry (SQLite, `status: active`)
5. Generate `snippets.d/{id}.caddy`
6. Enqueue Caddy reload into debounce window
7. **ACK returned to agent after catalog write** — not after reload (reload is async)
   - ACK body: `{"status": "registered", "url": "...", "live_at": <estimated_ms>}` (Peabody's refinement)
   - `live_at` = `now + debounce_window + estimated_reload_time` — Rocky knows debounce queue depth better than the publisher
   - Agent can poll or wait before handing URL to jkh (no DM'ing a link that 404s for 3 seconds)

### Publish Sequence (`artifact` type)

1. Agent POSTs content directly (ClawBus or multipart) — no tunnel
2. Rocky writes to MinIO with TTL tag (7-day default for `artifact`, no-expire for named/versioned)
3. Catalog entry written immediately (no port verify step — content is already on MinIO)
4. Caddy snippet generated only if path routing needed; otherwise serve via existing static handler
5. ACK returned with direct MinIO/Azure URL

### Caddy Reload Debounce

- **2-5 second debounce window** (Rocky's recommendation)
- Any publish that arrives while a reload is pending gets batched into the same wave
- Trades small latency on publish ACK for dramatically less Caddy churn
- On a busy publish day, 10 publishes in 5 seconds = 1 reload, not 10

### Rollback on Reload Failure

If a Caddy reload fails (bad snippet syntax, port conflict):
1. Revert the offending snippet file
2. Re-reload Caddy to restore previous working state
3. Mark the failed publish as `error` in catalog with reason
4. Notify the publishing agent via ClawBus
5. **Never use `caddy reload --force`** — it skips validation. One bad publish must not crater everyone else's routes.

### Unpublish / Liveness Monitoring

**Rocky-side probes are the source of truth.** Agent self-reporting is not trusted ("the fire alarm asks the fire if it's still burning").

1. Rocky daemon runs periodic liveness probes on all `active` service entries (30s interval)
2. Port check fails → mark catalog entry `status: degraded`
   - Caddy route updated to return 503 with `X-Agent-Status: degraded` header (Peabody — better than silent 502)
3. **3 consecutive failures → `status: dead`** (Peabody's escalation model)
   - Caddy snippet removed
   - Batched into next reload window
   - Agent notified via ClawBus: `{"event": "publish_dead", "id": "...", "reason": "3_consecutive_probe_failures"}`
4. Agent can re-publish to restore — same flow as initial publish
5. Tunnel reconnect without re-publish → Rocky re-runs port check → restore to `active` on success (tunnel port is known from catalog)

### Sequencing Ordering (Sherman's race condition fix)

**"Verify then register" — NOT "register then verify."**

If Caddy reloads before the tunnel is live, requests 404 until the tunnel catches up. The daemon verifies the tunnel port is accepting *before* writing the catalog entry. This eliminates the implicit race between tunnel startup and route registration.

Mirror for unpublish: probe fails → degraded → snippet removed → batched reload. Never leave a dead route serving 502s.

---

## The Publish Catalog (SQLite)

Rocky maintains a SQLite database tracking all published artifacts:

```sql
CREATE TABLE publications (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  name TEXT NOT NULL,
  type TEXT NOT NULL CHECK (type IN ('artifact', 'service')),
  url TEXT NOT NULL,
  visibility TEXT NOT NULL CHECK (visibility IN ('public', 'fleet', 'private', 'link')),
  tunnel_port INTEGER,
  status TEXT DEFAULT 'active' CHECK (status IN ('active', 'degraded', 'dead', 'expired', 'error')),
  keep_forever BOOLEAN DEFAULT FALSE,
  timeout_s INTEGER,
  created_at TEXT NOT NULL,
  expires_at TEXT,
  last_seen_at TEXT,
  error_reason TEXT,
  UNIQUE(agent, name)
);
```

**Status transitions:**
```
active ──probe fail──→ degraded ──3 consecutive──→ dead
  ↑                        ↑                         │
  └──probe pass────────────┘                         │
  └──re-publish──────────────────────────────────────┘
  
active ──TTL expired──→ expired (artifact only)
active ──reload fail──→ error
```

**API endpoints:**
- `GET  /api/publish` — list all publications (filterable by agent, type, visibility, status)
- `POST /api/publish` — publish (artifact or service, determined by `type` field)
- `DELETE /api/publish/{id}` — unpublish
- `GET  /api/publish/{id}/status` — health check

**Dashboard integration:** The WASM dashboard gets a "Published" tab showing all active publications with status indicators.

---

## URL Scheme

Published content lives under a predictable, human-readable URL namespace:

```
https://dashboard.yourmom.photos/agents/{agent-name}/{publication-name}/

Examples:
  https://dashboard.yourmom.photos/agents/bullwinkle/webchat/
  https://dashboard.yourmom.photos/agents/natasha/benchmark/
  https://dashboard.yourmom.photos/agents/snidely/training-dashboard/
```

**Subdomain opt-in:** If a publication truly needs root-path isolation (WebSockets at `/`, etc.), the publish request can specify `subdomain: true` — requires DNS challenge TLS in Caddy, handled case-by-case.

**Fallback:** Artifact publications are always accessible via their direct Azure Blob or MinIO URLs, regardless of Caddy/Rocky state.

---

## Port Allocation

### Dynamic Pool (v2 decision)

Rocky's daemon manages a dynamic port pool: **19100-19169** (70 ports).

- On `service` publish: daemon allocates next free port, returns it in ACK
- On unpublish/death: port reclaimed to pool
- Agent establishes SSH-R to the allocated port
- No static per-agent ranges — scales when one agent publishes 12 things and another publishes 1

### Pre-existing Allocations (DO NOT CHANGE)

```
vLLM tunnels:   Boris=18080, Peabody=18081, Sherman=18082, Snidely=18083, Dudley=18084
Shell tunnels:  Peabody=19080, Snidely=19081, Sherman=19082, Dudley=19083, Boris=19084
```

### Port Registry

Canonical source of truth for ALL tunnel port allocations lives on Rocky at:
```
/home/jkh/port-registry.json
```

This replaces the current situation where port assignments are scattered across MEMORY.md files.

---

## Caddy Configuration

### `snippets.d/` Approach (Dudley's recommendation)

Main Caddyfile adds one line:
```
import /etc/caddy/snippets.d/*.caddy
```

Rocky's publish daemon writes one `.caddy` file per active `service` publication:

```
# snippets.d/pub-bullwinkle-webchat.caddy
# Auto-generated by CCC publish daemon — DO NOT EDIT
handle_path /agents/bullwinkle/webchat/* {
    reverse_proxy localhost:19100
}
```

On publish/unpublish: write/remove snippet → enqueue reload (debounced).

**Benefits:**
- Hand-edited base config stays stable
- Generated routes are isolated and auditable
- `git diff` shows exactly what changed
- One bad snippet can be reverted without touching the rest

---

## Sweden Fleet Specifics

Sweden containers (Boris, Peabody, Sherman, Snidely, Dudley) have unique constraints:

- **Zero inbound connectivity** — all traffic flows through reverse SSH tunnels to Rocky
- **Publishing = pushing to Rocky** — they are publishers, not hosts
- **`artifact` type preferred** — compute on Sweden, serve from Rocky/MinIO
- **`service` type possible but discouraged** — latency through SSH tunnel to Rocky adds overhead for interactive UIs
- **Supervisord:** Publish tunnels get separate supervisord units from vLLM tunnels. Shared restart policy, not shared lifecycle. Unpublishing a dashboard must not restart the inference server.
- **Content for `artifact` type lives on MinIO** — stable URL, survives Rocky downtime. Catalog entry points to MinIO directly, not through a Rocky-generated route.

---

## Auth Model (v1)

| Visibility | Who can access | Mechanism |
|-----------|---------------|-----------|
| `public` | Anyone on the internet | No auth (Azure Blob or Caddy open route) |
| `fleet` | Any agent/operator on the tailnet | Tailscale IP allowlist in Caddy |
| `private` | Only the publishing agent + jkh | Agent-specific token |
| `link` | Anyone with the URL (unlisted) | No auth, URL not advertised in catalog |

**v1 simplification:** `fleet` auth = Tailscale source IP check. No token infrastructure needed. Token-based auth is a v2 enhancement if/when external collaborators need fleet access without Tailscale.

---

## Type Semantics Summary

_From Natasha's review, ratified by fleet._

```
| Field                | artifact              | service                    |
|----------------------|-----------------------|----------------------------|
| Transport            | ClawBus POST / SCP    | Dedicated SSH-R            |
| Content host         | MinIO (direct URL)    | Rocky proxy                |
| Port check timeout   | 1s                    | 30s (configurable)         |
| Expiry default       | 7 days                | Never (explicit unpublish) |
| Rocky-down behavior  | URL survives          | Route goes dark            |
| Sweden use case      | Primary               | Discouraged (latency)      |
| Supervisord unit     | N/A                   | Separate from vLLM         |
```

---

## Agent-Side API (the "one-liner")

From any agent's perspective, publishing should be trivial:

```bash
# Artifact (file)
curl -X POST https://rcc.yourmom.photos/api/publish \
  -H "Authorization: Bearer $AGENT_TOKEN" \
  -F "type=artifact" \
  -F "file=@report.html" \
  -F "name=weekly-report" \
  -F "visibility=public"
# Returns: {"url": "https://...", "id": "pub-xxx", "expires_at": "2026-04-11T..."}

# Service (after establishing tunnel)
curl -X POST https://rcc.yourmom.photos/api/publish \
  -H "Authorization: Bearer $AGENT_TOKEN" \
  -d '{"type": "service", "name": "webchat", "port": 19100, "visibility": "fleet"}'
# Returns: {"status": "registered", "url": "https://dashboard.yourmom.photos/agents/bullwinkle/webchat/", "id": "pub-xxx", "live_at": 1712234567890}
```

---

## Tunnel Lifecycle

For `service` type, the tunnel is the critical path:

1. **Agent publishes:** `POST /api/publish` with `type: service`
2. **Rocky allocates port** from dynamic pool (19100-19169)
3. **Agent establishes tunnel:** `ssh -R {allocated_port}:localhost:{local_port} jkh@do-host1`
4. **Rocky verifies port** is accepting (configurable timeout, fail-fast)
5. **Catalog entry written**, snippet generated, reload enqueued
6. **ACK returned** with URL and `live_at` estimate
7. **Ongoing liveness:** Rocky probes every 30s → `active`/`degraded`/`dead`
8. **Tunnel drops:** Agent re-establishes; Rocky auto-recovers on next probe (port is known from catalog)
9. **Agent unpublishes:** `DELETE /api/publish/{id}` — snippet removed, reload enqueued, port reclaimed

**Tunnel keepalive:** Agents use `ServerAliveInterval=30 ServerAliveCountMax=3` in SSH config. Rocky's watchdog checks tunnel health on its own schedule independently.

---

## Implementation Phases

### Phase 1: Artifact Publishing + Catalog (1-2 days)
- Unified `POST /api/publish` endpoint in RCC API
- Upload to Azure Blob (public) or MinIO (fleet/private)
- Publish catalog (SQLite, exposed via API)
- Dashboard "Published" tab (read-only list)
- `port-registry.json` created on Rocky

### Phase 2: Service Publishing + Sequencing (2-3 days)
- Service publish flow with full sequencing (verify → register → snippet → reload)
- Dynamic port allocator
- `snippets.d/` Caddy generation + debounced reload
- Rollback on reload failure
- ACK with `live_at` estimate
- Rocky-side tunnel liveness probes (30s interval, 3-strike escalation)

### Phase 3: Stream Publishing (1 day)
- ClawBus topic filtering extension (`?topic={name}`)
- Integrate with publish catalog

### Phase 4: Polish (ongoing)
- Expiry sweep job (clean up TTL-expired artifacts)
- ClawBus death notifications to publishing agents
- Custom domains per publication
- Token-based fleet auth (replacing Tailscale IP allowlist)
- CLI: `openclaw publish <file>` one-liner
- Publication analytics

---

## What This Replaces

| Before | After |
|--------|-------|
| Manual Azure Blob uploads | `POST /api/publish` |
| Ad-hoc reverse SSH tunnels | Managed tunnel pool + dynamic port allocation |
| Hand-editing Caddyfile | Auto-generated `snippets.d/*.caddy` |
| Pasting to Slack/MM | Publish URL → share link |
| "I can't reach that" | Every publication has a stable URL |
| Port assignments in MEMORY.md | `port-registry.json` on Rocky |
| Silent 502 on dead tunnels | `degraded` → `dead` with 503 + ClawBus notification |

---

## Changelog

### v2 (2026-04-04)
- Incorporated full fleet review from #rockyandfriends thread
- Added Publisher/Host model (Snidely/Sherman)
- Replaced `static`/`page`/`stream` with `artifact`/`service` type system (Sherman)
- Added complete § Sequencing section (merged Natasha + Peabody drafts)
- Added verify-then-register ordering (Sherman)
- Added Caddy reload debounce (Rocky/Snidely)
- Added rollback on reload failure (Bullwinkle)
- Added 3-strike liveness escalation: active → degraded → dead (Peabody)
- Added ACK with `live_at` estimate (Peabody)
- Changed port allocation from static ranges to dynamic pool (Bullwinkle)
- Added three-tier timeout precedence (Bullwinkle/Natasha)
- Added supervisord separation for Sweden fleet (Peabody/Natasha)
- Added `snippets.d/` Caddy approach (Dudley)
- Added Sweden fleet specifics section
- Added type semantics summary table (Natasha)
- Added status transition diagram
- Resolved all 6 original open questions + 6 new ones from review

### v1 (2026-04-04)
- Initial draft with problem statement, three publication modes, tunnel lifecycle, auth model
- 6 open questions for fleet review

---

*Nothing up my sleeve... but there's definitely a publishing layer in there somewhere.* 🫎
