#!/usr/bin/env bash
# restart-hub.sh — Stop any running acc-server process and relaunch it with
# / as its working directory so relative-path assumptions never bite.
#
# Usage:
#   bash deploy/restart-hub.sh
#   SERVER_DEST=/usr/local/bin/acc-server LOG_DIR=~/.acc/logs bash deploy/restart-hub.sh
#
# Environment (all have sensible defaults):
#   SERVER_DEST   Path to the acc-server binary  [/usr/local/bin/acc-server]
#   LOG_DIR       Directory for server log files  [~/.acc/logs]
#   ENV_FILE      .env to source before launch    [~/.acc/.env]
#
# The subshell pattern used here:
#   ( cd / && nohup "$SERVER_DEST" >> "${LOG_DIR}/acc-server.log" 2>&1 & )
# ensures acc-server always inherits / as its working directory regardless of
# where this script is invoked from.

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────
ACC_DIR="${HOME}/.acc"
SERVER_DEST="${SERVER_DEST:-/usr/local/bin/acc-server}"
LOG_DIR="${LOG_DIR:-${ACC_DIR}/logs}"
ENV_FILE="${ENV_FILE:-${ACC_DIR}/.env}"
PID_FILE="${ACC_DIR}/run/acc-server.pid"

# ── Helpers ───────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; BLUE='\033[0;34m'; NC='\033[0m'
info()  { echo -e "${BLUE}[restart-hub]${NC} $*"; }
ok()    { echo -e "${GREEN}[restart-hub]${NC} ✓ $*"; }
warn()  { echo -e "${YELLOW}[restart-hub]${NC} ⚠ $*"; }
error() { echo -e "${RED}[restart-hub]${NC} ✗ $*" >&2; exit 1; }

# ── Pre-flight checks ──────────────────────────────────────────────────────
[[ -x "${SERVER_DEST}" ]] || error "acc-server binary not found or not executable: ${SERVER_DEST}"

mkdir -p "${LOG_DIR}" "${ACC_DIR}/run"

# ── Load environment ───────────────────────────────────────────────────────
if [[ -f "${ENV_FILE}" ]]; then
  set -a
  # shellcheck source=/dev/null
  source "${ENV_FILE}"
  set +a
  info "Loaded env from ${ENV_FILE}"
else
  warn "No .env found at ${ENV_FILE} — continuing with current environment"
fi

# ── Stop existing acc-server process ──────────────────────────────────────
# Try PID file first, then fall back to pkill.
_stopped=false

if [[ -f "${PID_FILE}" ]]; then
  _old_pid=$(<"${PID_FILE}")
  if kill -0 "${_old_pid}" 2>/dev/null; then
    info "Stopping acc-server (PID ${_old_pid})..."
    kill "${_old_pid}" 2>/dev/null || true
    # Wait up to 5 s for a clean shutdown
    for _i in 1 2 3 4 5; do
      kill -0 "${_old_pid}" 2>/dev/null || { _stopped=true; break; }
      sleep 1
    done
    if [[ "${_stopped}" != "true" ]]; then
      warn "acc-server (PID ${_old_pid}) did not exit in 5 s — sending SIGKILL"
      kill -9 "${_old_pid}" 2>/dev/null || true
    fi
  fi
  rm -f "${PID_FILE}"
fi

# Belt-and-suspenders: kill any other acc-server processes owned by this user
if pkill -u "$(id -u)" -x acc-server 2>/dev/null; then
  sleep 1
  info "Killed stale acc-server process(es) via pkill"
fi

# ── Launch with / as working directory ────────────────────────────────────
info "Starting acc-server → ${SERVER_DEST}"
info "Log: ${LOG_DIR}/acc-server.log"

(
  cd /
  nohup "${SERVER_DEST}" >> "${LOG_DIR}/acc-server.log" 2>&1 &
  _new_pid=$!
  echo "${_new_pid}" > "${PID_FILE}"
  echo "${_new_pid}"
)

_launched_pid=$(<"${PID_FILE}")
ok "acc-server launched (PID ${_launched_pid}) with working directory /"
ok "Tail logs: tail -f ${LOG_DIR}/acc-server.log"
