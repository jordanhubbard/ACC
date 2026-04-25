#!/usr/bin/env bash
# task-workspace-init.sh — Bootstrap an isolated AccFS-backed workspace for a task.
#
# Clones the task's git repo into a local workspace, then syncs it to AccFS
# (~/.acc/shared/tasks/<id>/). The queue-worker calls this before handing work
# to claude or hermes. The workspace is the agent's exclusive working copy for
# the duration of the task.
#
# Usage:
#   eval "$(bash deploy/task-workspace-init.sh --task-id <id> [--repo <url>] [--branch <branch>])"
#
# Outputs KEY=VALUE lines suitable for eval. The caller gets:
#   TASK_ID, TASK_WORKSPACE_LOCAL, TASK_WORKSPACE_SHARED, TASK_BRANCH, TASK_REPO

set -euo pipefail

TASK_ID=""
REPO_URL=""
BRANCH="main"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --task-id) TASK_ID="$2"; shift 2 ;;
    --repo)    REPO_URL="$2"; shift 2 ;;
    --branch)  BRANCH="$2"; shift 2 ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -z "$TASK_ID" ]] && { echo "ERROR: --task-id required" >&2; exit 1; }

# Load env
ACC_DIR="${HOME}/.acc"
[[ -d "$ACC_DIR" ]] || ACC_DIR="${HOME}/.ccc"
[[ -f "${ACC_DIR}/.env" ]] && set -a && source "${ACC_DIR}/.env" && set +a

WORKSPACE_LOCAL="${ACC_DIR}/task-workspaces/${TASK_ID}"
mkdir -p "$WORKSPACE_LOCAL"

# ── Determine repo URL ────────────────────────────────────────────────────────

if [[ -z "$REPO_URL" ]]; then
  ACC_WS="${ACC_DIR}/workspace"
  if [[ -d "$ACC_WS/.git" ]]; then
    REPO_URL=$(git -C "$ACC_WS" remote get-url origin 2>/dev/null || echo "")
  fi
fi

# owner/repo shorthand → GitHub HTTPS
if [[ -n "$REPO_URL" && "$REPO_URL" != *"://"* && "$REPO_URL" != *"@"* ]]; then
  REPO_URL="https://github.com/$REPO_URL"
fi

# ── Clone or reuse ────────────────────────────────────────────────────────────

if [[ -d "$WORKSPACE_LOCAL/.git" ]]; then
  echo "→ Reusing workspace at $WORKSPACE_LOCAL" >&2
  git -C "$WORKSPACE_LOCAL" fetch origin 2>/dev/null || true
elif [[ -n "$REPO_URL" ]]; then
  echo "→ Cloning $REPO_URL ($BRANCH) → $WORKSPACE_LOCAL" >&2
  git clone --depth=1 --branch "$BRANCH" "$REPO_URL" "$WORKSPACE_LOCAL" 2>/dev/null \
    || git clone --depth=1 "$REPO_URL" "$WORKSPACE_LOCAL"
  SHA=$(git -C "$WORKSPACE_LOCAL" rev-parse --short HEAD 2>/dev/null || echo "?")
  echo "→ Cloned at $SHA" >&2
else
  echo "→ No repo URL — using empty workspace" >&2
fi

# ── Sync to AccFS shared storage ──────────────────────────────────────────────

WORKSPACE_SHARED=""
ACCFS_TASKS="${ACC_SHARED_DIR:-${ACC_DIR}/shared}/tasks"

if [[ -d "${ACC_SHARED_DIR:-${ACC_DIR}/shared}" ]]; then
  WORKSPACE_SHARED="${ACCFS_TASKS}/${TASK_ID}/workspace"
  mkdir -p "$WORKSPACE_SHARED"
  echo "→ Syncing to AccFS: $WORKSPACE_SHARED" >&2
  rsync -a --delete --quiet "$WORKSPACE_LOCAL/" "$WORKSPACE_SHARED/" 2>/dev/null \
    || echo "⚠ AccFS sync failed (non-fatal)" >&2

  # Write task metadata
  SHA=$(git -C "$WORKSPACE_LOCAL" rev-parse HEAD 2>/dev/null || echo "")
  cat > "${ACCFS_TASKS}/${TASK_ID}/meta.json" << EOF
{"task_id":"${TASK_ID}","repo":"${REPO_URL}","branch":"${BRANCH}","sha":"${SHA}","initiated_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","agent":"${AGENT_NAME:-unknown}"}
EOF
else
  echo "⚠ AccFS not mounted at ${ACC_SHARED_DIR:-${ACC_DIR}/shared} — skipping shared sync" >&2
fi

# ── Output env vars for eval ──────────────────────────────────────────────────

echo "TASK_ID=$TASK_ID"
echo "TASK_WORKSPACE_LOCAL=$WORKSPACE_LOCAL"
echo "TASK_WORKSPACE_SHARED=$WORKSPACE_SHARED"
echo "TASK_BRANCH=task/$TASK_ID"
echo "TASK_REPO=$REPO_URL"
