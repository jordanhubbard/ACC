#!/bin/bash
# acc-repo-sync.sh — Git pull + auto-commit + push for the AccFS shared CCC repo
#
# This runs on exactly ONE node (the designated CCC_REPO_PUSHER, typically Rocky).
# It keeps the shared AccFS repo in sync with GitHub:
#   1. Pull latest from origin (ff-only)
#   2. Auto-commit any local changes (from agents editing files on AccFS)
#   3. Push to origin
#
# Usage:
#   bash deploy/acc-repo-sync.sh           # one-shot
#   ACC_REPO_SYNC_DRY_RUN=1 bash deploy/acc-repo-sync.sh  # dry run
#
# Designed to run via systemd timer or cron every 30 minutes.

set -euo pipefail

ACC_DIR="$HOME/.acc"
ENV_FILE="$ACC_DIR/.env"
LOG_FILE="$ACC_DIR/logs/repo-sync.log"
MAX_LOG_LINES=500

# ---------------------------------------------------------------------------
# CIFS / SMB2 pre-flight
#
# The shared workspace lives on a CIFS-backed AccFS share.  Near-full
# filesystems and mount stalls cause git index writes to enter D-state,
# leaving zero-byte .git/index.lock files and hung processes.
#
# Root cause analysis: docs/git-index-write-failure-investigation.md
# ---------------------------------------------------------------------------
CIFS_FREE_MIB_MIN="${CIFS_FREE_MIB_MIN:-512}"
CIFS_PROBE_TIMEOUT="${CIFS_PROBE_TIMEOUT:-5}"

cifs_repo_preflight() {
  local repo="$1"
  local git_dir="${repo}/.git"

  local fs_type
  fs_type=$(stat -f --format="%T" "$repo" 2>/dev/null || echo "unknown")
  local is_cifs=false
  if [[ "$fs_type" == "smb2" ]] \
     || mount 2>/dev/null | grep -qE "type cifs.*${repo%/*}"; then
    is_cifs=true
  fi
  [[ "$is_cifs" == "false" ]] && return 0

  # Mount responsiveness probe
  if ! timeout "${CIFS_PROBE_TIMEOUT}" stat "${git_dir}/config" \
       > /dev/null 2>&1; then
    log "ERROR: CIFS mount not responding (${CIFS_PROBE_TIMEOUT}s timeout). Aborting sync."
    log "  Run: bash scripts/cifs-mount-health.sh"
    return 1
  fi

  # Disk-space guard
  local free_mib
  free_mib=$(timeout 10 df -BM "$repo" 2>/dev/null \
    | awk 'NR==2{gsub(/M/,""); print $4}' || echo "")
  if [[ -n "$free_mib" ]] && [[ "$free_mib" -lt "$CIFS_FREE_MIB_MIN" ]]; then
    log "ERROR: CIFS share only has ${free_mib} MiB free (< ${CIFS_FREE_MIB_MIN} MiB). Aborting sync."
    log "  On rocky: juicefs gc redis://127.0.0.1:6379/1"
    log "  See: docs/git-index-write-failure-investigation.md"
    return 1
  fi

  # Remove zero-byte stale lock files before any git write
  for lockfile in "${git_dir}/index.lock" "${git_dir}/HEAD.lock" \
                  "${git_dir}/packed-refs.lock"; do
    if [[ -f "$lockfile" ]]; then
      local lock_size
      lock_size=$(stat --format="%s" "$lockfile" 2>/dev/null || echo "0")
      if [[ "$lock_size" == "0" ]]; then
        log "WARNING: Removing zero-byte stale lock: $lockfile"
        rm -f "$lockfile" || true
      fi
    fi
  done

  # Apply CIFS-safe git config
  git -C "$repo" config --local core.trustctime     false  2>/dev/null || true
  git -C "$repo" config --local core.checkStat      minimal 2>/dev/null || true
  git -C "$repo" config --local core.preloadIndex   false  2>/dev/null || true
  git -C "$repo" config --local index.threads       1      2>/dev/null || true
  git -C "$repo" config --local gc.auto             0      2>/dev/null || true
  git -C "$repo" config --local fetch.writeCommitGraph false 2>/dev/null || true

  return 0
}

# Load .env if it exists
if [ -f "$ENV_FILE" ]; then
  set -a
  source "$ENV_FILE"
  set +a
fi

AGENT_NAME="${AGENT_NAME:-unknown}"
DRY_RUN="${ACC_REPO_SYNC_DRY_RUN:-0}"

WORKSPACE="$ACC_DIR/workspace"

if [ ! -d "$WORKSPACE/.git" ]; then
  echo "ERROR: No repo found at $WORKSPACE — run setup-node.sh first" >&2
  exit 1
fi
REPO="$WORKSPACE"

mkdir -p "$(dirname "$LOG_FILE")"

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$AGENT_NAME] [repo-sync] $1" >> "$LOG_FILE" 2>&1
}

# Rotate log
if [ -f "$LOG_FILE" ]; then
  lines=$(wc -l < "$LOG_FILE")
  if [ "$lines" -gt "$MAX_LOG_LINES" ]; then
    tail -n "$MAX_LOG_LINES" "$LOG_FILE" > "${LOG_FILE}.tmp" && mv "${LOG_FILE}.tmp" "$LOG_FILE"
  fi
fi

log "Sync starting (repo: $REPO, dry_run: $DRY_RUN)"

cd "$REPO"

# ── CIFS pre-flight ───────────────────────────────────────────────────────────
# Guard against D-state git hangs on the AccFS CIFS-backed workspace.
# See docs/git-index-write-failure-investigation.md for root-cause analysis.
cifs_repo_preflight "$REPO" || {
  log "ERROR: CIFS pre-flight failed — skipping sync to prevent D-state hang"
  exit 1
}

# ── DNS pre-flight ────────────────────────────────────────────────────────────
# A transient DNS failure causes `git fetch` to exit non-zero with no retry,
# aborting the entire sync.  Probe the remote hostname before touching git so
# we can distinguish "no DNS yet" (retriable) from a real git error
# (permanent).  Mirrors the retry+backoff logic in tasks.rs.
#
# Tunable env vars:
#   DNS_PREFLIGHT_HOST      — hostname to resolve (default: github.com)
#   DNS_PREFLIGHT_RETRIES   — total attempts before giving up (default: 3)
#   DNS_PREFLIGHT_BACKOFF   — seconds to wait between attempts (default: 10)
DNS_PREFLIGHT_HOST="${DNS_PREFLIGHT_HOST:-github.com}"
DNS_PREFLIGHT_RETRIES="${DNS_PREFLIGHT_RETRIES:-3}"
DNS_PREFLIGHT_BACKOFF="${DNS_PREFLIGHT_BACKOFF:-10}"

dns_preflight() {
  local host="$1"
  local max_attempts="$2"
  local backoff="$3"
  local attempt

  for attempt in $(seq 1 "$max_attempts"); do
    # Prefer getent (coreutils, works everywhere); fall back to nslookup then
    # host.  All three probe the system resolver so they catch the same
    # transient failures that would trip `git fetch`.
    if getent hosts "$host" > /dev/null 2>&1 \
       || nslookup "$host" > /dev/null 2>&1 \
       || host "$host" > /dev/null 2>&1; then
      [[ "$attempt" -gt 1 ]] && log "DNS resolved '$host' on attempt $attempt/$max_attempts"
      return 0
    fi

    if [[ "$attempt" -lt "$max_attempts" ]]; then
      log "WARNING: DNS lookup failed for '$host' (attempt $attempt/$max_attempts) — retrying in ${backoff}s"
      sleep "$backoff"
    fi
  done

  log "ERROR: DNS pre-flight failed — could not resolve '$host' after $max_attempts attempt(s). Aborting sync."
  return 1
}

dns_preflight "$DNS_PREFLIGHT_HOST" "$DNS_PREFLIGHT_RETRIES" "$DNS_PREFLIGHT_BACKOFF" || exit 1

# ── Step 1: Pull latest from origin ──────────────────────────────────────────
BEFORE=$(git rev-parse HEAD)
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)

# Retry git fetch with exponential-style backoff (10 s, 20 s) to ride out
# short-lived network glitches, consistent with the push retry loop in
# tasks.rs.  Permanent errors (auth failures, unknown host after DNS passed)
# are surfaced on the first attempt via the error log.
GIT_FETCH_MAX_ATTEMPTS=3
GIT_FETCH_BACKOFFS=(10 20)
_fetch_ok=false
for _attempt in $(seq 1 "$GIT_FETCH_MAX_ATTEMPTS"); do
  if git fetch origin --quiet 2>/dev/null; then
    _fetch_ok=true
    break
  fi
  if [[ "$_attempt" -lt "$GIT_FETCH_MAX_ATTEMPTS" ]]; then
    _wait="${GIT_FETCH_BACKOFFS[$((_attempt - 1))]}"
    log "WARNING: git fetch failed (attempt $_attempt/$GIT_FETCH_MAX_ATTEMPTS) — retrying in ${_wait}s"
    sleep "$_wait"
  fi
done
if [[ "$_fetch_ok" != "true" ]]; then
  log "ERROR: git fetch failed after $GIT_FETCH_MAX_ATTEMPTS attempt(s) — network or auth issue"
  exit 1
fi

if git rev-parse --verify "origin/$CURRENT_BRANCH" --quiet > /dev/null 2>&1; then
  # Stash any local changes before pull to avoid merge conflicts
  STASH_NEEDED=false
  if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
    STASH_NEEDED=true
    git stash --quiet 2>/dev/null || true
    log "Stashed local changes before pull"
  fi

  git merge --ff-only "origin/$CURRENT_BRANCH" --quiet 2>/dev/null || {
    log "WARNING: Fast-forward merge failed — diverged from origin. Skipping pull."
    if [ "$STASH_NEEDED" = true ]; then
      git stash pop --quiet 2>/dev/null || true
    fi
  }

  if [ "$STASH_NEEDED" = true ]; then
    git stash pop --quiet 2>/dev/null || {
      log "WARNING: stash pop had conflicts — check manually"
    }
  fi
fi

AFTER_PULL=$(git rev-parse HEAD)
if [ "$BEFORE" != "$AFTER_PULL" ]; then
  log "Pulled: $BEFORE -> $AFTER_PULL"
else
  log "Already up to date with origin"
fi

# ── Step 2: Auto-commit local changes ────────────────────────────────────────
# Ignore common runtime/temp files
git add -A 2>/dev/null

# Check if there's anything to commit
if git diff --cached --quiet 2>/dev/null; then
  log "No local changes to commit"
else
  CHANGED_FILES=$(git diff --cached --name-only | head -20 | tr '\n' ' ')
  COMMIT_MSG="auto-sync $(date -u +%Y%m%dT%H%M%SZ) [$AGENT_NAME]: $CHANGED_FILES"

  if [ "$DRY_RUN" = "1" ]; then
    log "DRY RUN: Would commit: $COMMIT_MSG"
    git reset HEAD --quiet 2>/dev/null || true
  else
    git commit -m "$COMMIT_MSG" --quiet 2>/dev/null
    log "Committed: $COMMIT_MSG"
  fi
fi

# ── Step 3: Push to origin ───────────────────────────────────────────────────
if [ "$DRY_RUN" = "1" ]; then
  log "DRY RUN: Would push to origin/$CURRENT_BRANCH"
else
  # Only push if we have commits ahead of origin
  AHEAD=$(git rev-list "origin/$CURRENT_BRANCH..HEAD" --count 2>/dev/null || echo "0")
  if [ "$AHEAD" -gt 0 ]; then
    git push origin "$CURRENT_BRANCH" --quiet 2>/dev/null || {
      log "WARNING: git push failed — will retry next cycle"
    }
    log "Pushed $AHEAD commit(s) to origin/$CURRENT_BRANCH"
  else
    log "Nothing to push"
  fi
fi

log "Sync complete"
