#!/usr/bin/env bash
# task-workspace-finalize.sh — Commit and push task workspace on completion.
#
# Enforces the "one push" rule: all changes accumulated during task execution
# are committed to a task branch and pushed exactly once on completion.
# Also syncs the final state back to AccFS shared storage before pushing to git.
#
# Usage:
#   bash deploy/task-workspace-finalize.sh --task-id <id> [--message <msg>]
#
# Environment expected (set by task-workspace-init.sh or queue-worker):
#   TASK_WORKSPACE_LOCAL     — local workspace path
#   TASK_WORKSPACE_SHARED    — AccFS shared path (optional)
#   TASK_BRANCH              — target branch (default: task/<task-id>)
#   AGENT_NAME               — for git author
#
# Exits 0 with result on stdout. Logs progress to stderr.

set -euo pipefail

TASK_ID=""
COMMIT_MSG=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --task-id) TASK_ID="$2"; shift 2 ;;
    --message) COMMIT_MSG="$2"; shift 2 ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -z "$TASK_ID" ]] && { echo "ERROR: --task-id required" >&2; exit 1; }

# Load env
ACC_DIR="${HOME}/.acc"; [[ -d "$ACC_DIR" ]] || ACC_DIR="${HOME}/.ccc"
[[ -f "${ACC_DIR}/.env" ]] && set -a && source "${ACC_DIR}/.env" && set +a

WORKSPACE_LOCAL="${TASK_WORKSPACE_LOCAL:-${ACC_DIR}/task-workspaces/$TASK_ID}"
WORKSPACE_SHARED="${TASK_WORKSPACE_SHARED:-}"
TASK_BRANCH="${TASK_BRANCH:-task/$TASK_ID}"
[[ -z "$COMMIT_MSG" ]] && COMMIT_MSG="task($TASK_ID): complete"

GIT_RESULT="no git action"

# ── Validate workspace exists ─────────────────────────────────────────────────

if [[ ! -d "$WORKSPACE_LOCAL" ]]; then
  echo "⚠ Workspace not found: $WORKSPACE_LOCAL — nothing to finalize" >&2
  echo "$GIT_RESULT"
  exit 0
fi

# ── 1. Final AccFS sync (local → shared) ─────────────────────────────────────

if [[ -n "$WORKSPACE_SHARED" ]] && command -v rsync &>/dev/null; then
  echo "→ Syncing to AccFS: $WORKSPACE_SHARED" >&2
  rsync -a --delete --quiet "$WORKSPACE_LOCAL/" "$WORKSPACE_SHARED/" 2>/dev/null \
    || echo "⚠ Final AccFS sync failed (non-fatal)" >&2
fi

# ── 2. Git push (ONE push, on completion only) ────────────────────────────────

if [[ ! -d "$WORKSPACE_LOCAL/.git" ]]; then
  echo "→ Workspace has no .git — skipping git push" >&2
  GIT_RESULT="workspace is not a git repo"
  echo "$GIT_RESULT"
  exit 0
fi

cd "$WORKSPACE_LOCAL"

# Check for any changes (tracked or untracked)
if git diff --quiet && git diff --staged --quiet && [[ -z "$(git status --porcelain 2>/dev/null)" ]]; then
  echo "→ No changes in workspace — git push skipped" >&2
  GIT_RESULT="workspace clean — no changes to push"
  echo "$GIT_RESULT"
  exit 0
fi

# Create or switch to task branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
if [[ "$CURRENT_BRANCH" != "$TASK_BRANCH" ]]; then
  git checkout -b "$TASK_BRANCH" 2>/dev/null \
    || git checkout "$TASK_BRANCH" 2>/dev/null \
    || { echo "⚠ Could not create branch $TASK_BRANCH" >&2; }
fi

# Stage everything
git add -A

# Commit (idempotent — if nothing staged after add, skip)
if ! git diff --staged --quiet; then
  git \
    -c "user.email=${AGENT_NAME:-ccc-agent}@ccc" \
    -c "user.name=${AGENT_NAME:-ccc-agent}" \
    commit -m "$COMMIT_MSG"
else
  echo "→ Nothing staged after git add -A — skipping commit" >&2
  GIT_RESULT="workspace had no new content to commit"
  echo "$GIT_RESULT"
  exit 0
fi

SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "?")

# ONE git push to task branch
REMOTE_URL=$(git remote get-url origin 2>/dev/null || echo "")
if [[ -z "$REMOTE_URL" ]]; then
  GIT_RESULT="committed locally @ $SHA (no remote configured)"
  echo "⚠ $GIT_RESULT" >&2
else
  echo "→ Pushing to $REMOTE_URL branch=$TASK_BRANCH" >&2
  if git push --force-with-lease origin "$TASK_BRANCH" 2>/dev/null \
     || git push --set-upstream origin "$TASK_BRANCH" 2>/dev/null; then
    GIT_RESULT="pushed to $TASK_BRANCH @ $SHA"
    echo "✓ $GIT_RESULT" >&2
  else
    GIT_RESULT="commit @ $SHA — push failed (check credentials/remote)"
    echo "⚠ $GIT_RESULT" >&2
  fi
fi

echo "$GIT_RESULT"
