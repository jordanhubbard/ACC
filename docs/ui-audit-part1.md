# ACC UI Audit

**Date:** 2026-04-23  
**Auditor:** boris (review task task-97f86f03cc2a4a09a42c6b6c2f5a86b6)  
**Scope:** All user-facing surfaces in the ACC codebase — Leptos WASM dashboard (port 8788), bootstrap/grievances proxy (`routes/ui.rs`), setup API (`routes/setup.rs`), CLI wizards (`deploy/wizard/`), and the wider `acc-server` route surface as it relates to UI completeness.

---

## 1. UI Surface Inventory

### 1.1 Surfaces That Exist

| Surface | Location | Technology | Port/Path |
|---|---|---|---|
| Leptos WASM dashboard | `dist/` (pre-built, CI-managed) | nginx serving static WASM | 8788 |
| ClawChat SPA | `deploy/nginx/dashboard.conf` `/clawchat/` alias | Leptos WASM (separate build) | 8788/clawchat/ |
| Bootstrap proxy / grievances proxy | `acc-server/src/routes/ui.rs` | Rust/Axum | 8789/api/bootstrap, /grievances |
| AgentBus HTML viewer | `acc-server/src/routes/bus.rs` `bus_viewer()` | Inline server-rendered HTML+JS | 8789/bus/viewer |
| Setup API | `acc-server/src/routes/setup.rs` | Rust/Axum JSON | 8789/api/setup/* |
| Hub setup wizard | `deploy/wizard/hub-setup.sh` | Bash interactive | CLI |
| Agent setup wizard | `deploy/wizard/agent-setup.sh` | Bash interactive | CLI |

### 1.2 Surfaces Referenced in ARCHITECTURE.md But Absent from the Repo

| Referenced Surface | ARCHITECTURE.md Citation | Status |
|---|---|---|
| Pre-built WASM dashboard at `dist/` | "Pre-built WASM dashboard (Leptos/trunk, committed to repo)" | **MISSING** — `dist/` directory does not exist in the repo tree. The docker-compose.yml bind-mounts `./dist:/usr/share/nginx/html:ro` but the directory is absent. A fresh `docker compose up` silently mounts an empty directory. |
| ClawChat SPA at `/clawchat/` | ARCHITECTURE.md §Active Work Areas item 2; nginx.conf `/clawchat/` alias | **MISSING** — nginx.conf references `/usr/share/nginx/clawchat/` but no such build or source directory is present in the repo. |
| `acc-dashboard` service | ARCHITECTURE.md "Services running on CCC: acc-dashboard" | **PARTIAL** — the docker-compose service exists but depends on the missing `dist/` directory. |
| `acc-watchdog` | ARCHITECTURE.md "Services running on CCC: acc-watchdog" | **ABSENT** — no watchdog module, route, or service file exists anywhere in the repo. |
| Rocky CLI (`rocky register`) | ARCHITECTURE.md §Agent Registration Flow | **ABSENT** — no `cli/` directory or `rocky` binary exists. Registration is handled ad-hoc by `deploy/wizard/agent-setup.sh`. |
| `acc-storage` service | ARCHITECTURE.md "Services running on CCC: acc-storage" | **PARTIAL** — replaced by `routes/fs.rs` (local AccFS) with no cloud tier abstraction as described. |

---

## 2. Route-by-Route Auth Guard Audit

The following endpoints are reachable without any authentication token. Each is evaluated against whether open access is intentional.

### 2.1 Intentionally Open (Correct)

| Endpoint | File | Rationale |
|---|---|---|
| `GET /api/health` | `health.rs` | Liveness probe; comment explicitly documents no-auth rationale |
| `GET /api/status` | `health.rs` | Readiness probe; comment explicitly documents no-auth rationale |
| `GET /api/setup/status` | `setup.rs` | First-run detection; needs to be readable before any token exists |
| `POST /api/auth/login` | `auth.rs` | Login endpoint; cannot require a token to obtain a token |

### 2.2 Missing Auth Guards (Bugs)

| Endpoint | File | Severity | Detail |
|---|---|---|---|
| `GET /api/agents` | `agents.rs` | **HIGH** | Lists all registered agents, their hostnames, capabilities, SSH credentials (`ssh_user`, `ssh_host`, `ssh_port`), last-seen timestamps, and API tokens in plain JSON. No auth check. |
| `GET /api/agents/names` | `agents.rs` | **MEDIUM** | Leaks agent name enumeration. No auth check. |
| `GET /api/heartbeats` | `agents.rs` | **MEDIUM** | Returns full heartbeat data for all agents. No auth check. |
| `GET /api/agents/:name` | `agents.rs` | **HIGH** | Returns a single agent record including token. No auth check. |
| `GET /api/agents/:name/health` | `agents.rs` | **MEDIUM** | Returns GPU/RAM telemetry. No auth check. |
| `POST /api/agents/:name/heartbeat` | `agents.rs` | **HIGH** | Any caller can spoof a heartbeat for any agent name, overwriting lastSeen and telemetry. No auth check. Also silently creates new agent records with a generated token if the name is not found. |
| `POST /api/agents/register` | `agents.rs` | **CRITICAL** | Registers a new agent (or re-registers existing) and returns its token. No auth check. Any unauthenticated party on the network can join the fleet and obtain a valid agent token. |
| `POST /api/agents` | `agents.rs` | **CRITICAL** | Thin wrapper over `register_agent`. No auth check. |
| `POST /api/agents/:name` (upsert) | `agents.rs` | **HIGH** | Upserts an agent record. No auth check. |
| `PATCH /api/agents/:name` | `agents.rs` | **HIGH** | Patches agent metadata including decommission status. No auth check. |
| `POST /api/heartbeat/:agent` | `agents.rs` | **HIGH** | Legacy heartbeat path. No auth check. Returns pending work items to caller. |
| `GET /api/queue` | `queue.rs` | **MEDIUM** | Returns entire work queue including all items and completed items. No auth check. |
| `GET /api/queue/stale` | `queue.rs` | **LOW** | Returns stale item list. No auth check. |
| `GET /api/item/:id` | `queue.rs` | **MEDIUM** | Returns single work item. No auth check. |
| `POST /api/queue` | `queue.rs` | **MEDIUM** | Creates work queue items. No auth check. Any caller can enqueue arbitrary work. |
| `POST /api/item/:id/claim` | `queue.rs` | **MEDIUM** | Claims a work item for any agent name. No auth check. |
| `POST /api/item/:id/complete` | `queue.rs` | **MEDIUM** | Marks item complete. No auth check. |
| `POST /api/item/:id/fail` | `queue.rs` | **MEDIUM** | Marks item failed. No auth check. |
| `POST /api/item/:id/keepalive` | `queue.rs` | **LOW** | Keepalive ping. No auth check. |
| `GET /api/projects` | `projects.rs` | **MEDIUM** | Lists all projects. No auth check. |
| `GET /api/projects/:owner/:repo` | `projects.rs` | **MEDIUM** | Returns project detail. No auth check. |
| `GET /api/projects/:id` | `projects.rs` | **MEDIUM** | Returns project by ID. No auth check. |
| `GET /api/projects/:owner/:repo/github` | `projects.rs` | **LOW** | Proxies `gh` CLI output. No auth check. |
| `GET /bus/stream` / `GET /api/bus/stream` | `bus.rs` | **HIGH** | SSE stream of all bus messages. No auth check whatsoever — the `?token=` query param is passed by the JS viewer but the server-side `bus_stream` handler takes no `Query` extractor and calls no `is_authed`. The inline comment in the viewer HTML acknowledges tokens are sent via query string but the handler never reads them. See §3.1 for full analysis. |
| `GET /bus/viewer` / `GET /api/bus/viewer` | `bus.rs` | **LOW** | Returns the self-contained HTML viewer page. Intentionally open per inline comment, but the comment states "No auth required: the dashboard is read-only and relies on the same bearer token" — the page's token validation happens client-side in JS only, which is not server-enforced. |
| `GET /api/acp/sessions` | `acp.rs` | **MEDIUM** | Lists all ACP coding sessions across all agents. No auth check. |
| `GET /api/acp/sessions/:agent` | `acp.rs` | **LOW** | Lists sessions for one agent. No auth check. |
| `GET /api/geek/topology` / `GET /api/mesh` | `geek.rs` | **MEDIUM** | Returns full topology map including internal hostnames, ports, and agent last-seen data. No auth check. |
| `GET /api/geek/stream` | `geek.rs` | **MEDIUM** | SSE stream of all bus events. No auth check. |
| `GET /api/providers` | `providers.rs` | **LOW** | Returns provider URLs and status. No auth check. |
| `GET /api/providers/models` | `providers.rs` | **LOW** | Proxies tokenhub model list. No auth check. |
| `GET /api/supervisor/status` | `supervisor.rs` | **LOW** | Returns supervisor process list with PIDs. No auth check. |
| `GET /api/services` (services_status) | `services.rs` | **LOW** | Returns service health probes. No auth check (implied by absent guard). |
| `GET /api/services/presence` | `services.rs` | **LOW** | Returns service presence. No auth check. |
| `GET /api/issues` | `issues.rs` | **MEDIUM** | Lists GitHub issues. No auth check. |
| `GET /api/issues/:id` | `issues.rs` | **MEDIUM** | Returns issue. No auth check. |
| `PATCH /api/issues/:id` | `issues.rs` | **HIGH** | Mutates issue state. No auth check. |
| `DELETE /api/issues/:id` | `issues.rs` | **HIGH** | Deletes issue. No auth check. |
| `POST /api/issues/sync` | `issues.rs` | **HIGH** | Triggers `gh` CLI sync against a remote repo. No auth check. |
| `POST /api/issues/create-from-wq` | `issues.rs` | **HIGH** | Runs `gh issue create` on behalf of the server. No auth check. |
| `POST /api/issues/:id/link` | `issues.rs` | **MEDIUM** | Links an issue to a work queue item. No auth check. |
| `GET /api/conversations` | `conversations.rs` | **MEDIUM** | Lists all conversations. No auth check. |
| `POST /api/conversations` | `conversations.rs` | **MEDIUM** | Creates a conversation. No auth check. |
| `GET /api/conversations/:id` | `conversations.rs` | **MEDIUM** | Returns a conversation. No auth check. |
| `PATCH /api/conversations/:id` | `conversations.rs` | **MEDIUM** | Mutates conversation metadata. No auth check. |
| `DELETE /api/conversations/:id` | `conversations.rs` | **MEDIUM** | Deletes a conversation. No auth check. |
| `POST /api/conversations/:id/messages` | `conversations.rs` | **HIGH** | Appends a message to a conversation. No auth check. Any caller can inject messages. |
| `GET /api/brain/status` | `brain.rs` | **LOW** | Returns brain queue status. No auth check. |
| `GET /api/bootstrap` | `ui.rs` | **LOW** | Returns CCC/tokenhub URLs when presented with a valid token; 401 otherwise. Token comparison is done but the endpoint is unauthenticated by design. Acceptable. |
| `GET /grievances` / `GET /grievances/*path` | `ui.rs` | **LOW** | Proxies to an external service. No auth check; if the grievances service returns sensitive data that is now exposed without a guard. |
| `GET /api/grievances` / `GET /api/grievances/*path` | `ui.rs` | **LOW** | Same as above for API prefix variant. |
| `GET /api/fs/read` | `fs.rs` | **CRITICAL** | Reads arbitrary files from the AccFS root. No auth check. Any unauthenticated caller can read any file under `/srv/accfs/`. |
| `POST /api/fs/write` | `fs.rs` | **CRITICAL** | Writes arbitrary files to the AccFS root. No auth check. |
| `GET /api/fs/list` | `fs.rs` | **HIGH** | Lists directory contents recursively. No auth check. |
| `HEAD /api/fs/exists` | `fs.rs` | **MEDIUM** | Checks file existence. No auth check. |
| `GET /api/vector/health` | `memory.rs` | **LOW** | Returns Qdrant collection names. No auth check. |

### 2.3 Auth Granularity Gap: No Role Separation for Mutations

The codebase defines two auth methods — `is_authed` (agent token or user token) and `is_admin_authed` (agent token only). However `is_admin_authed` is used only in `auth.rs` for user CRUD. All other mutating endpoints that do have auth checks use `is_authed`, meaning any user token can:

- Register and decommission agents
- Delete projects and tasks
- Trigger remote exec on all fleet nodes via `POST /api/exec`
- Write files to AccFS
- Rotate secrets

ARCHITECTURE.md specifies an `owner` / `collaborator` role distinction ("Only the owner can add/remove agents and other humans") but no such role field exists in the `users` table schema or in any auth check in the codebase.

