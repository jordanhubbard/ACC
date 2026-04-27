# Spec: CLI-First Session Routing and Task Plane Unification

Spec index:
- `docs/specs/README.md`

## Objective

ACC currently mixes several orchestration models:
- fleet SQL tasks in `/api/tasks`
- legacy queue items in `/api/queue`
- ad hoc remote execution in `/api/exec`
- coding work executed either by API loops or one-shot CLI subprocesses

That fragmentation no longer matches the intended operating model.

This spec re-centers ACC around four decisions:

1. **`/api/tasks` is the only durable task plane**
2. **Persistent CLI sessions are the default coding executor path**
3. **Executor choice is distinct from agent affinity**
4. **API providers are secondary for coding; primary for coordination and light reasoning**

The result should be a simpler control plane that treats Claude, Codex, Cursor, and similar coding CLIs as first-class, node-local resources managed through tmux/PTTY-backed sessions with explicit capacity limits and health reporting.

---

## Context

The current codebase contains partial versions of the desired model:
- docs describe a persistent tmux-backed CLI workflow
- queue schema lists multiple executor types
- dispatch and agent code carry some capability concepts

But the dominant implementation is still inconsistent:
- `preferred_executor` is overloaded to sometimes mean agent name
- several coding paths are one-shot subprocess calls with short timeouts
- the server does not use one canonical executor/session registration shape
- agents do not track live CLI sessions as managed resources

This spec closes those gaps.

---

## Core Decisions

### D1 — One durable work plane

`/api/tasks` is the canonical, durable, multi-agent work system.

Use:
- `/api/tasks` for all durable work
- `/api/exec` only for operator-issued remote commands
- `/api/queue` only as a compatibility layer during migration

No new scheduling or routing features should be built on `/api/queue`.

### D2 — CLI-first coding

For coding tasks, the preferred execution path is a persistent CLI session on an agent node:
- Claude Code
- Codex
- Cursor
- other compatible coding CLIs as added later

These sessions may:
- run for a long time
- span many turns
- be bound to a project or workspace
- exist multiple times on one machine

They must therefore be treated as managed local resources, not just binaries found in `PATH`.

### D3 — Separate executor type from node affinity

The task model must distinguish:
- **executor preference**: what type of tool should do the work
- **agent affinity**: which node should do the work, if any

`preferred_executor` and `required_executors` are for executor class only.
Agent affinity must use distinct fields such as:
- `preferred_agent`
- `assigned_agent`
- `assigned_session`

### D4 — Session-aware routing

Dispatch must route by live readiness, not static claims.

Relevant routing signals:
- agent online/offline
- executor installed
- executor authenticated
- session exists
- session busy/idle/stuck/dead
- free session slots
- memory headroom
- current task load
- optional project/session affinity

### D5 — API providers are secondary for coding

Anthropic/OpenAI-compatible API providers remain useful, but primarily for:
- dispatch reasoning
- summarization
- task decomposition
- lightweight review
- coordination when no CLI is available

They are not the default path for long coding tasks when a ready CLI session exists.

### D6 — GPU/vLLM is optional, not baseline

GPU-backed local serving remains an optional executor class.
It should not shape the baseline scheduling model until it is operationally reliable enough to be trusted fleet-wide.

---

## Canonical Data Model

### Task fields

Add or normalize the following fields on durable tasks:

```json
{
  "preferred_executor": "claude_cli",
  "required_executors": ["claude_cli", "codex_cli"],
  "preferred_agent": "bullwinkle",
  "assigned_agent": null,
  "assigned_session": null
}
```

Rules:
- `preferred_executor` is a soft hint
- `required_executors` is a hard filter
- `preferred_agent` is a soft node-affinity hint
- `assigned_agent` is set by the server when work is routed or claimed
- `assigned_session` may be set when a specific project-affine session is selected

### Agent registration

Agents publish a canonical live registration shape:

```json
{
  "agent": "bullwinkle",
  "host": "bullwinkle.local",
  "executors": [
    {
      "type": "claude_cli",
      "installed": true,
      "auth_state": "ready"
    },
    {
      "type": "codex_cli",
      "installed": true,
      "auth_state": "ready"
    },
    {
      "type": "cursor_cli",
      "installed": true,
      "auth_state": "unauthenticated"
    }
  ],
  "sessions": [
    {
      "name": "claude-main",
      "executor": "claude_cli",
      "project_id": "proj-abc123",
      "state": "idle",
      "busy": false,
      "stuck": false,
      "last_activity": "2026-04-27T18:00:00Z",
      "estimated_ram_mb": 2800
    }
  ],
  "capacity": {
    "max_sessions": 4,
    "free_session_slots": 1,
    "tasks_in_flight": 2,
    "estimated_free_slots": 1
  }
}
```

### Session states

Canonical states:
- `idle`
- `busy`
- `stuck`
- `dead`
- `unauthenticated`

`busy` and `stuck` may coexist with executor metadata, but routing should consider only `idle` and healthy sessions as ready for new work.

---

## Architecture Changes

### Server

The server becomes responsible for:
- durable task storage and claiming
- session-aware dispatch
- heartbeat/session telemetry storage
- dashboard visibility
- legacy compatibility gates during migration

It must stop inferring routing primarily from legacy boolean capability maps.

### Agent

The default agent runtime becomes:
- task worker
- bus listener
- session manager

Optional:
- Hermes gateway
- operator proxy
- legacy queue worker

The session manager is responsible for:
- discovering tmux-backed CLI sessions
- determining readiness and health
- spawning new sessions within policy
- reporting local session telemetry

### Executors

Coding executors should use persistent session adapters, not one-shot subprocesses, for the default path.

Required behaviors:
- send work into a named session
- wait for idle prompt or equivalent completion signal
- support background jobs for very long tasks
- capture output
- classify auth failure, stuck, interruption, and timeout cases

---

## Feature Set and Acceptance Criteria

### F1 — Canonical task-plane declaration
- Docs and runtime defaults identify `/api/tasks` as the durable work plane
- `/api/queue` is explicitly marked compatibility-only
- `/api/exec` is documented as operator-only

### F2 — Executor/agent semantic cleanup
- `preferred_executor` is never used as an agent identifier
- a separate agent-affinity field exists and is used in dispatch/claim logic

### F3 — Canonical live registration
- agents can publish one canonical executor/session shape
- legacy `capabilities` payloads are normalized during migration

### F4 — Local session registry
- each agent maintains discoverable state for local coding sessions
- session state survives process restart where appropriate

### F5 — Session discovery and health
- tmux-backed Claude/Codex/Cursor sessions can be discovered
- sessions can be labeled `idle`, `busy`, `stuck`, `dead`, or `unauthenticated`

### F6 — Bounded spawning and admission control
- an agent can cap total session count and per-executor count
- session spawn is denied under configured memory-pressure conditions

### F7 — Persistent CLI adapters
- Claude, Codex, and Cursor can run through persistent session adapters
- one-shot subprocess mode remains only as explicit fallback/debug behavior

### F8 — Session-aware routing
- dispatch considers live executor/session readiness and free slots
- coding work prefers ready CLI sessions over API coding loops

### F9 — Heartbeat/session telemetry
- heartbeats expose task load, free capacity, and session details consistently
- keepalive heartbeats do not drop capacity fields

### F10 — Migration safety
- legacy queue and exec paths remain observable during cutover
- operators can see session and capacity state in the dashboard
- runbooks cover auth expiry, stuck sessions, and spawn denial

---

## New or Modified Files

### New

| Path | Purpose |
|------|---------|
| `docs/specs/cli-first-session-routing.md` | This spec |
| `agent/acc-agent/src/session_registry.rs` | Local session state |
| `agent/acc-agent/src/session_discovery.rs` | tmux/process discovery |

### Modified

| Path | Change |
|------|--------|
| `acc-server/src/dispatch.rs` | Route by executor/session readiness and capacity |
| `acc-server/src/routes/agents.rs` | Canonical registration + session-aware heartbeats |
| `acc-server/src/routes/tasks.rs` | Separate executor preference from agent affinity |
| `acc-server/src/dashboard.html` | Session and capacity views |
| `agent/acc-agent/src/tasks.rs` | Use session manager and publish consistent telemetry |
| `agent/acc-agent/src/queue.rs` | Legacy compatibility only or optional path |
| `agent/acc-agent/src/worker.rs` | Simpler default runtime |
| `workqueue/executors/claude-cli.mjs` | Persistent session path |
| `workqueue/executors/codex.mjs` | Persistent session path |
| `workqueue/executors/cursor.mjs` | Persistent session path |
| `workqueue/SCHEMA.md` | Canonical task semantics |

---

## Configuration

Suggested config additions:

| Var | Purpose |
|-----|---------|
| `ACC_MAX_SESSIONS` | Max concurrent coding sessions on this node |
| `ACC_MAX_SESSIONS_PER_EXECUTOR` | Optional cap per executor class |
| `ACC_MIN_FREE_RAM_MB` | Minimum free RAM required to spawn a new session |
| `ACC_SESSION_STUCK_SECS` | Threshold before a busy session is marked stuck |
| `ACC_ENABLE_LEGACY_QUEUE` | Keep legacy queue worker enabled during migration |
| `ACC_ENABLE_ONE_SHOT_EXECUTOR_FALLBACK` | Allow old subprocess mode as fallback/debug |

---

## Rollout Plan

### Phase 1 — Semantic cleanup
- document canonical task plane
- split executor type from agent affinity
- stop writing agent names into `preferred_executor`

### Phase 2 — Canonical registration
- add canonical executor/session registration payload
- normalize legacy capability payloads

### Phase 3 — Session manager
- add local session registry
- add tmux discovery
- add health and auth-state classification
- add spawn limits and memory admission control

### Phase 4 — Persistent CLI execution
- land shared session adapter
- move Claude to session-backed default
- add Codex and Cursor session support

### Phase 5 — Dispatch cutover
- route coding work by live session readiness
- prefer CLI sessions over API coding loops
- demote GPU/vLLM from baseline assumptions

### Phase 6 — Simplify runtime and de-risk operations
- reduce default worker set
- add dashboard session views
- add migration/runbook docs
- freeze legacy queue feature growth

---

## Testing Strategy

- Unit tests for schema normalization
- Unit tests for dispatch selection using executor/session readiness
- Unit tests for session-state classification
- Integration tests for:
  - agent registration with live sessions
  - heartbeat updates preserving capacity data
  - task routing to a ready CLI session
  - spawn denial under memory pressure
- Manual smoke tests for:
  - Claude/Codex/Cursor session discovery
  - busy/stuck/dead detection
  - dashboard visibility

---

## Boundaries

| Always | Ask first | Never |
|--------|-----------|-------|
| Treat `/api/tasks` as the canonical durable work plane | Auto-spawn many sessions on a node with unknown RAM headroom | Overload `preferred_executor` to mean an agent name |
| Prefer ready CLI sessions for coding work | Enabling GPU/vLLM as a default coding path again | Add new durable scheduling features to `/api/queue` |
| Publish live executor/session readiness in heartbeats | Automatic session restart after repeated auth failures | Assume a binary in `PATH` is equivalent to a ready coding executor |
| Enforce bounded session spawning | Removing all legacy compatibility in one cutover | Hide session saturation or stuck state from operators |

---

## Success Criteria

This refactor is successful when:
- the server has one clear durable task plane
- coding work normally lands on persistent CLI sessions
- agents know exactly which sessions are alive, idle, busy, stuck, or dead
- routing respects free session capacity and memory headroom
- API providers still help with coordination, but no longer carry the default long-coding path
