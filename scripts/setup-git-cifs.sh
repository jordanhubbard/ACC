#!/usr/bin/env bash
# setup-git-cifs.sh — Apply git settings required for reliable operation on CIFS/SMB mounts.
#
# Background:
#   AccFS workspaces live on a CIFS (SMB) share.  The default git behaviour
#   relies on ctime and stat fields that CIFS does not preserve faithfully,
#   which causes git to treat every file as modified and triggers expensive
#   full-index rescans.  The three settings below tell git to trust only the
#   file size + mtime (checkStat=minimal), stop trusting ctime entirely, and
#   disable preloading the index into RAM (which stalls on slow/stale mounts).
#
# These settings are LOCAL (per-repo) and are NOT stored in a committed config
# file, so every fresh clone or new agent spin-up must run this script once.
# The script is idempotent — safe to run multiple times.
#
# Usage:
#   bash scripts/setup-git-cifs.sh [--repo-root <path>]
#
#   --repo-root <path>   Git repo to configure (default: git rev-parse root of CWD)
#
# Invoked automatically by:
#   make setup-git

set -euo pipefail

# ── Argument parsing ──────────────────────────────────────────────────────────
REPO_ROOT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-root) REPO_ROOT="$2"; shift 2 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

# Default: use the repo that contains this script (or the current directory).
if [[ -z "$REPO_ROOT" ]]; then
  REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel 2>/dev/null \
               || git rev-parse --show-toplevel)"
fi

if [[ ! -d "$REPO_ROOT/.git" ]]; then
  echo "ERROR: '$REPO_ROOT' does not appear to be a git repository." >&2
  exit 1
fi

echo "Configuring git CIFS settings for: $REPO_ROOT"

# ── Apply settings ────────────────────────────────────────────────────────────
# Each git-config call is idempotent; existing values are overwritten.

# Do not trust ctime — CIFS servers report inconsistent ctime values.
git -C "$REPO_ROOT" config --local core.trustctime false

# Use only file size + mtime for change detection; ignore inode / ctime fields.
git -C "$REPO_ROOT" config --local core.checkStat minimal

# Disable parallel index preload — avoids hangs when the CIFS mount is slow or
# temporarily unresponsive during git status / git add operations.
git -C "$REPO_ROOT" config --local core.preloadIndex false

# Restrict index write threads to 1 — multi-threaded index writes can produce
# partial/corrupt index files when the underlying CIFS write path serialises IO.
git -C "$REPO_ROOT" config --local index.threads 1

# Disable automatic garbage collection — 'git gc --auto' forks background
# processes that can stall indefinitely on CIFS; run 'git gc' manually instead.
git -C "$REPO_ROOT" config --local gc.auto 0

echo "Done. Active CIFS-related git config:"
git -C "$REPO_ROOT" config --local --list \
  | grep -E '^(core\.trustctime|core\.checkstat|core\.preloadindex|index\.threads|gc\.auto)' \
  | sort \
  | sed 's/^/  /'
