#!/usr/bin/env bash
# remove-stale-index-lock.sh — Safely remove a stale .git/index.lock file.
#
# When a git process (git status, git checkout, git commit, etc.) is interrupted
# while running on a CIFS/SMB2-backed filesystem it can leave a zero-byte
# .git/index.lock behind.  Subsequent git commands then fail with:
#
#   fatal: Unable to create '.git/index.lock': File exists.
#
# This script detects and removes the lock, guarding against the case where
# another live git process still holds it.
#
# Usage:
#   bash scripts/remove-stale-index-lock.sh [--repo <path>] [--force]
#
# Options:
#   --repo <path>   Path to the git repository root (default: current directory)
#   --force         Remove the lock even if another git process appears to be
#                   running in the same repo (use with caution)
#   -n, --dry-run   Print what would be done without removing anything
#   -h, --help      Show this help message and exit
#
# Exit codes:
#   0   Lock was absent (nothing to do) or was successfully removed
#   1   Lock exists and a live git process may still hold it (use --force to override)
#   2   Usage error
#
# See also:
#   scripts/phase-commit.sh       -- milestone commit with full CIFS pre-flight
#   scripts/cifs-mount-health.sh  -- broader CIFS mount diagnostics
#   docs/git-index-write-failure-investigation.md

set -euo pipefail

# Defaults
REPO_PATH="."
FORCE=false
DRY_RUN=false

# Argument parsing
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)       REPO_PATH="$2"; shift 2 ;;
    --force)      FORCE=true; shift ;;
    -n|--dry-run) DRY_RUN=true; shift ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Helpers
log()  { echo "[remove-stale-index-lock] $*" >&2; }
die()  { echo "[remove-stale-index-lock] FATAL: $*" >&2; exit 1; }
warn() { echo "[remove-stale-index-lock] WARN: $*" >&2; }

# Locate the git directory
GIT_ROOT=$(git -C "$REPO_PATH" rev-parse --show-toplevel 2>/dev/null) \
  || die "'${REPO_PATH}' is not inside a git repository"

GIT_DIR="${GIT_ROOT}/.git"
LOCKFILE="${GIT_DIR}/index.lock"

log "Repository root : ${GIT_ROOT}"
log "Lock file path  : ${LOCKFILE}"

# Check whether the lock file exists
if [[ ! -f "$LOCKFILE" ]]; then
  log "No index.lock present -- nothing to do"
  exit 0
fi

# Gather information about the lock file
LOCK_SIZE=$(stat --format="%s" "$LOCKFILE" 2>/dev/null || echo "unknown")
LOCK_MTIME=$(stat --format="%y" "$LOCKFILE" 2>/dev/null || echo "unknown")
log "Lock file exists: size=${LOCK_SIZE} bytes, mtime=${LOCK_MTIME}"

# Determine whether any live git process still owns the lock.
# Look for a git process running in this repo (excluding this script's own shell).
LIVE_GIT=$(pgrep -ax git 2>/dev/null \
  | grep -v "$$" \
  | grep -F "$GIT_ROOT" \
  || true)

if [[ -n "$LIVE_GIT" ]]; then
  if [[ "$FORCE" == "true" ]]; then
    warn "Active git process(es) detected -- removing lock anyway (--force):"
    while IFS= read -r proc; do
      warn "  ${proc}"
    done <<< "$LIVE_GIT"
  else
    log "Active git process(es) may still hold the lock:"
    while IFS= read -r proc; do
      log "  ${proc}"
    done <<< "$LIVE_GIT"
    log ""
    log "Wait for the process to finish, then retry."
    log ""
    log "If the process is stuck in D-state on a CIFS mount, run:"
    log "  bash scripts/cifs-mount-health.sh"
    log ""
    log "To remove the lock despite the running process (use with caution):"
    log "  bash $0 --force ${REPO_PATH:+--repo $REPO_PATH}"
    exit 1
  fi
elif [[ "$LOCK_SIZE" != "0" ]] && [[ "$LOCK_SIZE" != "unknown" ]]; then
  # Non-zero lock with no active git process -- still safe to remove, but note it.
  warn "Lock file is non-zero (${LOCK_SIZE} bytes) with no active git process."
  warn "This may indicate a previously crashed write.  Removing."
fi

# Remove the lock file
if [[ "$DRY_RUN" == "true" ]]; then
  log "DRY RUN: Would remove ${LOCKFILE}"
else
  rm -f "$LOCKFILE"
  log "Removed ${LOCKFILE}"
fi

# Verify git is now operational
if [[ "$DRY_RUN" != "true" ]]; then
  if git -C "$GIT_ROOT" status --short > /dev/null 2>&1; then
    log "git status: OK -- repository is operational"
  else
    warn "git status returned non-zero after lock removal."
    warn "There may be an underlying issue beyond the lock file."
    warn "Run: bash scripts/cifs-mount-health.sh"
  fi
fi

exit 0
