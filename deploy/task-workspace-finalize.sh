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

# ---------------------------------------------------------------------------
# CIFS / SMB2 pre-flight helpers
#
# The shared workspace lives on a CIFS-backed AccFS share.  Near-full
# filesystems and mount stalls cause git index writes to enter D-state,
# leaving zero-byte .git/index.lock files.  These helpers detect and
# remediate those conditions before any git write is attempted.
#
# Root cause analysis: docs/git-index-write-failure-investigation.md
# ---------------------------------------------------------------------------

# Minimum free space on the CIFS share before we allow git writes (MiB)
CIFS_FREE_MIB_MIN="${CIFS_FREE_MIB_MIN:-512}"
# Wall-clock seconds for the mount-responsiveness probe
CIFS_PROBE_TIMEOUT="${CIFS_PROBE_TIMEOUT:-5}"

# cifs_preflight <git-root>
# Returns 0 if safe to proceed, 1 if an unrecoverable problem was found.
cifs_preflight() {
  local git_root="$1"
  local git_dir="${git_root}/.git"

  # ── A. Detect CIFS mount ────────────────────────────────────────────────
  local fs_type
  fs_type=$(stat -f --format="%T" "$git_root" 2>/dev/null || echo "unknown")
  local is_cifs=false
  if [[ "$fs_type" == "smb2" ]] \
     || mount 2>/dev/null | grep -qE "type cifs.*${git_root%/*}"; then
    is_cifs=true
  fi

  if [[ "$is_cifs" == "false" ]]; then
    return 0   # not CIFS — no extra checks needed
  fi

  echo "→ [cifs-preflight] CIFS/SMB2 filesystem detected" >&2

  # ── B. Mount responsiveness probe ──────────────────────────────────────
  if ! timeout "${CIFS_PROBE_TIMEOUT}" stat "${git_dir}/config" \
       > /dev/null 2>&1; then
    echo "⚠ [cifs-preflight] CIFS mount is not responding \
(stat timed out after ${CIFS_PROBE_TIMEOUT}s)." >&2
    echo "  The AccFS server may be stalled.  git writes would cause D-state hangs." >&2
    echo "  Run: bash scripts/cifs-mount-health.sh" >&2
    return 1
  fi

  # ── C. Disk-space guard ─────────────────────────────────────────────────
  local free_mib
  free_mib=$(timeout 10 df -BM "$git_root" 2>/dev/null \
    | awk 'NR==2{gsub(/M/,""); print $4}' || echo "")

  if [[ -z "$free_mib" ]]; then
    echo "⚠ [cifs-preflight] Could not determine CIFS free space (df timed out)" >&2
  elif [[ "$free_mib" -lt "$CIFS_FREE_MIB_MIN" ]]; then
    echo "⚠ [cifs-preflight] CIFS share has only ${free_mib} MiB free \
(minimum: ${CIFS_FREE_MIB_MIN} MiB)." >&2
    echo "  Near-full CIFS volumes cause git index writes to stall in D-state." >&2
    echo "  On rocky: run 'juicefs gc redis://127.0.0.1:6379/1' to reclaim space." >&2
    echo "  See: docs/git-index-write-failure-investigation.md" >&2
    return 1
  else
    echo "→ [cifs-preflight] Free space: ${free_mib} MiB ✓" >&2
  fi

  # ── D. Stale lock file cleanup ──────────────────────────────────────────
  for lockfile in "${git_dir}/index.lock" "${git_dir}/HEAD.lock" \
                  "${git_dir}/packed-refs.lock"; do
    if [[ -f "$lockfile" ]]; then
      local lock_size
      lock_size=$(stat --format="%s" "$lockfile" 2>/dev/null || echo "0")
      if [[ "$lock_size" == "0" ]]; then
        echo "⚠ [cifs-preflight] Removing zero-byte stale lock: $lockfile" >&2
        rm -f "$lockfile" || true
      else
        # Non-zero lock: only remove if no other git process is active
        local live_git
        live_git=$(pgrep -ax git 2>/dev/null | grep -v "$$" \
                   | grep "$git_root" || true)
        if [[ -z "$live_git" ]]; then
          echo "⚠ [cifs-preflight] Removing stale lock (no active git process): $lockfile" >&2
          rm -f "$lockfile" || true
        else
          echo "⚠ [cifs-preflight] Lock file exists and git is active — skipping cleanup" >&2
          echo "  $lockfile (size=${lock_size})" >&2
        fi
      fi
    fi
  done

  # ── E. Apply CIFS-safe git config ───────────────────────────────────────
  git -C "$git_root" config --local core.trustctime     false  2>/dev/null || true
  git -C "$git_root" config --local core.checkStat      minimal 2>/dev/null || true
  git -C "$git_root" config --local core.preloadIndex   false  2>/dev/null || true
  git -C "$git_root" config --local index.threads       1      2>/dev/null || true
  git -C "$git_root" config --local gc.auto             0      2>/dev/null || true
  git -C "$git_root" config --local fetch.writeCommitGraph false 2>/dev/null || true
  echo "→ [cifs-preflight] CIFS-safe git config applied ✓" >&2

  return 0
}

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

# ── CIFS pre-flight: mount health + disk space + stale lock cleanup ───────────
# This guards against D-state git hangs on the AccFS CIFS-backed workspace.
# See docs/git-index-write-failure-investigation.md for root-cause analysis.
cifs_preflight "$WORKSPACE_LOCAL" || {
  echo "⚠ CIFS pre-flight failed — aborting git operations to prevent D-state hang" >&2
  echo "  Run 'bash scripts/cifs-mount-health.sh' for diagnostics" >&2
  GIT_RESULT="aborted: CIFS mount unhealthy (disk full or stalled)"
  echo "$GIT_RESULT"
  exit 1
}

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
