#!/usr/bin/env bash
# cifs-mount-health.sh — Diagnose CIFS/SMB mount health for AccFS git repos
#
# Checks for the root causes of D-state git hangs on CIFS-backed workspaces:
#   1. Stale git lock files (zero-byte, left by interrupted writes)
#   2. Near-full CIFS filesystem (< threshold → write stalls → D-state)
#   3. D-state processes blocked on CIFS I/O
#   4. CIFS mount responsiveness (timed stat probe)
#   5. CIFS mount options (soft/hard, retrans, cache mode)
#   6. dmesg CIFS errors (if accessible)
#
# Usage:
#   bash scripts/cifs-mount-health.sh [--fix] [--mount-path <path>]
#
#   --fix           Auto-remove zero-byte stale lock files (default: report only)
#   --mount-path    Path on the CIFS mount to probe (default: /home/jkh/.acc/shared)
#
# Exit codes:
#   0  All checks passed
#   1  One or more checks failed / unhealthy condition found
#
# See docs/git-index-write-failure-investigation.md for full analysis.

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
MOUNT_PATH="${CIFS_MOUNT_PATH:-/home/jkh/.acc/shared}"
FIX_LOCKS=false
PROBE_TIMEOUT=5          # seconds for mount responsiveness probe
DISK_WARN_MIB=1024       # warn if free space < 1 GiB
DISK_CRIT_MIB=512        # critical if free space < 512 MiB
OVERALL_STATUS=0          # 0=ok, 1=problem detected

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --fix)          FIX_LOCKS=true; shift ;;
    --mount-path)   MOUNT_PATH="$2"; shift 2 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

# ── Helpers ───────────────────────────────────────────────────────────────────
info()  { echo "[INFO ] $*"; }
warn()  { echo "[WARN ] $*"; OVERALL_STATUS=1; }
crit()  { echo "[CRIT ] $*"; OVERALL_STATUS=1; }
ok()    { echo "[OK   ] $*"; }
sep()   { echo "──────────────────────────────────────────────────────────"; }

sep
echo "  CIFS Mount Health Check — $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "  Probe path: ${MOUNT_PATH}"
sep

# ── Check 1: CIFS mount is present ───────────────────────────────────────────
echo ""
info "Check 1: CIFS mount presence"

MOUNT_ENTRY=$(mount | grep -E "^//.*${MOUNT_PATH}|${MOUNT_PATH}.*type cifs" 2>/dev/null || true)
if [[ -z "$MOUNT_ENTRY" ]]; then
  crit "No CIFS mount found covering '${MOUNT_PATH}'"
  crit "  → The AccFS share may be unmounted.  On rocky: juicefs mount …"
  crit "  → On worker nodes: re-mount with: mount -t cifs //rocky/accfs ${MOUNT_PATH} -o ..."
else
  ok "CIFS mount found:"
  echo "    ${MOUNT_ENTRY}"

  # Check for dangerous mount options
  if echo "$MOUNT_ENTRY" | grep -q 'hard[^l]'; then
    warn "  Mount is 'hard' — git processes will hang indefinitely on server failure"
    warn "  → Recommend adding 'soft,retrans=3' to mount options"
  fi
  if echo "$MOUNT_ENTRY" | grep -qv 'soft'; then
    warn "  Mount is not explicitly 'soft' — may hang indefinitely on I/O errors"
  else
    ok "  Mount is 'soft' — will eventually return errors instead of hanging forever"
  fi

  RETRANS=$(echo "$MOUNT_ENTRY" | grep -oP 'retrans=\K[0-9]+' || echo "unknown")
  if [[ "$RETRANS" == "1" ]]; then
    warn "  retrans=1 is very low — consider retrans=3 for transient network issues"
  elif [[ "$RETRANS" != "unknown" ]]; then
    ok "  retrans=${RETRANS}"
  fi

  CACHE=$(echo "$MOUNT_ENTRY" | grep -oP 'cache=\K\w+' || echo "unknown")
  info "  cache=${CACHE} — 'strict' means writeback flush must succeed; 'none' avoids dirty-page stalls"
fi

# ── Check 2: Mount responsiveness (timed probe) ───────────────────────────────
echo ""
info "Check 2: CIFS mount responsiveness (${PROBE_TIMEOUT}s timeout)"

PROBE_START=$(date +%s%N)
if timeout "${PROBE_TIMEOUT}" stat "${MOUNT_PATH}" > /dev/null 2>&1; then
  PROBE_END=$(date +%s%N)
  PROBE_MS=$(( (PROBE_END - PROBE_START) / 1000000 ))
  if [[ $PROBE_MS -gt 2000 ]]; then
    warn "stat('${MOUNT_PATH}') took ${PROBE_MS}ms — mount is slow (possible server stall)"
  else
    ok "stat('${MOUNT_PATH}') responded in ${PROBE_MS}ms"
  fi
else
  crit "stat('${MOUNT_PATH}') timed out after ${PROBE_TIMEOUT}s — mount is STALLED"
  crit "  → CIFS server (100.89.199.14) may be unreachable or overloaded"
  crit "  → D-state git processes will not recover until mount recovers"
  crit "  → On rocky: check JuiceFS/MinIO/Redis; on worker: remount the share"
fi

# ── Check 3: Disk space ────────────────────────────────────────────────────────
echo ""
info "Check 3: Disk space on CIFS share"

DF_OUT=$(timeout 10 df -BM "${MOUNT_PATH}" 2>/dev/null | tail -1 || true)
if [[ -z "$DF_OUT" ]]; then
  warn "Could not determine free space (df timed out or failed)"
else
  FREE_MIB=$(echo "$DF_OUT" | awk '{gsub(/M/,""); print $4}')
  USE_PCT=$(echo "$DF_OUT" | awk '{print $5}')
  echo "    ${DF_OUT}"
  if [[ -n "$FREE_MIB" ]] && [[ "$FREE_MIB" -lt "$DISK_CRIT_MIB" ]]; then
    crit "CRITICAL: Only ${FREE_MIB} MiB free (< ${DISK_CRIT_MIB} MiB threshold)"
    crit "  → Git index writes WILL stall → D-state hangs"
    crit "  → On rocky: run 'juicefs gc' and remove stale files from AccFS"
    crit "  → Target: keep ≥ 5 GiB free at all times"
  elif [[ -n "$FREE_MIB" ]] && [[ "$FREE_MIB" -lt "$DISK_WARN_MIB" ]]; then
    warn "WARNING: Only ${FREE_MIB} MiB free (< ${DISK_WARN_MIB} MiB threshold)"
    warn "  → Risk of D-state hangs during git index writes"
    warn "  → On rocky: run 'juicefs gc' to reclaim stale chunks"
  else
    ok "Free space: ${FREE_MIB} MiB (${USE_PCT} used)"
  fi
fi

# ── Check 4: Stale git lock files ─────────────────────────────────────────────
echo ""
info "Check 4: Stale git lock files under '${MOUNT_PATH}'"

# Use find with timeout to avoid hanging on stalled mount
LOCK_FILES=$(timeout 15 find "${MOUNT_PATH}" -name "*.lock" -size 0 2>/dev/null || true)

if [[ -z "$LOCK_FILES" ]]; then
  ok "No zero-byte lock files found"
else
  while IFS= read -r lf; do
    LOCK_AGE=""
    if LOCK_STAT=$(timeout 3 stat "$lf" 2>/dev/null); then
      LOCK_MTIME=$(echo "$LOCK_STAT" | grep -oP 'Modify: \K[0-9-]+ [0-9:]+' | head -1 || true)
      LOCK_AGE=" (mtime: ${LOCK_MTIME})"
    fi
    if [[ "$FIX_LOCKS" == "true" ]]; then
      rm -f "$lf" && ok "Removed stale lock: $lf${LOCK_AGE}" \
                  || warn "Could not remove: $lf"
    else
      warn "Stale zero-byte lock file: $lf${LOCK_AGE}"
      warn "  → Remove with: rm -f '$lf'"
      warn "  → Or re-run with --fix to auto-remove"
    fi
  done <<< "$LOCK_FILES"
fi

# ── Check 5: D-state processes ────────────────────────────────────────────────
echo ""
info "Check 5: Processes in D-state (uninterruptible sleep)"

DSTATE_PROCS=$(cat /proc/*/status 2>/dev/null \
  | awk '/^Name:/{name=$2} /^Pid:/{pid=$2} /^State:.*D /{print pid, name}' \
  | head -20 || true)

if [[ -z "$DSTATE_PROCS" ]]; then
  ok "No D-state processes found"
else
  warn "D-state processes detected:"
  while IFS= read -r proc; do
    echo "    PID $proc"
  done <<< "$DSTATE_PROCS"
  warn "  → These are likely blocked on CIFS I/O"
  warn "  → They will recover when the server responds or the mount is remounted"
  warn "  → If persistent: check server health at 100.89.199.14"
fi

# ── Check 6: git config (CIFS safety settings) ────────────────────────────────
echo ""
info "Check 6: git configuration for CIFS safety"

# Find git repos under the mount path
GIT_REPOS=$(timeout 10 find "${MOUNT_PATH}" -maxdepth 4 -name ".git" -type d 2>/dev/null | head -10 || true)
REPOS_CHECKED=0

if [[ -n "$GIT_REPOS" ]]; then
  while IFS= read -r gitdir; do
    repo_root="${gitdir%/.git}"
    REPOS_CHECKED=$((REPOS_CHECKED + 1))
    echo "  Repo: ${repo_root}"

    cfg="${gitdir}/config"
    [[ -f "$cfg" ]] || continue

    # Check critical CIFS settings
    for setting in "core.trustctime=false" "core.checkStat=minimal" \
                   "core.preloadIndex=false" "index.threads=1" "gc.auto=0"; do
      key="${setting%=*}"
      expected_val="${setting#*=}"
      actual_val=$(git -C "$repo_root" config --local "$key" 2>/dev/null || echo "")
      if [[ -z "$actual_val" ]]; then
        warn "    ${key} not set (recommend: ${key}=${expected_val})"
      elif [[ "$actual_val" != "$expected_val" ]]; then
        warn "    ${key}=${actual_val} (recommend: ${expected_val})"
      else
        ok "    ${key}=${actual_val}"
      fi
    done
  done <<< "$GIT_REPOS"
fi

if [[ $REPOS_CHECKED -eq 0 ]]; then
  info "No git repos found under ${MOUNT_PATH} (or find timed out)"
fi

# ── Check 7: dmesg CIFS errors ────────────────────────────────────────────────
echo ""
info "Check 7: Recent CIFS errors in kernel log (dmesg)"

DMESG_OUT=$(dmesg --level=err,warn 2>/dev/null \
  | grep -iE 'cifs|smb|STATUS_DISK_FULL|writeback error|neterr' \
  | tail -10 || true)

if [[ -z "$DMESG_OUT" ]]; then
  ok "No recent CIFS errors in dmesg (or dmesg not accessible)"
else
  warn "CIFS-related kernel messages:"
  while IFS= read -r line; do
    echo "    $line"
  done <<< "$DMESG_OUT"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
sep
if [[ $OVERALL_STATUS -eq 0 ]]; then
  echo "  RESULT: All checks passed ✓"
else
  echo "  RESULT: Issues detected — see WARN/CRIT messages above ✗"
  echo ""
  echo "  Quick remediation:"
  echo "    1. Check disk space on rocky: df -h /mnt/accfs"
  echo "    2. Reclaim space: juicefs gc redis://127.0.0.1:6379/1"
  echo "    3. Remove stale locks: $0 --fix"
  echo "    4. Full investigation: docs/git-index-write-failure-investigation.md"
fi
sep
echo ""

exit "$OVERALL_STATUS"
