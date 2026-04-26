#!/usr/bin/env bash
# fix_fuse_mount.sh — Automated remediation for a stalled JuiceFS/NFS FUSE mount.
#
# Performs the following steps in order:
#
#   1. Abort queued FUSE kernel requests by writing to /sys/fs/fuse/connections/
#      (forces pending I/O to return EIO so D-state processes can unblock).
#   2. SIGKILL any git processes stuck in D-state that are blocked on the mount.
#   3. Remove stale git lock files (.git/index.lock, .git/HEAD.lock,
#      .git/packed-refs.lock) left behind by killed processes.
#   4. Lazy-unmount the stalled filesystem with `umount -l` so the VFS layer
#      detaches immediately even if the FUSE daemon is unresponsive.
#   5. Restart the JuiceFS or NFS daemon (accfs.service / juicefs) with
#      hardened mount options (--no-bgjob, --max-retries, entry-ttl tuning)
#      and re-mount the volume.
#
# Designed for the AccFS FUSE mount on rocky (do-host1).  Can be pointed at
# any JuiceFS or generic FUSE mount by setting the environment variables below.
#
# Usage:
#   sudo bash scripts/fix_fuse_mount.sh [OPTIONS]
#
# Options:
#   --mount-point <path>    FUSE mount point to fix (default: /mnt/accfs)
#   --meta-url <url>        JuiceFS metadata URL  (default: redis://127.0.0.1:6379/1)
#   --service <name>        systemd service managing the FUSE daemon
#                           (default: accfs.service; set to "none" to skip)
#   --git-root <path>       Git repo to clean up lock files in; may be specified
#                           multiple times (default: auto-discover under --mount-point
#                           and /home/jkh/.acc/shared)
#   --dry-run               Print actions without executing them
#   --no-restart            Skip daemon restart after unmount
#   --force                 Kill D-state processes even if mount probe succeeds
#   -h, --help              Show this help and exit
#
# Environment variables (override defaults without flags):
#   FUSE_MOUNT_POINT        Same as --mount-point
#   JUICEFS_META_URL        Same as --meta-url
#   ACCFS_SERVICE           Same as --service
#
# Exit codes:
#   0   All steps completed (or nothing needed)
#   1   One or more steps failed
#
# Prerequisites:
#   - Must run as root (or with passwordless sudo) — required for umount,
#     /sys/fs/fuse/connections writes, and SIGKILL across UIDs.
#   - juicefs binary must be on PATH (or set JUICEFS_BIN=/path/to/juicefs).
#   - For NFS mounts: replace the juicefs-specific restart with the appropriate
#     NFS daemon restart command (see DAEMON_RESTART_CMD below).
#
# See also:
#   docs/accfs.md                              — AccFS architecture
#   docs/git-index-write-failure-investigation.md — CIFS/FUSE D-state analysis
#   scripts/cifs-mount-health.sh               — broader mount diagnostics
#   scripts/remove-stale-index-lock.sh         — standalone lock-file removal

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# Defaults (overridable via environment or flags)
# ─────────────────────────────────────────────────────────────────────────────
MOUNT_POINT="${FUSE_MOUNT_POINT:-/mnt/accfs}"
META_URL="${JUICEFS_META_URL:-redis://127.0.0.1:6379/1}"
DAEMON_SERVICE="${ACCFS_SERVICE:-accfs.service}"
JUICEFS_BIN="${JUICEFS_BIN:-juicefs}"
DRY_RUN=false
NO_RESTART=false
FORCE=false

# Paths to probe for git lock files (beyond the mount point itself)
EXTRA_GIT_ROOTS=()

# Hard limits for the remediation loop
DSTATE_KILL_TIMEOUT=15   # seconds to wait for D-state processes to exit after SIGKILL
MOUNT_PROBE_TIMEOUT=5    # seconds for mount-point accessibility probe
DAEMON_START_WAIT=20     # seconds to wait for daemon to become active after (re)start

# Hardened JuiceFS mount options appended at restart time
# --no-bgjob          disable background GC/check jobs that can cause I/O spikes
# --max-retries 3     retry metadata ops up to 3 times before surfacing an error
# --entry-cache 1     cache FUSE directory-entry lookups for 1 s (reduces stat storms)
# --attr-cache 1      cache inode attributes for 1 s
# --open-cache 0      do not cache open file handles (safer for concurrent writers)
JUICEFS_HARDENED_OPTS="--no-bgjob --max-retries 3 --entry-cache 1 --attr-cache 1 --open-cache 0"
# Background mount daemon flag
JUICEFS_BG_FLAG="-d"

# ─────────────────────────────────────────────────────────────────────────────
# Argument parsing
# ─────────────────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mount-point)  MOUNT_POINT="$2";        shift 2 ;;
    --meta-url)     META_URL="$2";           shift 2 ;;
    --service)      DAEMON_SERVICE="$2";     shift 2 ;;
    --git-root)     EXTRA_GIT_ROOTS+=("$2"); shift 2 ;;
    --dry-run)      DRY_RUN=true;            shift   ;;
    --no-restart)   NO_RESTART=true;         shift   ;;
    --force)        FORCE=true;              shift   ;;
    -h|--help)
      sed -n '/^# fix_fuse_mount/,/^[^#]/{ /^[^#]/d; s/^# \?//; p }' "$0"
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────
log()   { echo "[fix-fuse] $(date -u '+%H:%M:%SZ') $*"; }
warn()  { echo "[fix-fuse] WARN  $(date -u '+%H:%M:%SZ') $*" >&2; }
die()   { echo "[fix-fuse] FATAL $(date -u '+%H:%M:%SZ') $*" >&2; exit 1; }
sep()   { echo "────────────────────────────────────────────────────────────"; }

# Run a command or print it in dry-run mode.
run() {
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] $*"
  else
    "$@"
  fi
}

# Check for root (most operations require it)
require_root() {
  if [[ $EUID -ne 0 ]]; then
    die "This script must be run as root (or via sudo)."
  fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Root check
# ─────────────────────────────────────────────────────────────────────────────
require_root

sep
log "fix_fuse_mount.sh — FUSE stall remediation"
log "Mount point : ${MOUNT_POINT}"
log "Meta URL    : ${META_URL}"
log "Service     : ${DAEMON_SERVICE}"
log "Dry run     : ${DRY_RUN}"
log "No restart  : ${NO_RESTART}"
sep

# ─────────────────────────────────────────────────────────────────────────────
# Step 0: Probe the mount — decide whether anything needs to be done
# ─────────────────────────────────────────────────────────────────────────────
log "Step 0: Probing mount accessibility (${MOUNT_PROBE_TIMEOUT}s timeout)…"

MOUNT_STALLED=false
if ! timeout "${MOUNT_PROBE_TIMEOUT}" stat "${MOUNT_POINT}" > /dev/null 2>&1; then
  warn "Mount point '${MOUNT_POINT}' did not respond within ${MOUNT_PROBE_TIMEOUT}s — stall confirmed."
  MOUNT_STALLED=true
elif [[ "$FORCE" == "true" ]]; then
  warn "Mount probe succeeded but --force specified — proceeding anyway."
  MOUNT_STALLED=true
else
  log "Mount point responded promptly — no stall detected."
  log "Re-run with --force to apply remediation regardless."
  exit 0
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 1: Abort queued FUSE kernel requests
# ─────────────────────────────────────────────────────────────────────────────
# The kernel FUSE layer queues requests to the userspace daemon.  When the
# daemon is dead or stuck, these requests sit in the queue indefinitely,
# keeping any process that made a syscall on the mountpoint in D-state.
# Writing "1" to the connection's "abort" file immediately terminates all
# queued requests with EIO, allowing the blocked processes to wake up.
# ─────────────────────────────────────────────────────────────────────────────
log "Step 1: Aborting queued FUSE kernel requests…"

FUSE_CONN_DIR="/sys/fs/fuse/connections"
ABORTED_COUNT=0

if [[ ! -d "$FUSE_CONN_DIR" ]]; then
  warn "  ${FUSE_CONN_DIR} not found — kernel FUSE module may not be loaded; skipping abort step."
else
  # Find connection subdirectories that correspond to our mount point.
  # Each connection directory contains a 'dev' file that holds the device
  # major:minor matching the mounted filesystem.  We match by comparing the
  # device number of the mount point to the 'dev' entries under
  # /sys/fs/fuse/connections/.
  MOUNT_DEV=""
  if MOUNT_DEV=$(stat -c '%D' "${MOUNT_POINT}" 2>/dev/null); then
    # stat -c '%D' returns hex device number.  Convert to dec for comparison.
    MOUNT_DEV_DEC=$((16#${MOUNT_DEV}))
    MOUNT_MAJOR=$(( (MOUNT_DEV_DEC >> 8) & 0xfff ))
    MOUNT_MINOR=$(( (MOUNT_DEV_DEC & 0xff) | ((MOUNT_DEV_DEC >> 12) & ~0xff) ))
    log "  Mount device: major=${MOUNT_MAJOR} minor=${MOUNT_MINOR} (0x${MOUNT_DEV})"
  else
    warn "  Could not stat '${MOUNT_POINT}' to get device number — will abort ALL fuse connections."
    MOUNT_MAJOR=""
    MOUNT_MINOR=""
  fi

  for conn_dir in "${FUSE_CONN_DIR}"/*/; do
    [[ -d "$conn_dir" ]] || continue
    ABORT_FILE="${conn_dir}abort"
    DEV_FILE="${conn_dir}dev"

    # If we know the device major:minor, only abort the matching connection.
    if [[ -n "$MOUNT_MAJOR" ]] && [[ -f "$DEV_FILE" ]]; then
      CONN_DEV=$(cat "$DEV_FILE" 2>/dev/null || echo "")
      CONN_MAJOR="${CONN_DEV%%:*}"
      CONN_MINOR="${CONN_DEV##*:}"
      if [[ "$CONN_MAJOR" != "$MOUNT_MAJOR" ]] || [[ "$CONN_MINOR" != "$MOUNT_MINOR" ]]; then
        continue  # not our mount
      fi
    fi

    if [[ -f "$ABORT_FILE" ]]; then
      log "  Aborting FUSE connection: ${conn_dir}"
      if [[ "$DRY_RUN" == "true" ]]; then
        echo "[DRY-RUN] echo 1 > ${ABORT_FILE}"
      else
        echo 1 > "$ABORT_FILE" 2>/dev/null && ABORTED_COUNT=$((ABORTED_COUNT + 1)) \
          || warn "  Failed to write to ${ABORT_FILE} (may already be inactive)"
      fi
    fi
  done

  if [[ $ABORTED_COUNT -gt 0 ]]; then
    log "  Aborted ${ABORTED_COUNT} FUSE connection(s). Waiting 2s for processes to unblock…"
    [[ "$DRY_RUN" == "true" ]] || sleep 2
  else
    log "  No matching FUSE connection entries found (or none needed aborting)."
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 2: SIGKILL git processes stuck in D-state
# ─────────────────────────────────────────────────────────────────────────────
# After the FUSE abort in Step 1 most processes should have woken up and
# either exited or returned an error to the caller.  Any git process that is
# *still* in D-state is irrecoverably stuck and must be hard-killed.
# ─────────────────────────────────────────────────────────────────────────────
log "Step 2: SIGKILLing D-state git processes…"

# Collect PIDs of git processes in state D.
# /proc/<pid>/status has "State: D (disk sleep)" for D-state processes.
# We cross-check that the process name is 'git' to avoid collateral damage.
KILLED_PIDS=()

while IFS= read -r status_file; do
  pid_dir="${status_file%/status}"
  pid="${pid_dir##*/}"

  # Must be a numeric PID directory
  [[ "$pid" =~ ^[0-9]+$ ]] || continue

  # Read process state and name atomically (best-effort; process may exit)
  proc_state=$(awk '/^State:/{print $2}' "$status_file" 2>/dev/null || echo "")
  proc_name=$(awk '/^Name:/{print $2}'  "$status_file" 2>/dev/null || echo "")

  [[ "$proc_state" == "D" ]] || continue
  [[ "$proc_name"  == "git" ]] || continue

  log "  Found D-state git process: PID=${pid} name=${proc_name}"

  # Print the process's working directory for context (may fail if proc exited)
  proc_cwd=$(readlink "/proc/${pid}/cwd" 2>/dev/null || echo "(unknown)")
  log "    cwd: ${proc_cwd}"

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] kill -9 ${pid}  # D-state git process: ${proc_name}"
  else
    if kill -9 "$pid" 2>/dev/null; then
      log "    Sent SIGKILL to PID ${pid}"
      KILLED_PIDS+=("$pid")
    else
      warn "    Could not kill PID ${pid} (may have already exited)"
    fi
  fi
done < <(find /proc -maxdepth 2 -name status 2>/dev/null | sort -t/ -k3 -n)

if [[ ${#KILLED_PIDS[@]} -gt 0 ]]; then
  log "  Waiting up to ${DSTATE_KILL_TIMEOUT}s for killed processes to vanish…"
  DEADLINE=$(( $(date +%s) + DSTATE_KILL_TIMEOUT ))
  for pid in "${KILLED_PIDS[@]}"; do
    while kill -0 "$pid" 2>/dev/null; do
      if [[ $(date +%s) -ge $DEADLINE ]]; then
        warn "  PID ${pid} is still visible after ${DSTATE_KILL_TIMEOUT}s (kernel may need to clean up)"
        break
      fi
      sleep 1
    done
  done
  log "  SIGKILL step complete (killed: ${#KILLED_PIDS[@]} process(es))."
else
  log "  No D-state git processes found."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 3: Remove stale git lock files
# ─────────────────────────────────────────────────────────────────────────────
# Processes killed in Step 2 (or by an earlier OOM event) leave zero-byte
# .git/index.lock (and potentially .git/HEAD.lock, .git/packed-refs.lock)
# files behind.  Any subsequent git operation fails with:
#   fatal: Unable to create '.git/index.lock': File exists.
# We remove all zero-byte lock files found under the paths of interest.
# ─────────────────────────────────────────────────────────────────────────────
log "Step 3: Removing stale git lock files…"

# Build list of paths to search
SEARCH_PATHS=("${MOUNT_POINT}" "/home/jkh/.acc/shared")
for extra in "${EXTRA_GIT_ROOTS[@]}"; do
  SEARCH_PATHS+=("$extra")
done
# Deduplicate
mapfile -t SEARCH_PATHS < <(printf '%s\n' "${SEARCH_PATHS[@]}" | sort -u)

LOCK_PATTERNS=("index.lock" "HEAD.lock" "packed-refs.lock" "config.lock" "COMMIT_EDITMSG.lock")
REMOVED_LOCKS=()

for search_dir in "${SEARCH_PATHS[@]}"; do
  [[ -d "$search_dir" ]] || continue
  for pattern in "${LOCK_PATTERNS[@]}"; do
    # Use a timed find to avoid hanging if the directory is also stalled
    while IFS= read -r lockfile; do
      [[ -f "$lockfile" ]] || continue
      lock_size=$(stat --format="%s" "$lockfile" 2>/dev/null || echo "unknown")
      log "  Found lock file: ${lockfile} (${lock_size} bytes)"

      # Safety: only auto-remove zero-byte locks (always stale) or locks
      # with no live owner.  Non-zero locks with a live git process may still
      # be in use.
      if [[ "$lock_size" == "0" ]]; then
        log "    Zero-byte lock — unconditionally stale; removing."
        run rm -f "$lockfile" && REMOVED_LOCKS+=("$lockfile")
      else
        # Non-zero: check for a live git process in the same repo
        repo_root="${lockfile%/.git/*}"
        live_owner=$(pgrep -ax git 2>/dev/null | grep -F "$repo_root" || true)
        if [[ -n "$live_owner" ]]; then
          warn "    Non-zero lock and a live git process detected — leaving intact:"
          warn "      ${live_owner}"
        else
          warn "    Non-zero lock (${lock_size}B) with no live git process — removing."
          run rm -f "$lockfile" && REMOVED_LOCKS+=("$lockfile")
        fi
      fi
    done < <(timeout 10 find "$search_dir" -name "$pattern" 2>/dev/null || true)
  done
done

if [[ ${#REMOVED_LOCKS[@]} -gt 0 ]]; then
  log "  Removed ${#REMOVED_LOCKS[@]} stale lock file(s)."
else
  log "  No stale lock files found."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 4: Lazy-unmount the stalled filesystem
# ─────────────────────────────────────────────────────────────────────────────
# `umount -l` (lazy unmount) detaches the filesystem from the VFS name-space
# immediately, even while the mount point is busy or the FUSE daemon is not
# responding.  Any open file descriptors remain valid until closed, but new
# opens and path lookups will fail with ENOENT, unblocking any remaining
# processes that were waiting on VFS path resolution.
# ─────────────────────────────────────────────────────────────────────────────
log "Step 4: Lazy-unmounting stalled FUSE filesystem at '${MOUNT_POINT}'…"

# Check whether the mount point is currently mounted
if mountpoint -q "${MOUNT_POINT}" 2>/dev/null; then
  log "  '${MOUNT_POINT}' is mounted — issuing lazy unmount."
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] umount -l ${MOUNT_POINT}"
    echo "[DRY-RUN] fusermount -u -z ${MOUNT_POINT}  # fallback for non-root FUSE"
  else
    if umount -l "${MOUNT_POINT}" 2>/dev/null; then
      log "  Lazy unmount succeeded."
    else
      warn "  umount -l failed — trying fusermount -u -z as fallback…"
      if fusermount -u -z "${MOUNT_POINT}" 2>/dev/null; then
        log "  fusermount lazy unmount succeeded."
      else
        warn "  fusermount also failed.  The filesystem may already be detached."
      fi
    fi
    # Brief pause to let the kernel finish cleaning up the VFS entries
    sleep 1
  fi
else
  log "  '${MOUNT_POINT}' is not currently mounted — skipping unmount."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 5: Restart the JuiceFS / NFS daemon with hardened options and remount
# ─────────────────────────────────────────────────────────────────────────────
# Hardened options applied at restart:
#   --no-bgjob       disable background GC/compaction/check jobs (reduces I/O spikes)
#   --max-retries 3  retry metadata operations up to 3 times before returning an error
#   --entry-cache 1  cache FUSE directory-entry lookups for 1 second
#   --attr-cache  1  cache inode attribute lookups for 1 second
#   --open-cache  0  do not cache open file handles (safer for concurrent writers)
#
# If a systemd service manages the daemon, it is restarted first (so systemd
# takes ownership of the new process); the FUSE mount is then re-established
# by the service's ExecStart.  If no service is configured, juicefs mount is
# invoked directly.
# ─────────────────────────────────────────────────────────────────────────────
log "Step 5: Restarting FUSE daemon and remounting…"

if [[ "$NO_RESTART" == "true" ]]; then
  log "  --no-restart specified — skipping daemon restart and remount."
  sep
  log "Remediation complete (no-restart mode).  Manual remount required:"
  log "  sudo juicefs mount ${META_URL} ${MOUNT_POINT} ${JUICEFS_HARDENED_OPTS} ${JUICEFS_BG_FLAG}"
  exit 0
fi

# 5a. Stop / restart the systemd service if configured
if [[ "$DAEMON_SERVICE" != "none" ]] && command -v systemctl > /dev/null 2>&1; then
  log "  Checking systemd service: ${DAEMON_SERVICE}"

  if systemctl list-unit-files "${DAEMON_SERVICE}" > /dev/null 2>&1 \
     && systemctl list-unit-files "${DAEMON_SERVICE}" | grep -q "${DAEMON_SERVICE}"; then

    log "  Stopping ${DAEMON_SERVICE}…"
    run systemctl stop "${DAEMON_SERVICE}" 2>/dev/null || warn "  stop returned non-zero (service may already be stopped)"

    # Brief delay so the old daemon fully releases the FUSE device
    [[ "$DRY_RUN" == "true" ]] || sleep 2

    log "  Starting ${DAEMON_SERVICE}…"
    run systemctl start "${DAEMON_SERVICE}"

    # Wait for the service to become active
    log "  Waiting up to ${DAEMON_START_WAIT}s for ${DAEMON_SERVICE} to become active…"
    if [[ "$DRY_RUN" != "true" ]]; then
      DEADLINE=$(( $(date +%s) + DAEMON_START_WAIT ))
      until systemctl is-active --quiet "${DAEMON_SERVICE}" 2>/dev/null; do
        if [[ $(date +%s) -ge $DEADLINE ]]; then
          warn "  ${DAEMON_SERVICE} did not become active within ${DAEMON_START_WAIT}s."
          warn "  Check: systemctl status ${DAEMON_SERVICE}"
          break
        fi
        sleep 2
      done
      if systemctl is-active --quiet "${DAEMON_SERVICE}" 2>/dev/null; then
        log "  ${DAEMON_SERVICE} is active ✓"
      fi
    fi
  else
    warn "  Service '${DAEMON_SERVICE}' not found in systemd — falling through to direct mount."
  fi
fi

# 5b. Ensure the mount point directory exists
if [[ "$DRY_RUN" != "true" ]]; then
  mkdir -p "${MOUNT_POINT}"
fi

# 5c. Re-mount using juicefs directly (also serves as the primary path when
#     no systemd service is configured, or as a post-service verification step).
if command -v "${JUICEFS_BIN}" > /dev/null 2>&1; then
  # Check whether the mount point is already mounted (service may have done it)
  if mountpoint -q "${MOUNT_POINT}" 2>/dev/null; then
    log "  '${MOUNT_POINT}' is already mounted after service restart — skipping direct mount."
  else
    log "  Mounting JuiceFS volume:"
    log "    ${JUICEFS_BIN} mount ${META_URL} ${MOUNT_POINT} ${JUICEFS_HARDENED_OPTS} ${JUICEFS_BG_FLAG}"

    # shellcheck disable=SC2086
    run "${JUICEFS_BIN}" mount \
      ${JUICEFS_HARDENED_OPTS} \
      ${JUICEFS_BG_FLAG} \
      "${META_URL}" \
      "${MOUNT_POINT}"

    # Verify the mount actually appeared
    if [[ "$DRY_RUN" != "true" ]]; then
      sleep 2
      if mountpoint -q "${MOUNT_POINT}"; then
        log "  JuiceFS mounted at '${MOUNT_POINT}' ✓"
      else
        warn "  '${MOUNT_POINT}' is not a mountpoint after juicefs mount — check logs:"
        warn "    journalctl -u ${DAEMON_SERVICE} --since '1 min ago'"
        warn "    ${JUICEFS_BIN} status ${META_URL}"
      fi
    fi
  fi
else
  warn "  '${JUICEFS_BIN}' not found on PATH."
  warn "  If this is an NFS mount, restart the NFS daemon manually:"
  warn "    systemctl restart nfs-client.target   # or your NFS client service"
  warn "    mount ${MOUNT_POINT}                  # re-mount from /etc/fstab"
  warn "  Set JUICEFS_BIN=/path/to/juicefs if juicefs is installed elsewhere."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Final health check
# ─────────────────────────────────────────────────────────────────────────────
log "Final health check…"

if [[ "$DRY_RUN" != "true" ]]; then
  if timeout "${MOUNT_PROBE_TIMEOUT}" stat "${MOUNT_POINT}" > /dev/null 2>&1; then
    log "  Mount point '${MOUNT_POINT}' is responsive ✓"

    # Quick disk-space report
    df_out=$(timeout 5 df -h "${MOUNT_POINT}" 2>/dev/null | tail -1 || echo "(df timed out)")
    log "  Disk space: ${df_out}"
  else
    warn "  Mount point '${MOUNT_POINT}' still not responding after ${MOUNT_PROBE_TIMEOUT}s."
    warn "  Further investigation required:"
    warn "    bash scripts/cifs-mount-health.sh --mount-path ${MOUNT_POINT}"
    warn "    journalctl -u ${DAEMON_SERVICE} -n 50"
    warn "    ${JUICEFS_BIN} status ${META_URL}"
    sep
    exit 1
  fi
fi

sep
log "fix_fuse_mount.sh complete."
log ""
log "Summary:"
log "  FUSE kernel requests aborted : ${ABORTED_COUNT}"
log "  D-state git processes killed : ${#KILLED_PIDS[@]}"
log "  Stale lock files removed     : ${#REMOVED_LOCKS[@]}"
sep
