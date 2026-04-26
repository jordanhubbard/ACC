#!/usr/bin/env bash
# install.sh — One-shot installer for the FUSE remediation toolkit.
#
# Deploys all FUSE/CIFS remediation scripts to /usr/local/bin, registers the
# systemd fuse-watchdog service and timer, and applies kernel sysctl tuning to
# reduce the probability of JuiceFS/FUSE mount stalls on rocky (do-host1).
#
# What this script does, in order:
#
#   1. Preflight checks — root, systemd presence, OS detection.
#   2. Deploy remediation scripts to /usr/local/bin:
#        fix-fuse-mount          (from scripts/fix_fuse_mount.sh)
#        cifs-mount-health       (from scripts/cifs-mount-health.sh)
#        remove-stale-index-lock (from scripts/remove-stale-index-lock.sh)
#        setup-git-cifs          (from scripts/setup-git-cifs.sh)
#        fuse-watchdog           (from scripts/fuse-watchdog)
#   3. Install the systemd watchdog service + timer:
#        fuse-watchdog.service   — periodic health probe; auto-heals stalled mount
#        fuse-watchdog.timer     — fires every 5 minutes (configurable)
#   4. Apply kernel sysctl tuning:
#        vm.dirty_expire_centisecs    → 500     (flush dirty pages within 5 s)
#        vm.dirty_writeback_centisecs → 100     (writeback thread wakes every 1 s)
#        vm.dirty_ratio               → 5       (start synchronous writeback at 5 %)
#        vm.dirty_background_ratio    → 2       (start background writeback at 2 %)
#        fs.pipe-max-size             → 4194304 (4 MiB — matches JuiceFS wsize)
#   5. Persist sysctl settings to /etc/sysctl.d/80-fuse-remediation.conf
#   6. Enable and start the watchdog timer.
#
# Usage:
#   sudo bash scripts/install.sh [OPTIONS]
#
# Options:
#   --install-dir <path>    Target directory for scripts (default: /usr/local/bin)
#   --mount-point <path>    FUSE mount point the watchdog should monitor
#                           (default: /mnt/accfs)
#   --meta-url <url>        JuiceFS metadata URL passed to fix-fuse-mount
#                           (default: redis://127.0.0.1:6379/1)
#   --service <name>        systemd service managing the FUSE daemon
#                           (default: accfs.service; "none" to skip daemon restart)
#   --watchdog-interval <N> Watchdog timer interval in minutes (default: 5)
#   --no-sysctl             Skip kernel sysctl tuning
#   --no-watchdog           Skip systemd watchdog service/timer installation
#   --no-scripts            Skip script deployment to --install-dir
#   --dry-run               Print actions without executing them
#   --uninstall             Remove everything this script installed
#   -h, --help              Show this help and exit
#
# Environment variables:
#   FUSE_MOUNT_POINT        Same as --mount-point
#   JUICEFS_META_URL        Same as --meta-url
#   ACCFS_SERVICE           Same as --service
#   INSTALL_DIR             Same as --install-dir
#
# Exit codes:
#   0   Installation succeeded (or dry-run completed)
#   1   One or more steps failed
#
# Prerequisites:
#   - Must run as root (required for /usr/local/bin writes, sysctl, systemctl).
#   - systemd must be the init system (for watchdog service/timer).
#   - The repository root must be available (scripts are deployed from ./scripts/).
#
# See also:
#   scripts/fix_fuse_mount.sh                  — the core remediation script
#   scripts/cifs-mount-health.sh               — mount diagnostics
#   scripts/remove-stale-index-lock.sh         — standalone lock-file removal
#   scripts/setup-git-cifs.sh                  — CIFS-safe git configuration
#   docs/accfs.md                              — AccFS architecture overview
#   docs/git-index-write-failure-investigation.md — CIFS/FUSE D-state analysis

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# Defaults (overridable via environment variables or CLI flags)
# ─────────────────────────────────────────────────────────────────────────────
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
MOUNT_POINT="${FUSE_MOUNT_POINT:-/mnt/accfs}"
META_URL="${JUICEFS_META_URL:-redis://127.0.0.1:6379/1}"
DAEMON_SERVICE="${ACCFS_SERVICE:-accfs.service}"
WATCHDOG_INTERVAL=5          # minutes between watchdog probe runs
SYSCTL_CONF="/etc/sysctl.d/80-fuse-remediation.conf"
SYSTEMD_DIR="/etc/systemd/system"
WATCHDOG_SERVICE="fuse-watchdog.service"
WATCHDOG_TIMER="fuse-watchdog.timer"

DO_SYSCTL=true
DO_WATCHDOG=true
DO_SCRIPTS=true
DRY_RUN=false
UNINSTALL=false

# Locate the repository root (directory containing this script's parent)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ─────────────────────────────────────────────────────────────────────────────
# Argument parsing
# ─────────────────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)       INSTALL_DIR="$2";        shift 2 ;;
    --mount-point)       MOUNT_POINT="$2";        shift 2 ;;
    --meta-url)          META_URL="$2";           shift 2 ;;
    --service)           DAEMON_SERVICE="$2";     shift 2 ;;
    --watchdog-interval) WATCHDOG_INTERVAL="$2";  shift 2 ;;
    --no-sysctl)         DO_SYSCTL=false;         shift   ;;
    --no-watchdog)       DO_WATCHDOG=false;       shift   ;;
    --no-scripts)        DO_SCRIPTS=false;        shift   ;;
    --dry-run)           DRY_RUN=true;            shift   ;;
    --uninstall)         UNINSTALL=true;          shift   ;;
    -h|--help)
      sed -n '/^# install\.sh/,/^[^#]/{ /^[^#]/d; s/^# \?//; p }' "$0"
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
  esac
done

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────
log()  { echo "[install] $(date -u '+%H:%M:%SZ') $*"; }
warn() { echo "[install] WARN  $(date -u '+%H:%M:%SZ') $*" >&2; }
die()  { echo "[install] FATAL $(date -u '+%H:%M:%SZ') $*" >&2; exit 1; }
sep()  { echo "────────────────────────────────────────────────────────────"; }

# Execute a command, or print it in dry-run mode.
run() {
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] $*"
  else
    "$@"
  fi
}

# Write a file, or show its content in dry-run mode.
write_file() {
  local path="$1"
  local content="$2"
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] write ${path}:"
    echo "$content" | sed 's/^/  /'
  else
    echo "$content" > "$path"
  fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Preflight checks
# ─────────────────────────────────────────────────────────────────────────────
if [[ $EUID -ne 0 ]] && [[ "$DRY_RUN" != "true" ]]; then
  die "This script must be run as root (or via sudo). Use --dry-run for a non-root preview."
fi

sep
log "FUSE Remediation Installer"
log "Repo root     : ${REPO_ROOT}"
log "Install dir   : ${INSTALL_DIR}"
log "Mount point   : ${MOUNT_POINT}"
log "Meta URL      : ${META_URL}"
log "FUSE service  : ${DAEMON_SERVICE}"
log "Watchdog every: ${WATCHDOG_INTERVAL} min"
log "Deploy scripts: ${DO_SCRIPTS}"
log "Install wdog  : ${DO_WATCHDOG}"
log "Apply sysctl  : ${DO_SYSCTL}"
log "Dry run       : ${DRY_RUN}"
log "Uninstall     : ${UNINSTALL}"
sep

# Validate watchdog interval is a positive integer
if ! [[ "$WATCHDOG_INTERVAL" =~ ^[1-9][0-9]*$ ]]; then
  die "--watchdog-interval must be a positive integer (got: '${WATCHDOG_INTERVAL}')"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Uninstall path
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$UNINSTALL" == "true" ]]; then
  log "=== Uninstalling FUSE remediation toolkit ==="

  # Stop and disable the watchdog timer/service
  if command -v systemctl > /dev/null 2>&1; then
    for unit in "${WATCHDOG_TIMER}" "${WATCHDOG_SERVICE}"; do
      if systemctl list-unit-files "$unit" 2>/dev/null | grep -q "$unit"; then
        log "  Stopping and disabling ${unit}…"
        run systemctl stop    "$unit" 2>/dev/null || true
        run systemctl disable "$unit" 2>/dev/null || true
      fi
    done
    log "  Removing systemd unit files…"
    run rm -f "${SYSTEMD_DIR}/${WATCHDOG_SERVICE}" "${SYSTEMD_DIR}/${WATCHDOG_TIMER}"
    run systemctl daemon-reload
  fi

  # Remove sysctl configuration
  if [[ -f "$SYSCTL_CONF" ]]; then
    log "  Removing sysctl config: ${SYSCTL_CONF}"
    run rm -f "$SYSCTL_CONF"
    # Reload sysctl without our custom file; values revert on next boot.
    run sysctl --system > /dev/null 2>&1 || true
  fi

  # Remove deployed scripts
  for script_name in fix-fuse-mount cifs-mount-health remove-stale-index-lock setup-git-cifs fuse-watchdog; do
    target="${INSTALL_DIR}/${script_name}"
    if [[ -f "$target" ]]; then
      log "  Removing ${target}"
      run rm -f "$target"
    fi
  done

  sep
  log "Uninstall complete."
  exit 0
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 1: Deploy remediation scripts to INSTALL_DIR
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$DO_SCRIPTS" == "true" ]]; then
  log "=== Step 1: Deploying remediation scripts to ${INSTALL_DIR} ==="

  run mkdir -p "${INSTALL_DIR}"

  # Map: source file in scripts/ → installed name in INSTALL_DIR
  # The installed names use hyphens (POSIX-friendly) instead of underscores.
  declare -A SCRIPT_MAP=(
    ["${SCRIPT_DIR}/fix_fuse_mount.sh"]="${INSTALL_DIR}/fix-fuse-mount"
    ["${SCRIPT_DIR}/cifs-mount-health.sh"]="${INSTALL_DIR}/cifs-mount-health"
    ["${SCRIPT_DIR}/remove-stale-index-lock.sh"]="${INSTALL_DIR}/remove-stale-index-lock"
    ["${SCRIPT_DIR}/setup-git-cifs.sh"]="${INSTALL_DIR}/setup-git-cifs"
    # fuse-watchdog: long-running polling daemon that aborts stalled FUSE
    # connections by writing to /sys/fs/fuse/connections/*/abort.  Deployed
    # here so fuse-watchdog.service can call it with --one-shot (the systemd
    # oneshot pattern) rather than shelling out to fix-fuse-mount.  Running
    # it with --one-shot in ExecStart is the intended mode for
    # timer-activated services; the daemon's continuous-polling mode is
    # reserved for supervisor- or screen-based deployments.
    ["${SCRIPT_DIR}/fuse-watchdog"]="${INSTALL_DIR}/fuse-watchdog"
  )

  DEPLOY_OK=true
  for src in "${!SCRIPT_MAP[@]}"; do
    dst="${SCRIPT_MAP[$src]}"
    if [[ ! -f "$src" ]]; then
      warn "  Source not found: ${src} — skipping"
      DEPLOY_OK=false
      continue
    fi
    log "  Installing: $(basename "$src") → ${dst}"
    if [[ "$DRY_RUN" == "true" ]]; then
      echo "[DRY-RUN] install -m 755 ${src} ${dst}"
    else
      install -m 755 "$src" "$dst"
    fi
  done

  if [[ "$DEPLOY_OK" == "true" ]]; then
    log "  All remediation scripts installed."
  else
    die "One or more source scripts were not found — aborting to prevent a partial install." \
        "Ensure you are running from the repository root: cd ${REPO_ROOT}"
  fi
else
  log "=== Step 1: Skipping script deployment (--no-scripts) ==="
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 2: Install systemd watchdog service and timer
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$DO_WATCHDOG" == "true" ]]; then
  log "=== Step 2: Installing systemd watchdog service and timer ==="

  if ! command -v systemctl > /dev/null 2>&1; then
    warn "  systemctl not found — cannot install watchdog (non-systemd host?)."
    warn "  Skip this step with --no-watchdog if systemd is not available."
  else
    # ── fuse-watchdog.service ────────────────────────────────────────────────
    # The service runs fuse-watchdog in --one-shot mode, which polls
    # /sys/fs/fuse/connections once, aborts any stalled connection (waiting
    # counter >= threshold), and exits cleanly.  Using --one-shot here is
    # the correct systemd pattern: the timer drives the cadence and
    # Type=oneshot lets systemd track success/failure of each probe.  The
    # daemon's continuous-polling mode (without --one-shot) is reserved for
    # supervisor- or screen-based deployments where the process is expected
    # to stay resident.
    #
    # Key settings:
    #   Type=oneshot        — systemd waits for the script to exit before the
    #                         unit transitions to "inactive".
    #   TimeoutStartSec=120 — the entire probe must complete within 2 min;
    #                         this prevents the watchdog itself from hanging on
    #                         a fully stalled mount.
    #   PrivateTmp=true     — tmpfs /tmp inside the service namespace; avoids
    #                         cross-contamination if /tmp is on the FUSE mount.
    #   ProtectSystem=full  — read-only bind-mounts /usr and /boot; the script
    #                         only needs to write to /sys and /mnt.
    #   CapabilityBoundingSet — restrict to the capabilities actually needed:
    #     CAP_SYS_ADMIN       unmount, write to /sys/fs/fuse/connections
    #     CAP_KILL            SIGKILL across UIDs
    #     CAP_DAC_OVERRIDE    remove lock files owned by other UIDs
    # ─────────────────────────────────────────────────────────────────────────
    WATCHDOG_SERVICE_CONTENT="[Unit]
Description=FUSE mount watchdog — abort stalled JuiceFS/FUSE connections
Documentation=https://github.com/ccc-org/ccc
# Don't start if the network is not yet up (the FUSE daemon needs the network)
After=network-online.target
Wants=network-online.target
# Allow the timer to restart us even if a previous run failed
StartLimitIntervalSec=0

[Service]
Type=oneshot
# Run as root — required for /sys/fs/fuse/connections writes and CAP_SYS_ADMIN
User=root

# Core invocation — poll /sys/fs/fuse/connections once and abort any
# connection whose 'waiting' counter >= threshold, then exit.
# --one-shot is the intended mode for timer-activated systemd services.
ExecStart=${INSTALL_DIR}/fuse-watchdog --one-shot

# Hard deadline — the entire probe must complete in 2 minutes.
# If it doesn't, systemd kills the watchdog process itself and logs a failure.
# This prevents the watchdog from being stuck on a totally unrecoverable mount.
TimeoutStartSec=120

# Log to the journal with a stable identifier for easy filtering:
#   journalctl -u fuse-watchdog -f
#   journalctl -u fuse-watchdog --since '1 hour ago'
StandardOutput=journal
StandardError=journal
SyslogIdentifier=fuse-watchdog

# Harden the service namespace
PrivateTmp=true
ProtectSystem=full
CapabilityBoundingSet=CAP_SYS_ADMIN CAP_KILL CAP_DAC_OVERRIDE

[Install]
WantedBy=multi-user.target"

    # ── fuse-watchdog.timer ──────────────────────────────────────────────────
    # The timer activates the service on a fixed interval.
    #
    # OnBootSec=60       — first probe fires 60 s after boot, giving the FUSE
    #                      daemon time to start before we check it.
    # OnUnitActiveSec    — subsequent probes every WATCHDOG_INTERVAL minutes.
    # Persistent=true    — if the system was off during a scheduled run, the
    #                      timer catches up and fires once on next boot.
    # AccuracySec=30     — allows systemd to coalesce with other timers within
    #                      a 30-second window (reduces wake-up overhead).
    # ─────────────────────────────────────────────────────────────────────────
    WATCHDOG_TIMER_CONTENT="[Unit]
Description=FUSE mount watchdog timer — probe every ${WATCHDOG_INTERVAL} minutes
Documentation=https://github.com/ccc-org/ccc

[Timer]
# Initial probe 60 s after boot (give the FUSE daemon time to start)
OnBootSec=60
# Subsequent probes every ${WATCHDOG_INTERVAL} minutes
OnUnitActiveSec=${WATCHDOG_INTERVAL}min
# Fire a catch-up probe if any scheduled run was missed (e.g. system was off)
Persistent=true
# Allow up to 30 s of scheduling slack so systemd can coalesce timer wakeups
AccuracySec=30

[Install]
WantedBy=timers.target"

    log "  Writing ${SYSTEMD_DIR}/${WATCHDOG_SERVICE}…"
    write_file "${SYSTEMD_DIR}/${WATCHDOG_SERVICE}" "$WATCHDOG_SERVICE_CONTENT"

    log "  Writing ${SYSTEMD_DIR}/${WATCHDOG_TIMER}…"
    write_file "${SYSTEMD_DIR}/${WATCHDOG_TIMER}" "$WATCHDOG_TIMER_CONTENT"

    log "  Reloading systemd daemon…"
    run systemctl daemon-reload

    log "  Enabling ${WATCHDOG_TIMER} (starts on boot)…"
    run systemctl enable "${WATCHDOG_TIMER}"

    log "  Starting ${WATCHDOG_TIMER} (starts now without reboot)…"
    run systemctl start "${WATCHDOG_TIMER}"

    if [[ "$DRY_RUN" != "true" ]]; then
      # Brief verification
      if systemctl is-active --quiet "${WATCHDOG_TIMER}" 2>/dev/null; then
        log "  ${WATCHDOG_TIMER} is active ✓"
        log "  Next run: $(systemctl show --property=NextElapseUSecRealtime --value "${WATCHDOG_TIMER}" 2>/dev/null || echo '(unknown)')"
      else
        warn "  ${WATCHDOG_TIMER} did not become active — check: systemctl status ${WATCHDOG_TIMER}"
      fi
    fi
  fi
else
  log "=== Step 2: Skipping watchdog installation (--no-watchdog) ==="
fi

# ─────────────────────────────────────────────────────────────────────────────
# Step 3: Apply kernel sysctl tuning
# ─────────────────────────────────────────────────────────────────────────────
# Rationale for each setting:
#
# vm.dirty_expire_centisecs = 500
#   Dirty pages are written back to disk (or network) within 5 s (default:
#   3000 cs = 30 s).  On a FUSE/CIFS mount a 30-second dirty-page window means
#   a git index write can sit in the page cache for up to 30 s before the
#   kernel tries to flush it.  If the FUSE daemon is dead or the server is
#   full, that flush attempt blocks the writing process in D-state for the
#   entire dirty-expire window.  Reducing to 5 s shrinks the stall window
#   dramatically.
#
# vm.dirty_writeback_centisecs = 100
#   The writeback thread wakes every 1 s (default: 500 cs = 5 s) to flush
#   dirty pages that have passed dirty_expire_centisecs.  More frequent
#   wakeups mean stalls surface sooner and the watchdog can react faster.
#
# vm.dirty_ratio = 5
#   When dirty pages exceed 5 % of total RAM, new writes block synchronously
#   until writeback catches up (default: 20 %).  This prevents a runaway
#   dirty-page build-up that would cause widespread D-state if the FUSE daemon
#   becomes unresponsive.
#
# vm.dirty_background_ratio = 2
#   Background writeback starts at 2 % of RAM dirty (default: 10 %).
#   Starting earlier keeps the dirty set small so synchronous writeback
#   pressure (above) is rarely reached under normal conditions.
#
# fs.pipe-max-size = 4194304
#   Maximum pipe buffer size — 4 MiB, matching JuiceFS's default wsize/rsize
#   (4194304 bytes in the CIFS mount options observed in production).  Tools
#   that splice data through pipes between the FUSE daemon and the kernel
#   avoid short writes when the pipe can buffer a full FUSE request at once.
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$DO_SYSCTL" == "true" ]]; then
  log "=== Step 3: Applying kernel sysctl tuning ==="

  SYSCTL_CONTENT="# /etc/sysctl.d/80-fuse-remediation.conf
# Kernel tunables to reduce FUSE/JuiceFS mount stall frequency and severity.
# Applied by: scripts/install.sh (FUSE remediation installer)
# See: docs/git-index-write-failure-investigation.md
#
# Flush dirty pages to the FUSE/CIFS server within 5 s (default: 30 s).
# A shorter window means D-state stalls resolve faster when the daemon is slow.
vm.dirty_expire_centisecs = 500

# Wake the writeback thread every 1 s (default: 5 s) to flush expired pages.
vm.dirty_writeback_centisecs = 100

# Start synchronous writeback when dirty pages exceed 5 % of RAM (default: 20 %).
# Prevents dirty-page build-up from causing widespread D-state under load.
vm.dirty_ratio = 5

# Start background writeback at 2 % of RAM dirty (default: 10 %).
# Keeps the dirty set small so the synchronous threshold is rarely hit.
vm.dirty_background_ratio = 2

# Pipe buffer limit — 4 MiB, matching JuiceFS wsize/rsize mount options.
# Avoids short writes when splicing data between the FUSE daemon and kernel.
fs.pipe-max-size = 4194304
"

  log "  Writing ${SYSCTL_CONF}…"
  write_file "$SYSCTL_CONF" "$SYSCTL_CONTENT"

  # Apply the new settings immediately (without rebooting).
  # sysctl --system loads all /etc/sysctl.d/*.conf files.
  log "  Applying sysctl settings (sysctl --system)…"
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[DRY-RUN] sysctl --system"
  else
    # Suppress the flood of output from --system; show only our own file's lines.
    sysctl --system 2>&1 | grep -E 'vm\.dirty|fs\.pipe' | sed 's/^/  /' || true
    log "  Sysctl tuning applied."
    log "  Active values:"
    for key in vm.dirty_expire_centisecs vm.dirty_writeback_centisecs \
                vm.dirty_ratio vm.dirty_background_ratio fs.pipe-max-size; do
      val=$(sysctl -n "$key" 2>/dev/null || echo "(error)")
      log "    ${key} = ${val}"
    done
  fi
else
  log "=== Step 3: Skipping sysctl tuning (--no-sysctl) ==="
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
sep
log "Installation complete."
log ""
log "Installed commands (${INSTALL_DIR}):"
if [[ "$DO_SCRIPTS" == "true" ]]; then
  log "  fix-fuse-mount          — full FUSE stall remediation (abort/kill/remount)"
  log "  cifs-mount-health       — mount diagnostics (responsiveness, disk, D-state)"
  log "  remove-stale-index-lock — safely remove stale .git/index.lock files"
  log "  setup-git-cifs          — apply CIFS-safe git settings to a repo"
  log "  fuse-watchdog           — poll /sys/fs/fuse/connections and abort stalled connections"
fi
log ""
log "Systemd units:"
if [[ "$DO_WATCHDOG" == "true" ]]; then
  log "  ${WATCHDOG_SERVICE}  — one-shot remediation service"
  log "  ${WATCHDOG_TIMER}    — fires every ${WATCHDOG_INTERVAL} min"
  log ""
  log "  Watchdog commands:"
  log "    systemctl status ${WATCHDOG_TIMER}          # timer status + next run"
  log "    journalctl -u fuse-watchdog -f              # follow watchdog logs"
  log "    systemctl start ${WATCHDOG_SERVICE}         # run a probe now"
fi
log ""
log "Kernel sysctl:"
if [[ "$DO_SYSCTL" == "true" ]]; then
  log "  Config written to: ${SYSCTL_CONF}"
  log "  Settings are persistent across reboots."
fi
log ""
log "Manual remediation:"
log "  sudo fix-fuse-mount --mount-point ${MOUNT_POINT}"
log "  sudo fix-fuse-mount --mount-point ${MOUNT_POINT} --dry-run"
log "  cifs-mount-health --mount-path ${MOUNT_POINT}"
log "  remove-stale-index-lock --repo /path/to/repo"
log ""
log "Uninstall:"
log "  sudo bash scripts/install.sh --uninstall"
sep
