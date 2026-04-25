#!/usr/bin/env bash
# Description: Rename stale ccc-* systemd/launchd units to acc-* counterparts
#
# Context: deploy/systemd/ccc-agent.timer → acc-agent.timer and
# deploy/launchd/com.ccc.{agent,claude-main,consul,exec-listen}.plist →
# com.acc.* were renamed in the repo. This migration stops/unloads the old
# units and installs the renamed acc-* equivalents on each node.
#
# The com.ccc.{bus-listener,queue-worker} plist renames were handled by
# migration 0016; this handles the remaining four launchd agents plus the
# Linux timer.
#
# Condition: linux nodes (timer), macos nodes (launchd agents)

set -euo pipefail

# ── Linux: swap ccc-agent.timer → acc-agent.timer ──────────────────────────
if on_platform linux; then
  m_info "Linux: replacing ccc-agent.timer with acc-agent.timer"

  # Stop and disable the old timer (best-effort)
  if systemctl is-active --quiet ccc-agent.timer 2>/dev/null; then
    sudo systemctl stop ccc-agent.timer && m_success "stopped ccc-agent.timer"
  fi
  if systemctl is-enabled --quiet ccc-agent.timer 2>/dev/null; then
    sudo systemctl disable ccc-agent.timer && m_success "disabled ccc-agent.timer"
  fi

  # Remove old unit file if it exists
  OLD_TIMER="/etc/systemd/system/ccc-agent.timer"
  if [[ -f "$OLD_TIMER" ]]; then
    sudo rm -f "$OLD_TIMER" && m_success "removed ${OLD_TIMER}"
  fi

  # Install new acc-agent.timer
  systemd_install deploy/systemd/acc-agent.timer acc-agent.timer
  m_success "acc-agent.timer installed and started"

  sudo systemctl daemon-reload
fi

# ── macOS: unload old com.ccc.* agents, load new com.acc.* agents ──────────
if on_platform macos; then
  AGENTS_DIR="${HOME}/Library/LaunchAgents"
  DEPLOY_DIR="${WORKSPACE}/deploy/launchd"

  # Pairs: OLD_PLIST → NEW_LABEL:NEW_PLIST
  declare -a OLD_PLISTS=(
    "com.ccc.agent.plist"
    "com.ccc.claude-main.plist"
    "com.ccc.consul.plist"
    "com.ccc.exec-listen.plist"
  )
  declare -a NEW_PLISTS=(
    "com.acc.agent.plist"
    "com.acc.claude-main.plist"
    "com.acc.consul.plist"
    "com.acc.exec-listen.plist"
  )
  declare -a NEW_LABELS=(
    "com.acc.agent"
    "com.acc.claude-main"
    "com.acc.consul"
    "com.acc.exec-listen"
  )

  for i in "${!OLD_PLISTS[@]}"; do
    old="${AGENTS_DIR}/${OLD_PLISTS[$i]}"
    new_src="${DEPLOY_DIR}/${NEW_PLISTS[$i]}"
    new_dst="${AGENTS_DIR}/${NEW_PLISTS[$i]}"
    label="${NEW_LABELS[$i]}"

    # Unload old agent if loaded
    if [[ -f "$old" ]]; then
      launchctl unload "$old" 2>/dev/null && m_success "unloaded ${OLD_PLISTS[$i]}" || true
      rm -f "$old" && m_success "removed ${old}"
    else
      m_skip "${OLD_PLISTS[$i]} not installed — skipping unload"
    fi

    # Install new agent (skip if source template doesn't exist for this node type)
    if [[ ! -f "$new_src" ]]; then
      m_skip "${NEW_PLISTS[$i]} template not found in deploy/ — skipping"
      continue
    fi

    # Substitute AGENT_HOME/AGENT_USER placeholders
    mkdir -p "$AGENTS_DIR"
    sed "s|AGENT_HOME|${HOME}|g; s|AGENT_USER|${USER}|g" "$new_src" > "$new_dst"
    chmod 644 "$new_dst"

    # Only load if the agent isn't already running
    if launchctl list 2>/dev/null | grep -q "$label"; then
      m_skip "${label} already loaded"
    else
      launchctl load "$new_dst" 2>/dev/null \
        && m_success "loaded ${label}" \
        || m_warn "launchctl load ${NEW_PLISTS[$i]} returned non-zero — check: launchctl list | grep acc"
    fi
  done
fi

m_success "Migration 0020 complete — all ccc-* units replaced with acc-* equivalents"
