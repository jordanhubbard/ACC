#!/usr/bin/env bash
# apply-cifs-git-config.sh — Idempotently apply all six CIFS-safe git tunables
#                             and print a verification summary.
#
# Background:
#   AccFS workspaces live on a CIFS/SMB2 share.  Default git behaviour relies
#   on ctime, parallel stat, and background GC processes that interact badly
#   with CIFS, causing spurious "modified" files, D-state hangs, and corrupt
#   index writes.  The six settings below are the minimal set identified during
#   the git-index-write-failure investigation (see
#   docs/git-index-write-failure-investigation.md) that make git reliable on
#   this mount.
#
#   These settings are LOCAL (per-repo) and are NOT stored in a committed
#   config file, so every fresh clone or new agent spin-up must re-apply them.
#   This script is idempotent — safe to run multiple times.
#
# Post-clone / onboarding note:
#   The repository tracks a shared .gitconfig (at the repo root) that contains
#   project-wide git aliases, hooks, and other settings.  Git does NOT load
#   this file automatically — you must wire it into your local .git/config
#   with an [include] stanza once after every fresh clone:
#
#     git config --local include.path ../.gitconfig
#
#   This script applies the six CIFS tunables but does NOT add the include
#   stanza automatically (the path to .gitconfig may vary by setup).  Run the
#   command above from the repo root after cloning, or ask your operator which
#   path is appropriate for your deployment.  See CONTRIBUTING.md §"CIFS /
#   SMB2 Filesystem Notes" and GETTING_STARTED.md §"Developer Path" for the
#   full post-clone checklist.
#
# Settings applied:
#   core.trustctime          = false   CIFS ctime is unreliable; skip re-stats
#   core.checkStat           = minimal Use only mtime+size for change detection
#   core.preloadIndex        = false   No parallel stat storm over SMB2
#   index.threads            = 1       Serialise index I/O; safer on CIFS
#   gc.auto                  = 0       Disable auto-GC; run git gc manually
#   fetch.writeCommitGraph   = false   Skip large object writes on every fetch
#
# Usage:
#   bash scripts/apply-cifs-git-config.sh [--repo-root <path>]
#
# Options:
#   --repo-root <path>   Git repository to configure (default: root of CWD)
#   -h, --help           Show this help text and exit
#
# Exit codes:
#   0  All six settings applied and verified successfully
#   1  Error (not a git repo, unknown argument, etc.)
#
# See also:
#   scripts/setup-git-cifs.sh          — earlier five-setting version
#   scripts/cifs-mount-health.sh       — broader CIFS mount diagnostics
#   scripts/remove-stale-index-lock.sh — clean up stale .git/index.lock
#   scripts/git-push-helper.sh         — DNS pre-flight checks before git push
#   docs/git-index-write-failure-investigation.md

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

# Default: detect repo root from the script's own location, then fall back to CWD.
if [[ -z "$REPO_ROOT" ]]; then
  REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel 2>/dev/null \
               || git rev-parse --show-toplevel)"
fi

if [[ ! -d "$REPO_ROOT/.git" ]]; then
  echo "ERROR: '$REPO_ROOT' does not appear to be a git repository." >&2
  exit 1
fi

echo "Applying CIFS-safe git tunables to: $REPO_ROOT"
echo ""

# ── Helper ────────────────────────────────────────────────────────────────────
apply() {
  local key="$1" value="$2"
  git -C "$REPO_ROOT" config --local "$key" "$value"
}

# ── Apply all six settings ────────────────────────────────────────────────────
# 1. Do not trust ctime — CIFS servers report inconsistent ctime values which
#    cause git to treat every file as modified and trigger expensive rescans.
apply core.trustctime false

# 2. Use only file size + mtime for change detection; ignore inode / ctime fields.
apply core.checkStat minimal

# 3. Disable parallel index preload — avoids hangs when the CIFS mount is slow
#    or temporarily unresponsive during git status / git add operations.
apply core.preloadIndex false

# 4. Restrict index write threads to 1 — multi-threaded writes can produce
#    partial/corrupt index files when the underlying CIFS path serialises I/O.
apply index.threads 1

# 5. Disable automatic garbage collection — 'git gc --auto' forks background
#    processes that stall indefinitely on CIFS; run 'git gc' manually instead.
apply gc.auto 0

# 6. Skip commit-graph write on fetch — writing the commit-graph produces large
#    temporary objects that amplify CIFS write pressure and risk D-state hangs.
apply fetch.writeCommitGraph false

# ── Verification summary ──────────────────────────────────────────────────────
echo "Verification summary"
echo "────────────────────────────────────────────────"

declare -A EXPECTED=(
  [core.trustctime]="false"
  [core.checkstat]="minimal"      # git normalises the key to lowercase
  [core.preloadindex]="false"
  [index.threads]="1"
  [gc.auto]="0"
  [fetch.writecommitgraph]="false"
)

ALL_OK=true
for key in \
  core.trustctime \
  core.checkstat \
  core.preloadindex \
  index.threads \
  gc.auto \
  fetch.writecommitgraph
do
  actual="$(git -C "$REPO_ROOT" config --local "$key" 2>/dev/null || echo "")"
  expected="${EXPECTED[$key]}"
  if [[ "$actual" == "$expected" ]]; then
    printf "  %-30s %s  ✓\n" "${key}" "${actual}"
  else
    printf "  %-30s got '%s', expected '%s'  ✗\n" "${key}" "${actual}" "${expected}"
    ALL_OK=false
  fi
done

echo "────────────────────────────────────────────────"

if [[ "$ALL_OK" == "true" ]]; then
  echo "All six CIFS tunables applied and verified."
  exit 0
else
  echo "ERROR: One or more settings did not apply correctly." >&2
  exit 1
fi
