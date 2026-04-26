# Spec: Two-Way GitHub ↔ Beads ↔ Fleet Sync

## Objective

Close the loop between external contributors (GitHub) and the ACC agent fleet (beads + fleet tasks).
Today issues flow in one direction only: a human manually runs `bd issues sync` or calls
`POST /api/issues/sync` to pull GitHub issues into a server-side in-memory store.
That store is not connected to beads, and beads issues are not mirrored back to GitHub.

**Target users:**
- External contributors who open GitHub issues and never hear back
- Maintainers who triage issues and want to dispatch work with a label
- Agents who should discover and claim work without human intervention

---

## Core Data Flow

```
GitHub Issue (external)
   │  label: agent-ready
   ▼
POST /api/github/webhook  (or poll via scripts/github-sync.py)
   │
   ▼
acc-server: upsert into issues store + create beads issue (bd create)
   │  if label: agent-ready  →  create fleet task (POST /api/tasks)
   ▼
Dispatch loop: nudge agent, agent claims task
   │
   ▼
Agent completes task  →  POST /api/issues/:id/link (wq_id)
   │                 →  gh issue comment + close (optional)
   ▼
Beads issue closed  →  bd export  →  git push
```

Reverse direction (beads → GitHub):
```
bd create --mirror-to-github
   │
   ▼
gh issue create  →  issue number stored in beads metadata.github_number
   │
   ▼
Future edits/closes propagate via the same webhook or sync script
```

---

## Features & Acceptance Criteria

### F1 — GitHub → beads ingestion (poll)
- `scripts/github-sync.py` polls `gh issue list` for configured repos every N minutes
- New issues not in beads → `bd create` with `source=github`, `github_number=<n>`, `github_repo=<repo>`
- Updated issues (title/body/state changed) → `bd update`
- Closed GitHub issues → `bd close` (if open in beads)
- **AC:** Running the script twice is idempotent; no duplicate beads issues

### F2 — Triage gate: label → fleet task
- When a GitHub issue has the label `agent-ready` (configurable via `GITHUB_DISPATCH_LABEL`), the sync script calls `POST /api/tasks` to create a fleet task
- Fleet task stores `metadata.beads_id` and `metadata.github_number`
- **AC:** A GH issue without the label is ingested into beads but no fleet task is created

### F3 — GitHub webhook (real-time, optional)
- New acc-server route: `POST /api/github/webhook`
- Validates `X-Hub-Signature-256` HMAC with `GITHUB_WEBHOOK_SECRET`
- Handles events: `issues.opened`, `issues.labeled`, `issues.closed`, `issues.edited`
- On `labeled` with `agent-ready` → same logic as F2
- **AC:** Fake webhook payload with correct HMAC is processed correctly; invalid HMAC → 401

### F4 — Fleet task completion → close GitHub issue
- When a fleet task with `metadata.github_number` is marked `completed`, acc-server calls `gh issue close <number>` (or adds a comment) on the linked repo
- Configurable: `GITHUB_AUTO_CLOSE=true` (default false — comment only)
- **AC:** Completing a fleet task with a GitHub link posts a comment; with `AUTO_CLOSE=true` also closes

### F5 — beads → GitHub mirror (opt-in)
- `bd create --mirror` flag creates both a beads issue and a `gh issue create` entry
- The returned GitHub issue number is stored as `metadata.github_number` in the beads issue
- **AC:** `bd create --mirror` creates matching entries in both systems with linked numbers

### F6 — Sync state persistence
- Sync position (last-synced timestamp per repo) stored in `~/.acc/data/github-sync-state.json`
- Survives restarts; poll always resumes from last known position
- **AC:** After a server restart, re-running sync does not re-ingest already-seen issues

### F7 — beads metadata schema for linked issues
Every beads issue that has a GitHub counterpart carries:
```json
{
  "source": "github",
  "github_number": 42,
  "github_repo": "jordanhubbard/ACC",
  "github_url": "https://github.com/jordanhubbard/ACC/issues/42",
  "fleet_task_id": "task-abc123",
  "github_labels": ["bug", "agent-ready"]
}
```

---

## Architecture

### New files
| Path | Purpose |
|------|---------|
| `scripts/github-sync.py` | Poll-based sync daemon / cron script |
| `acc-server/src/routes/github.rs` | Webhook endpoint (F3) |
| `acc-server/src/github.rs` | GitHub API client helpers |

### Modified files
| Path | Change |
|------|--------|
| `acc-server/src/lib.rs` | Add `routes::github` module |
| `acc-server/src/routes/watchdog.rs` | Hook fleet task completion to call GitHub close |
| `acc-server/src/routes/tasks.rs` | On task complete, check `metadata.github_number` |
| `.beads/config.yaml` | Document `github.org`, `github.repo` config keys |

### Env vars
| Var | Default | Purpose |
|-----|---------|---------|
| `GITHUB_REPOS` | `""` | Comma-separated `owner/repo` list to poll |
| `GITHUB_DISPATCH_LABEL` | `agent-ready` | Label that triggers fleet task creation |
| `GITHUB_SYNC_INTERVAL` | `300` | Poll interval in seconds |
| `GITHUB_WEBHOOK_SECRET` | `""` | HMAC secret for webhook validation |
| `GITHUB_AUTO_CLOSE` | `false` | Close GH issue on fleet task completion |
| `GITHUB_TOKEN` | `""` | PAT for API calls (falls back to `gh` CLI auth) |

---

## Tech Stack

- **Poll script:** Python 3, stdlib only + `gh` CLI (already on all fleet nodes)
- **Webhook endpoint:** Rust/Axum, same pattern as existing routes
- **HMAC validation:** `sha2` + `hmac` crates (already in Cargo.toml or trivially added)
- **Fleet task creation:** existing `POST /api/tasks` internal call
- **GitHub writes:** `gh` CLI subprocess (avoids needing an OAuth app)

---

## Testing Strategy

- Unit tests for HMAC validation (valid/invalid signatures)
- Unit tests for sync dedup logic (idempotency)
- Integration test: POST fake webhook payload → assert beads issue created + fleet task created
- Integration test: mark fleet task complete → assert GitHub comment posted (mock `gh` CLI)
- Manual smoke test: open a real GH issue with `agent-ready` label, verify end-to-end

---

## Boundaries

| Always | Ask first | Never |
|--------|-----------|-------|
| Validate webhook HMAC before processing | Auto-close GitHub issues (default off) | Commit GitHub credentials to repo |
| Preserve `wq_id` on issue upsert | Mirror all new beads issues to GitHub | Delete GitHub issues |
| Idempotent sync (no duplicates) | Auto-label GH issues from fleet side | Force-push to external repos |
| Store sync state across restarts | Bulk-dispatch all open GH issues on first run | Expose webhook secret in logs |
