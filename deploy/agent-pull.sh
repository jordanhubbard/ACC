#!/usr/bin/env bash
# agent-pull.sh — git pull then hand off to acc-agent upgrade.
# Kept as a compatibility entry point for nodes that haven't yet
# updated acc-agent. Once all nodes run acc-agent >= the upgrade
# subcommand, this file can be deleted.
set -euo pipefail

ACC_DIR="${HOME}/.acc"
[[ -d "${HOME}/.acc" ]] || ACC_DIR="${HOME}/.ccc"
source "${ACC_DIR}/.env" 2>/dev/null || true
WORKSPACE="${ACC_DIR}/workspace"
LOG_FILE="${ACC_DIR}/logs/pull.log"
mkdir -p "${ACC_DIR}/logs"

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] [agent-pull] $*" | tee -a "${LOG_FILE}"; }

log "Starting pull -> ${WORKSPACE}"
cd "${WORKSPACE}"
git fetch origin --quiet 2>>"${LOG_FILE}"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
git merge --ff-only "origin/${BRANCH}" --quiet 2>>"${LOG_FILE}"
log "Pull complete ($(git rev-parse --short HEAD))"

# Hand off to acc-agent upgrade (handles migrations + restarts + heartbeat)
ACC_AGENT="${HOME}/.acc/bin/acc-agent"
[[ -x "${ACC_AGENT}" ]] || ACC_AGENT="$(command -v acc-agent 2>/dev/null || echo "")"
if [[ -n "${ACC_AGENT}" ]]; then
    log "Handing off to acc-agent upgrade"
    exec "${ACC_AGENT}" upgrade "$@"
else
    log "WARNING: acc-agent not found -- running legacy run-migrations.sh fallback"
    bash "${WORKSPACE}/deploy/run-migrations.sh" 2>>"${LOG_FILE}" || true
fi
