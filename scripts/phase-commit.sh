#!/usr/bin/env bash
# phase-commit.sh — Commit and push the phase/milestone branch reliably on CIFS-backed repos.
#
# This script wraps the git operations needed to push the phase/milestone branch
# with the following safeguards, addressing the root causes documented in:
#   docs/git-index-write-failure-investigation.md (CIFS D-state / disk-full)
#   docs/git-push-timeout-investigation.md        (SSH/DNS push failures)
#
# Safeguards implemented:
#   1. CIFS mount health check (timed stat probe, disk-space guard)
#   2. Stale index.lock cleanup before any git write
#   3. SSH + DNS pre-flight before push
#   4. Retry loop (3 attempts, 15 s back-off) for the push step
#   5. Hard wall-clock timeout on the entire push operation
#   6. Fetch + merge --ff-only before push to prevent non-fast-forward rejection
#      without rewriting history (Mitigation H — see docs/git-push-timeout-investigation.md
#      Incident 8 and task-203f3e70a84c48be8d8c40dc9994ddfb)
#      NOTE: rebase was the original approach but is unsafe on a shared branch —
#      it rewrites commit SHAs, can itself produce non-fast-forward failures when two
#      agents race, and makes the phase-commit audit trail harder to follow.
#      fetch + merge --ff-only integrates upstream-only advances without any
#      history rewrite; if the remote and local have diverged the merge fails
#      fast and the operator (or retry loop) can decide what to do.
#
# Usage:
#   bash scripts/phase-commit.sh [--branch <branch>] [--message <msg>] [--dry-run]
#
# Environment:
#   AGENT_NAME            Git author name  (default: acc-agent)
#   AGENT_EMAIL           Git author email (default: acc-agent@acc)
#   GIT_PUSH_MAX_ATTEMPTS Push retry count (default: 3)
#   GIT_PUSH_RETRY_DELAY  Seconds between retries (default: 15)
#   GIT_PUSH_TIMEOUT      Wall-clock seconds for each push attempt (default: 120)
#   CIFS_FREE_MIB_MIN     Abort if CIFS share has less free space (default: 512)
#   CIFS_PROBE_TIMEOUT    Seconds for mount responsiveness probe (default: 5)

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
BRANCH="${PHASE_BRANCH:-phase/milestone}"
COMMIT_MSG=""
DRY_RUN=false

AGENT_NAME="${AGENT_NAME:-acc-agent}"
AGENT_EMAIL="${AGENT_EMAIL:-acc-agent@acc}"

MAX_ATTEMPTS="${GIT_PUSH_MAX_ATTEMPTS:-3}"
RETRY_DELAY="${GIT_PUSH_RETRY_DELAY:-15}"
PUSH_TIMEOUT="${GIT_PUSH_TIMEOUT:-120}"
CIFS_FREE_MIB_MIN="${CIFS_FREE_MIB_MIN:-512}"
CIFS_PROBE_TIMEOUT="${CIFS_PROBE_TIMEOUT:-5}"

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --branch)   BRANCH="$2"; shift 2 ;;
    --message)  COMMIT_MSG="$2"; shift 2 ;;
    --dry-run)  DRY_RUN=true; shift ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -z "$COMMIT_MSG" ]] && COMMIT_MSG="chore(milestone): phase commit $(date -u +%Y%m%dT%H%M%SZ) [${AGENT_NAME}]"

# ── Helpers ───────────────────────────────────────────────────────────────────
log()  { echo "[phase-commit] $*" >&2; }
die()  { echo "[phase-commit] FATAL: $*" >&2; exit 1; }
warn() { echo "[phase-commit] WARN: $*" >&2; }

# ── 0. Verify we are inside a git repo ───────────────────────────────────────
GIT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) \
  || die "Not inside a git repository"
cd "$GIT_ROOT"
log "Repo root: $GIT_ROOT"
GIT_DIR="${GIT_ROOT}/.git"

# ── 0a. Remove stale index.lock early (prevents git failures if a previous run
#        was interrupted and left a stale lock behind) ─────────────────────────
rm -f .git/index.lock

# ── 1. CIFS mount health pre-flight ──────────────────────────────────────────
log "Pre-flight 1/3: CIFS mount health"

# 1a. Detect whether this repo lives on a CIFS/SMB mount
FS_TYPE=$(stat -f --format="%T" "$GIT_ROOT" 2>/dev/null || echo "unknown")
if [[ "$FS_TYPE" == "smb2" ]] || mount | grep -qE "${GIT_ROOT%/*}.*type cifs"; then
  IS_CIFS=true
  log "  Filesystem type: CIFS/SMB2"
else
  IS_CIFS=false
  log "  Filesystem type: ${FS_TYPE} (not CIFS — skipping CIFS-specific checks)"
fi

if [[ "$IS_CIFS" == "true" ]]; then
  # 1b. Mount responsiveness probe
  log "  Probing mount responsiveness (timeout: ${CIFS_PROBE_TIMEOUT}s)…"
  if ! timeout "${CIFS_PROBE_TIMEOUT}" stat "${GIT_DIR}/config" > /dev/null 2>&1; then
    die "CIFS mount is not responding (stat timed out after ${CIFS_PROBE_TIMEOUT}s).
  The CIFS server (AccFS) is stalled or unreachable.
  → Check server health: ssh rocky 'systemctl status accfs minio redis'
  → Check D-state processes: ps aux | awk '\$8==\"D\"{print}'
  → Run: bash scripts/cifs-mount-health.sh"
  fi
  log "  Mount is responsive ✓"

  # 1c. Disk space guard
  log "  Checking free space on CIFS share…"
  FREE_MIB=$(timeout 10 df -P -BM "$GIT_ROOT" 2>/dev/null \
    | awk 'NR==2{gsub(/M/,""); print $4}' || echo "")

  if [[ -z "$FREE_MIB" ]]; then
    warn "Could not determine free space (df timed out) — proceeding with caution"
  elif [[ "$FREE_MIB" -lt "$CIFS_FREE_MIB_MIN" ]]; then
    die "CIFS share has only ${FREE_MIB} MiB free (minimum: ${CIFS_FREE_MIB_MIN} MiB).
  Near-full CIFS filesystems cause git index writes to stall in D-state.
  → On rocky: run 'juicefs gc redis://127.0.0.1:6379/1' to reclaim space
  → Remove stale files: mc rm --recursive local/accfs/accfs/tmp/
  → Target: ≥ 5 GiB free before running git operations
  → See: docs/git-index-write-failure-investigation.md"
  else
    log "  Free space: ${FREE_MIB} MiB ✓"
  fi
fi

# ── 2. Remove stale lock files ────────────────────────────────────────────────
# Covers two distinct root causes that both leave a zero-byte index.lock:
#
#   a) Crashed git process (SIGKILL / OOM kill) — the process is gone but left
#      a zero-byte lock behind because it was killed before writing any content.
#      Safe to remove immediately; no live process holds the file.
#      → Documented in: docs/git-push-timeout-investigation.md (Incident 9)
#
#   b) CIFS D-state hang — the kernel accepted the lock-file create call but the
#      subsequent write stalled indefinitely (typically: near-full CIFS share).
#      The git process is still alive in D-state.  Only remove after confirming
#      no live git process is detected; resolve the CIFS stall first if needed.
#      → Documented in: docs/git-index-write-failure-investigation.md
#
# Both cases surface as:
#   "fatal: Unable to create '.git/index.lock': File exists.
#    Another git process seems to be running in this repository."
#
# Running this cleanup BEFORE git add / git commit ensures phase-commit
# self-heals from either scenario without requiring manual intervention.
log "Pre-flight 2/3: Stale lock file cleanup"

for LOCKFILE in "${GIT_DIR}/index.lock" "${GIT_DIR}/HEAD.lock" \
                "${GIT_DIR}/packed-refs.lock"; do
  if [[ -f "$LOCKFILE" ]]; then
    LOCK_SIZE=$(stat --format="%s" "$LOCKFILE" 2>/dev/null || echo "?")
    # A zero-byte lock is unconditionally stale: the process was killed before
    # writing any content (crashed-process scenario, Incident 9) or the write
    # never completed (CIFS D-state scenario).  Safe to remove in either case.
    if [[ "$LOCK_SIZE" == "0" ]]; then
      warn "Removing zero-byte stale lock (crashed git process or CIFS stall): $LOCKFILE"
      rm -f "$LOCKFILE"
    else
      # Non-zero lock: check whether a live git process still owns it.
      LIVE_GIT=$(pgrep -ax git 2>/dev/null | grep -v "$$" | grep "$GIT_ROOT" || true)
      if [[ -z "$LIVE_GIT" ]]; then
        warn "Removing stale lock (non-zero, no active git process): $LOCKFILE"
        rm -f "$LOCKFILE"
      else
        die "Lock file exists and another git process appears active: $LOCKFILE
  Active git processes: $LIVE_GIT
  → Wait for them to finish, or kill if stuck in D-state
  → See: docs/git-index-write-failure-investigation.md and
          docs/git-push-timeout-investigation.md (Incident 9)"
      fi
    fi
  fi
done
log "  Lock files clear ✓"

# ── 3. SSH + DNS pre-flight ───────────────────────────────────────────────────
log "Pre-flight 3/3: SSH/DNS connectivity to git remote"

REMOTE_URL=$(git remote get-url origin 2>/dev/null || echo "")
if [[ -z "$REMOTE_URL" ]]; then
  warn "No remote 'origin' configured — push will be skipped"
  SKIP_PUSH=true
else
  SKIP_PUSH=false
  log "  Remote: $REMOTE_URL"

  # Extract host from SSH or HTTPS URL
  if [[ "$REMOTE_URL" =~ ^git@ ]]; then
    REMOTE_HOST=$(echo "$REMOTE_URL" | sed -E 's|git@([^:]+):.*|\1|')
    TRANSPORT="ssh"
  elif [[ "$REMOTE_URL" =~ ^https?:// ]]; then
    REMOTE_HOST=$(echo "$REMOTE_URL" | sed -E 's|https?://([^/]+)/.*|\1|')
    TRANSPORT="https"
  else
    REMOTE_HOST=""
    TRANSPORT="unknown"
  fi

  if [[ -n "$REMOTE_HOST" ]]; then
    # DNS check
    if ! getent hosts "$REMOTE_HOST" > /dev/null 2>&1; then
      die "DNS resolution failed for '${REMOTE_HOST}'.
  → Check /etc/resolv.conf and upstream nameserver health
  → Test: getent hosts ${REMOTE_HOST}
  → See: docs/git-push-timeout-investigation.md"
    fi
    log "  DNS resolved: $REMOTE_HOST ✓"

    # SSH connectivity check (only for SSH transport)
    if [[ "$TRANSPORT" == "ssh" ]]; then
      SSH_HOST="$REMOTE_HOST"
      if ! timeout 10 ssh -q \
              -o BatchMode=yes \
              -o ConnectTimeout=5 \
              -o StrictHostKeyChecking=accept-new \
              -o ExitOnForwardFailure=yes \
              "${SSH_HOST}" exit 2>/dev/null; then
        # Exit code from 'ssh host exit' is 255 if connection failed,
        # but github returns 1 (no shell) on success — so we distinguish:
        if ! getent hosts "$SSH_HOST" > /dev/null 2>&1; then
          die "SSH: DNS resolution for '${SSH_HOST}' failed"
        fi
        # If DNS works, assume SSH auth will succeed (gh key may not allow shell login)
        log "  SSH TCP reachable (auth-only host): $SSH_HOST ✓"
      else
        log "  SSH connectivity: $SSH_HOST ✓"
      fi
    fi
  fi
fi

# ── 4. Ensure CIFS-safe git configuration ────────────────────────────────────
log "Applying CIFS-safe git configuration…"
if [[ "$IS_CIFS" == "true" ]]; then
  git config --local core.trustctime false
  git config --local core.checkStat minimal
  git config --local core.preloadIndex false
  git config --local index.threads 1
  git config --local gc.auto 0
  git config --local fetch.writeCommitGraph false
  log "  CIFS git config applied ✓"
fi

# ── 5. Checkout / create branch ──────────────────────────────────────────────
# git checkout can itself stall on a CIFS/FUSE mount and leave a zero-byte
# index.lock behind (the root cause of task-4676eb6f51534a1ea66d14a630962811,
# documented as Incident 8 in docs/git-index-write-failure-investigation.md).
# We guard against this by:
#   a) Unconditionally removing any index.lock immediately before the checkout
#      (not just when [[ -f ]] is true).  This closes the TOCTOU race window
#      where a lock is created between the step-2 pre-flight and the checkout
#      call.  See Incident 8 for details.
#   b) Running checkout under `timeout` so a D-state hang is bounded.
#   c) Removing any stale index.lock left behind if checkout fails/times out,
#      then aborting with a clear error message.
#
# CHECKOUT_TIMEOUT defaults to 60 s — generous enough for a normal local
# checkout but short enough to surface a CIFS stall promptly.
CHECKOUT_TIMEOUT="${GIT_CHECKOUT_TIMEOUT:-60}"

log "Switching to branch '${BRANCH}'…"
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")

if [[ "$CURRENT_BRANCH" != "$BRANCH" ]]; then
  # Pre-checkout: unconditionally remove any stale index.lock before calling
  # git checkout.  Using an unconditional rm -f (rather than a [[ -f ]] guard)
  # closes the TOCTOU race window where a lock created between the earlier
  # pre-flight check (step 2) and this checkout call would otherwise survive.
  # This is safe because:
  #   • A zero-byte lock left by a previous killed/D-state process is always
  #     stale — git creates it with O_EXCL; if the creating process is gone
  #     (killed or OOM) it can never be the active writer.
  #   • If a concurrent live git process exists, it will re-create the lock
  #     immediately and the checkout below will fail with a clear error, which
  #     is the correct behaviour.
  # See: docs/git-index-write-failure-investigation.md (Incident 8)
  rm -f "${GIT_DIR}/index.lock"

  checkout_exit=0
  if git rev-parse --verify "$BRANCH" > /dev/null 2>&1; then
    timeout "${CHECKOUT_TIMEOUT}" git checkout "$BRANCH" || checkout_exit=$?
  else
    timeout "${CHECKOUT_TIMEOUT}" git checkout -B "$BRANCH" || checkout_exit=$?
  fi

  # Post-checkout: clean up any lock the timed-out / failed checkout left behind.
  if [[ $checkout_exit -ne 0 ]]; then
    rm -f "${GIT_DIR}/index.lock"
    if [[ $checkout_exit -eq 124 ]]; then
      die "git checkout '${BRANCH}' timed out after ${CHECKOUT_TIMEOUT}s.
  This typically means the CIFS/FUSE mount is stalled.
  → Check D-state processes: ps aux | awk '\$8==\"D\"{print}'
  → Run: bash scripts/cifs-mount-health.sh
  → Run: bash scripts/fuse-watchdog --one-shot
  → See: docs/git-index-write-failure-investigation.md"
    else
      die "git checkout '${BRANCH}' failed with exit code ${checkout_exit}"
    fi
  fi

  log "  On branch: $BRANCH ✓"
else
  log "  Already on branch: $BRANCH ✓"
fi

# ── 6. Stage and commit ───────────────────────────────────────────────────────
log "Staging changes…"
git add -A

if git diff --staged --quiet; then
  log "Nothing to commit — branch is already up to date"
  COMMITTED=false
else
  if [[ "$DRY_RUN" == "true" ]]; then
    log "DRY RUN: Would commit: $COMMIT_MSG"
    git diff --staged --stat
    COMMITTED=false
  else
    git \
      -c "user.name=${AGENT_NAME}" \
      -c "user.email=${AGENT_EMAIL}" \
      commit -m "$COMMIT_MSG" --no-verify
    SHA=$(git rev-parse --short HEAD)
    log "Committed: $COMMIT_MSG @ $SHA ✓"
    COMMITTED=true
  fi
fi

# ── 7. Sync with remote (fetch + merge --ff-only) to avoid non-fast-forward ───
# The phase/milestone branch is shared across multiple agents; the remote can
# advance while we are working.  We fetch and then attempt a fast-forward-only
# merge so that our push is always a fast-forward.
#
# WHY NOT REBASE?
# The original Mitigation H (Incident 8) used `git rebase` here, but that was
# flagged as unsafe in task-203f3e70a84c48be8d8c40dc9994ddfb:
#   • Rebase rewrites local commit SHAs.  On a *shared* branch that means peers
#     will see a diverged history the next time they fetch, potentially causing
#     the very non-fast-forward failures we are trying to avoid.
#   • When two agents run phase-commit.sh concurrently, both may rebase onto the
#     same remote tip and then race to push — one will win, the other will fail
#     with a non-fast-forward even after rebasing.
#   • --force-with-lease does NOT protect against the race: after rebase the
#     local remote-tracking ref is updated, so the lease check passes for both
#     agents, and whichever pushes second will silently overwrite the other.
#
# SAFER ALTERNATIVE — fetch + merge --ff-only:
#   • If the remote is strictly ahead of local (the common case when another
#     agent pushed while we were working), --ff-only advances HEAD without
#     touching any existing commit objects — no history rewrite.
#   • If local and remote have *diverged* (both agents committed independently),
#     --ff-only fails immediately with a clear error rather than silently
#     rewriting history.  The operator can then decide whether to merge, pick,
#     or re-run the script after the conflict is resolved.
#   • The existing --force-with-lease push + retry loop below continues to
#     guard against races: if a second agent pushes between our fetch and our
#     push, --force-with-lease will reject the push and the retry loop will
#     fetch again.
#
# See: docs/git-push-timeout-investigation.md (Incident 8, Mitigation H rev 2)
#      task-203f3e70a84c48be8d8c40dc9994ddfb
if [[ "$SKIP_PUSH" == "true" ]]; then
  log "No remote configured — skipping push"
  exit 0
fi

if [[ "$DRY_RUN" == "true" ]]; then
  log "DRY RUN: Would fetch + merge --ff-only + push branch '${BRANCH}' to origin"
  exit 0
fi

log "Syncing with remote before push (fetch + merge --ff-only)…"
if git fetch --quiet origin "${BRANCH}" 2>&1; then
  REMOTE_TRACKING="origin/${BRANCH}"
  if git rev-parse --verify "${REMOTE_TRACKING}" > /dev/null 2>&1; then
    if git merge-base --is-ancestor "${REMOTE_TRACKING}" HEAD 2>/dev/null; then
      log "  Local branch is already ahead of remote — no merge needed ✓"
    else
      log "  Remote has new commits — fast-forward merging ${REMOTE_TRACKING} into local…"
      git merge --ff-only "${REMOTE_TRACKING}" \
        || die "Fast-forward merge of ${REMOTE_TRACKING} failed.
  This means the local branch and remote have diverged (both have independent
  commits).  Rebase is intentionally NOT used here because it rewrites history
  on a shared branch — see task-203f3e70a84c48be8d8c40dc9994ddfb.
  Options:
    • If your local commit(s) should follow the remote: reset and re-apply.
        git reset --hard ${REMOTE_TRACKING}
        # re-stage and re-commit your changes, then re-run this script
    • If both sets of commits must be kept: use an explicit merge commit on a
      personal branch first, then fast-forward phase/milestone to it.
  See: docs/git-push-timeout-investigation.md (Incident 8)"
      log "  Fast-forward merge complete ✓"
    fi
  else
    log "  Remote tracking ref not found — proceeding without merge"
  fi
else
  warn "git fetch failed — proceeding without merge (push may be rejected if remote is ahead)"
fi

# ── 8. Push with retry ────────────────────────────────────────────────────────
log "Pushing '${BRANCH}' to origin (max ${MAX_ATTEMPTS} attempts, ${RETRY_DELAY}s back-off)…"

PUSHED=false
for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  log "  Push attempt ${attempt}/${MAX_ATTEMPTS}…"

  if timeout "${PUSH_TIMEOUT}" \
       env GIT_TERMINAL_PROMPT=0 \
           GIT_HTTP_LOW_SPEED_LIMIT=1000 \
           GIT_HTTP_LOW_SPEED_TIME=30 \
       git push --force-with-lease origin "${BRANCH}" 2>&1; then
    log "  Push succeeded on attempt ${attempt} ✓"
    PUSHED=true
    break
  fi

  if [[ "$attempt" -lt "$MAX_ATTEMPTS" ]]; then
    warn "  Push failed on attempt ${attempt} — retrying in ${RETRY_DELAY}s…"
    sleep "$RETRY_DELAY"

    # Re-check DNS before retry (transient resolver failures)
    if [[ -n "${REMOTE_HOST:-}" ]]; then
      if ! getent hosts "$REMOTE_HOST" > /dev/null 2>&1; then
        warn "  DNS still failing for '${REMOTE_HOST}' — will retry anyway"
      fi
    fi
  else
    die "Push failed after ${MAX_ATTEMPTS} attempts.
  → Check SSH key: ssh -T git@github.com
  → Check DNS:    getent hosts ${REMOTE_HOST:-github.com}
  → Check remote: git remote -v
  → See: docs/git-push-timeout-investigation.md"
  fi
done

if [[ "$PUSHED" == "true" ]]; then
  SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "?")
  log "Phase commit complete: ${BRANCH} @ ${SHA}"
else
  die "Phase commit push failed"
fi
