# Implementation Plan: CLI-First Session Routing and Task Plane Unification

Source spec: `docs/specs/cli-first-session-routing.md`

Primary goals:
- Make `/api/tasks` the only durable task plane
- Make persistent CLI sessions the default coding executor model
- Separate executor choice from agent affinity
- Add node-local session management, capacity limits, and resource-aware routing

---

## Epic Registry

| Epic | Slug | Purpose |
|------|------|---------|
| E1 | `orchestration-unification` | Collapse onto one durable task plane and remove semantic overloads |
| E2 | `executor-schema` | Define one canonical executor/session registration model |
| E3 | `session-manager` | Add node-local tmux/session discovery, health, and spawning policy |
| E4 | `cli-executors` | Replace one-shot coding subprocesses with persistent CLI session adapters |
| E5 | `routing-and-capacity` | Route by live executor/session readiness and free capacity |
| E6 | `runtime-simplification` | Reduce node runtime sprawl and unify heartbeat semantics |
| E7 | `migration-and-ops` | Roll out safely with compatibility, dashboard visibility, and runbooks |

---

## Dependency Graph

```text
T1 docs-canonical-task-plane
  └── T2 split-executor-from-agent-affinity
        ├── T3 dispatch-and-worker-semantic-cleanup
        └── T4 canonical-agent-registration-schema
              └── T5 client-model-updates

T4 canonical-agent-registration-schema
  └── T6 local-session-registry
        ├── T7 tmux-session-discovery
        ├── T8 session-health-and-stuck-detection
        └── T9 bounded-session-spawning

T7 tmux-session-discovery
  └── T10 shared-pty-tmux-adapter
        ├── T11 claude-session-adapter
        ├── T12 codex-session-adapter
        └── T13 cursor-session-adapter

T5 client-model-updates
  └── T14 session-aware-heartbeats
        └── T15 session-aware-dispatch
              ├── T16 api-secondary-routing-policy
              └── T17 gpu-vllm-demotion

T15 session-aware-dispatch
  └── T18 minimal-agent-runtime
        ├── T19 legacy-compat-freeze-guards
        ├── T20 dashboard-session-views
        └── T21 migration-runbooks
```

---

## Tasks

### T1 — Declare `/api/tasks` as the canonical durable task plane

**Epic:** E1 `orchestration-unification`

**What:**
- Update architecture and operator docs so `/api/tasks` is the only durable work plane
- Mark `/api/queue` as legacy compatibility only
- Mark `/api/exec` as operator-only remote execution, not general task orchestration

**Files:**
- `README.md`
- `ARCHITECTURE.md`
- `docs/acc-executor-design.md`

**Acceptance criteria:**
- Docs no longer describe `/api/queue` as the primary scheduling path
- Docs clearly state coding work defaults to CLI sessions, not API agent loops
- Docs distinguish durable work (`/api/tasks`) from operator commands (`/api/exec`)

---

### T2 — Split executor choice from agent affinity in the task schema

**Epic:** E1 `orchestration-unification`

**What:**
- Keep `preferred_executor` and `required_executors` as executor-type-only fields
- Add a separate field for node affinity, e.g. `preferred_agent` or `assigned_agent`
- Remove all remaining code paths that treat `preferred_executor` as an agent name

**Files:**
- `acc-model/src/task.rs`
- `acc-server/src/routes/tasks.rs`
- `agent/acc-agent/src/tasks.rs`
- `workqueue/SCHEMA.md`

**Acceptance criteria:**
- No code path uses `preferred_executor` to mean an agent name
- Review task fanout uses the new agent-affinity field
- Task docs define executor preference and agent affinity separately

---

### T3 — Clean up dispatch and worker logic to match the new task semantics

**Epic:** E1 `orchestration-unification`

**What:**
- Update dispatch and claim filters to use executor fields for capability matching
- Update agent claim loops to honor `preferred_agent`/`assigned_agent` separately
- Remove overloaded skip logic based on `preferred_executor == <agent-name>`

**Files:**
- `acc-server/src/dispatch.rs`
- `agent/acc-agent/src/tasks.rs`

**Acceptance criteria:**
- Dispatch selects agents by executor compatibility, not overloaded task metadata
- Agent claim logic no longer skips valid work because an executor field contains a peer name
- Existing tests are updated or replaced with new semantics

---

### T4 — Implement a canonical live agent registration schema

**Epic:** E2 `executor-schema`

**What:**
- Add one canonical registration format with live `executors[]` and `sessions[]`
- Normalize legacy `capabilities` and `tool_capabilities` into that format server-side
- Stop relying on undocumented or nonexistent `/api/capabilities` behavior

**Files:**
- `acc-server/src/routes/agents.rs`
- `acc-model/src/agent.rs`
- `docs/specs/cli-first-session-routing.md`

**Acceptance criteria:**
- Server accepts and stores one canonical executor/session payload
- Legacy payloads still work via normalization
- Agent model exposes executor and session data without ad hoc `Value` parsing in common paths

---

### T5 — Update client and model types for executor/session registration

**Epic:** E2 `executor-schema`

**What:**
- Add typed request/response models for executors and sessions
- Update client code to publish and fetch the new shape
- Preserve compatibility where needed during migration

**Files:**
- `acc-model/src/agent.rs`
- `acc-client/src/agents.rs`
- `acc-client/src/items.rs`

**Acceptance criteria:**
- Agent registration and heartbeat code can use typed models for executors/sessions
- Backward-compatible JSON decoding remains in place during rollout
- Integration tests cover old and new registration shapes

---

### T6 — Add a local session registry on each agent

**Epic:** E3 `session-manager`

**What:**
- Implement a persistent node-local registry for coding sessions
- Track executor type, session name, project binding, state, auth readiness, and timestamps

**Files:**
- `agent/acc-agent/src/session_registry.rs` (new)
- `agent/acc-agent/src/main.rs`
- `agent/acc-agent/src/config.rs`

**Acceptance criteria:**
- Agent can persist and reload local session state across restarts
- Registry supports discovered sessions and agent-spawned sessions
- Session state is queryable by other agent modules

---

### T7 — Implement tmux-based session discovery for supported coding CLIs

**Epic:** E3 `session-manager`

**What:**
- Discover active `claude`, `codex`, `cursor`, and `opencode` sessions from tmux
- Determine session identity, executor type, and current coarse state

**Files:**
- `agent/acc-agent/src/session_discovery.rs` (new)
- `agent/acc-agent/src/session_registry.rs`

**Acceptance criteria:**
- Discovery can find supported CLI sessions by tmux pane/process inspection
- Missing tmux or no sessions degrades cleanly
- Discovered sessions are published into the local session registry

---

### T8 — Add session health, stuck detection, and auth-readiness classification

**Epic:** E3 `session-manager`

**What:**
- Distinguish `idle`, `busy`, `stuck`, `dead`, and `unauthenticated`
- Define configurable stuck thresholds and idle-prompt heuristics
- Report unusable sessions without blindly retrying them

**Files:**
- `agent/acc-agent/src/session_registry.rs`
- `agent/acc-agent/src/session_discovery.rs`

**Acceptance criteria:**
- Sessions transition between health states based on observable signals
- Unauthenticated sessions are surfaced distinctly from dead sessions
- Stuck sessions can be identified without manual inspection

---

### T9 — Add bounded session spawning and memory-aware admission control

**Epic:** E3 `session-manager`

**What:**
- Enforce per-agent and per-executor session limits
- Refuse to spawn more sessions when RAM headroom is below threshold
- Expose free session slots and spawn denials as telemetry

**Files:**
- `agent/acc-agent/src/session_registry.rs`
- `agent/acc-agent/src/config.rs`
- `agent/acc-agent/src/tasks.rs`

**Acceptance criteria:**
- Agent enforces configured maximum session counts
- Spawn attempts are denied when memory headroom is insufficient
- Heartbeats expose free session slots and the reason when at capacity

---

### T10 — Implement a shared PTY/tmux session adapter

**Epic:** E4 `cli-executors`

**What:**
- Build a reusable adapter that can:
  - target a named session
  - inject work
  - wait for idle
  - collect output
  - support foreground and background jobs

**Files:**
- `workqueue/scripts/claude-worker.mjs` or Rust replacement
- `docs/specs/cli-first-session-routing.md`

**Acceptance criteria:**
- Adapter supports send/wait/capture for existing tmux-backed coding sessions
- Background execution is supported for long-running jobs
- Session completion detection is test-covered

---

### T11 — Replace Claude one-shot subprocess execution with a persistent session adapter

**Epic:** E4 `cli-executors`

**What:**
- Replace `execFile(... claude ...)` as the default coding path
- Route Claude coding work through the shared session adapter

**Files:**
- `workqueue/executors/claude-cli.mjs`
- `agent/acc-agent/src/tasks.rs`

**Acceptance criteria:**
- Default Claude coding tasks use a persistent tmux-backed session
- One-shot subprocess mode remains optional fallback/debug behavior only
- Long-running coding tasks are no longer capped by the current 300s JS timeout path

---

### T12 — Add persistent Codex session support

**Epic:** E4 `cli-executors`

**What:**
- Support Codex as a persistent session-backed coding executor
- Preserve model/base-url overrides without using one-shot execution as the default

**Files:**
- `workqueue/executors/codex.mjs`
- agent session manager modules

**Acceptance criteria:**
- Codex tasks can run via discovered or spawned persistent sessions
- Codex executor reports auth readiness and busy state through the session registry

---

### T13 — Add persistent Cursor session support

**Epic:** E4 `cli-executors`

**What:**
- Support Cursor as a persistent session-backed coding executor
- Keep it opt-in until stable

**Files:**
- `workqueue/executors/cursor.mjs`
- agent session manager modules

**Acceptance criteria:**
- Cursor tasks can target a persistent session when explicitly requested
- Cursor auth/session readiness is exposed in registration and telemetry

---

### T14 — Publish session-aware heartbeat telemetry

**Epic:** E5 `routing-and-capacity`

**What:**
- Include executor/session readiness, free session slots, and per-session state in heartbeats
- Ensure long-running keepalives preserve capacity fields instead of dropping them

**Files:**
- `agent/acc-agent/src/tasks.rs`
- `agent/acc-agent/src/queue.rs`
- `acc-model/src/queue.rs`
- `acc-server/src/routes/agents.rs`

**Acceptance criteria:**
- Heartbeats consistently include `tasks_in_flight`, `estimated_free_slots`, and session telemetry
- Keepalive heartbeats do not erase capacity state
- Server stores and serves the session-aware heartbeat payload

---

### T15 — Rewrite dispatch to route by live executor/session readiness

**Epic:** E5 `routing-and-capacity`

**What:**
- Dispatch by:
  - online state
  - executor compatibility
  - auth readiness
  - free session slots
  - current load
  - optional project/session affinity
- Stop matching against legacy boolean `capabilities` only

**Files:**
- `acc-server/src/dispatch.rs`

**Acceptance criteria:**
- Coding tasks prefer agents with ready CLI sessions
- Saturated agents are skipped when they report no free session slots
- Existing dispatch tests are updated to the new model

---

### T16 — Make API executors secondary for coding work

**Epic:** E5 `routing-and-capacity`

**What:**
- Change policy so coding tasks default to CLI executors
- Keep API providers for light reasoning, planning, summaries, and non-coding tasks

**Files:**
- `acc-server/src/dispatch.rs`
- `README.md`
- `ARCHITECTURE.md`

**Acceptance criteria:**
- Default coding work is not routed to the in-process Anthropic loop when a ready CLI session exists
- Docs clearly define API providers as secondary for coding and primary for coordination/light work

---

### T17 — Demote GPU/vLLM from baseline scheduling assumptions

**Epic:** E5 `routing-and-capacity`

**What:**
- Treat GPU/vLLM executors as optional classes, not part of the default operating model
- Keep support hooks without letting them shape the main routing semantics

**Files:**
- `README.md`
- `CAPABILITIES.md`
- `workqueue/SCHEMA.md`
- `acc-server/src/dispatch.rs`

**Acceptance criteria:**
- Baseline docs no longer imply GPU/vLLM is a required or default coding path
- Routing defaults do not assume local model-serving capacity exists

---

### T18 — Define and implement a minimal default agent runtime

**Epic:** E6 `runtime-simplification`

**What:**
- Make the default runtime: task worker + session manager + bus listener
- Keep Hermes, gateway, proxy, and legacy queue paths optional

**Files:**
- `agent/acc-agent/src/worker.rs`
- `agent/acc-agent/src/supervise.rs`
- docs

**Acceptance criteria:**
- Default runtime no longer implies every node should run all worker classes
- Node startup docs clearly separate required and optional processes

---

### T19 — Add compatibility freeze guards for legacy queue and exec paths

**Epic:** E7 `migration-and-ops`

**What:**
- Prevent new feature work from landing on `/api/queue`
- Emit warnings/metrics when legacy paths are used
- Document retention window and removal conditions

**Files:**
- `acc-server/src/routes/queue.rs`
- `acc-server/src/routes/exec.rs`
- migration docs

**Acceptance criteria:**
- Legacy path usage is visible in logs or metrics
- New architectural docs treat legacy routes as compatibility-only

---

### T20 — Add dashboard visibility for sessions and capacity

**Epic:** E7 `migration-and-ops`

**What:**
- Show per-agent session list, executor readiness, stuck/dead state, and free slots in the dashboard

**Files:**
- `acc-server/src/dashboard.html`
- supporting agent routes if needed

**Acceptance criteria:**
- Operator can see which CLI sessions exist and whether they are usable
- Saturated or stuck agents are visible without SSHing into the node

---

### T21 — Write migration and operator runbooks

**Epic:** E7 `migration-and-ops`

**What:**
- Add staged migration plan
- Add operator runbooks for:
  - session stuck
  - auth expired
  - memory pressure
  - session spawn denied
  - API fallback mode

**Files:**
- `docs/specs/cli-first-session-routing.md`
- `docs/` runbook markdown files

**Acceptance criteria:**
- Rollout phases are documented from schema-compat to dispatch cutover
- Operators have clear remediation steps for common failure modes

---

## Milestones

### Checkpoint 1 — Semantic cleanup
- [ ] T1 complete
- [ ] T2 complete
- [ ] T3 complete
- [ ] `preferred_executor` no longer means agent name anywhere

### Checkpoint 2 — Session-aware agent model
- [ ] T4 complete
- [ ] T5 complete
- [ ] T6 complete
- [ ] T7 complete
- [ ] T8 complete
- [ ] T9 complete

### Checkpoint 3 — Persistent CLI execution
- [ ] T10 complete
- [ ] T11 complete
- [ ] T12 complete
- [ ] T13 complete

### Checkpoint 4 — Session-aware routing cutover
- [ ] T14 complete
- [ ] T15 complete
- [ ] T16 complete
- [ ] T17 complete

### Checkpoint 5 — Runtime simplification and rollout
- [ ] T18 complete
- [ ] T19 complete
- [ ] T20 complete
- [ ] T21 complete

---

## Smallest Useful Delivery Slice

If the team wants the shortest path to a coherent CLI-first system, ship this slice first:
- T2 — split executor from agent affinity
- T4 — canonical agent registration schema
- T6 — local session registry
- T7 — tmux session discovery
- T9 — bounded session spawning
- T10 — shared PTY/tmux adapter
- T11 — Claude session adapter
- T15 — session-aware dispatch

That slice removes the current semantic ambiguity and establishes persistent CLI sessions as the primary coding path without requiring the full cleanup in one shot.
