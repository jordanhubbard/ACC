#!/usr/bin/env bash
# 0023_add_hermes_worker_disable_exec_listen.sh
#
# 1. Disables acc-exec-listen (duplicate of acc-bus-listener — caused double execution)
# 2. Installs acc-hermes-worker on nodes where `hermes` is available
# 3. Installs default commands.json (structured exec registry, replaces raw shell exec)
set -euo pipefail

ACC_DIR="${HOME}/.acc"
WORKSPACE="${ACC_DIR}/workspace"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
ok()   { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

# ── 1. Disable acc-exec-listen (Linux) ───────────────────────────────────────
if command -v systemctl &>/dev/null; then
    if systemctl is-enabled --quiet acc-exec-listen 2>/dev/null; then
        systemctl stop acc-exec-listen 2>/dev/null || true
        systemctl disable acc-exec-listen 2>/dev/null || true
        ok "Disabled acc-exec-listen.service (was duplicate of acc-bus-listener)"
    else
        ok "acc-exec-listen not enabled — skip"
    fi
fi

# ── 1. Disable com.acc.exec-listen (macOS) ───────────────────────────────────
if command -v launchctl &>/dev/null; then
    if launchctl list 2>/dev/null | grep -q "com.acc.exec-listen"; then
        launchctl unload "${HOME}/Library/LaunchAgents/com.acc.exec-listen.plist" 2>/dev/null || true
        ok "Unloaded com.acc.exec-listen (was duplicate of com.acc.bus-listener)"
    else
        ok "com.acc.exec-listen not loaded — skip"
    fi
fi

# ── 2. Install commands.json (exec registry) ─────────────────────────────────
COMMANDS_SRC="${WORKSPACE}/deploy/commands.json"
COMMANDS_DST="${ACC_DIR}/commands.json"

if [[ -f "$COMMANDS_SRC" ]]; then
    if [[ -f "$COMMANDS_DST" ]]; then
        # Back up existing custom commands before overwriting defaults
        cp "$COMMANDS_DST" "${COMMANDS_DST}.bak"
        warn "Backed up existing commands.json → commands.json.bak"
    fi
    cp "$COMMANDS_SRC" "$COMMANDS_DST"
    ok "Installed commands.json"
else
    warn "commands.json not found in workspace — skip"
fi

# ── 3. Install acc-hermes-worker (Linux, only if hermes is available) ─────────
if command -v systemctl &>/dev/null; then
    if command -v hermes &>/dev/null || [[ -x "${ACC_DIR}/bin/hermes" ]]; then
        UNIT_SRC="${WORKSPACE}/deploy/systemd/acc-hermes-worker.service"
        UNIT_DST="/etc/systemd/system/acc-hermes-worker.service"

        if [[ -f "$UNIT_SRC" ]]; then
            AGENT_USER="$(whoami)"
            AGENT_HOME="${HOME}"
            sed \
                -e "s|AGENT_USER|${AGENT_USER}|g" \
                -e "s|AGENT_HOME|${AGENT_HOME}|g" \
                "$UNIT_SRC" > "$UNIT_DST"

            systemctl daemon-reload
            systemctl enable --now acc-hermes-worker
            ok "Installed and started acc-hermes-worker.service"
        else
            warn "acc-hermes-worker.service template not found — skip"
        fi
    else
        ok "hermes not found on this node — skipping acc-hermes-worker"
    fi
fi

# ── 3. Install com.acc.hermes-worker (macOS, only if hermes is available) ─────
if command -v launchctl &>/dev/null; then
    if command -v hermes &>/dev/null || [[ -x "${ACC_DIR}/bin/hermes" ]]; then
        PLIST_SRC="${WORKSPACE}/deploy/launchd/com.acc.hermes-worker.plist"
        PLIST_DST="${HOME}/Library/LaunchAgents/com.acc.hermes-worker.plist"

        if [[ -f "$PLIST_SRC" ]]; then
            AGENT_HOME="${HOME}"
            sed "s|AGENT_HOME|${AGENT_HOME}|g" "$PLIST_SRC" > "$PLIST_DST"
            launchctl load "$PLIST_DST"
            ok "Installed and loaded com.acc.hermes-worker.plist"
        else
            warn "com.acc.hermes-worker.plist template not found — skip"
        fi
    else
        ok "hermes not found on this node — skipping com.acc.hermes-worker"
    fi
fi
