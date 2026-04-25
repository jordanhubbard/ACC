# Description: Install ccc-exec-listen (Rust ClawBus exec daemon, replaces agent-listener.mjs)
#
# Context: ccc-agent listen connects to the CCC bus SSE stream and executes
#   ccc.exec messages via /bin/sh. This replaces agent-listener.mjs which was
#   removed in migration 0006. Requires ccc-agent binary (migration 0011).
#   Also tears down setup-container.sh's legacy exec-listener wrapper if present.
# Condition: all platforms

# ── Require ccc-agent binary ──────────────────────────────────────────────
CCC_AGENT="${CCC_AGENT:-$HOME/.ccc/bin/ccc-agent}"
if [ ! -x "$CCC_AGENT" ]; then
    m_warn "ccc-agent not found — run migration 0011 first"
    return 0
fi

# ── Remove legacy exec-listener wrapper (setup-container.sh artefact) ─────
LEGACY="$HOME/.ccc/ccc-exec-listener.sh"
if [ -f "$LEGACY" ]; then
    rm -f "$LEGACY"
    m_success "removed legacy ccc-exec-listener.sh"
fi

# ── Install service ────────────────────────────────────────────────────────
mkdir -p "$HOME/.ccc/logs"

if on_platform linux; then
    SVC_SRC="$WORKSPACE/deploy/systemd/ccc-exec-listen.service"
    if [ ! -f "$SVC_SRC" ]; then
        m_warn "ccc-exec-listen.service template not found: $SVC_SRC"
        return 1
    fi
    sed -e "s|AGENT_USER|$(whoami)|g" \
        -e "s|AGENT_HOME|$HOME|g" \
        "$SVC_SRC" | sudo tee /etc/systemd/system/ccc-exec-listen.service > /dev/null
    sudo systemctl daemon-reload
    sudo systemctl enable ccc-exec-listen.service 2>/dev/null || true
    sudo systemctl restart ccc-exec-listen.service \
        && m_success "ccc-exec-listen.service started" \
        || m_warn "ccc-exec-listen.service failed to start — check: journalctl -u ccc-exec-listen"
fi

if on_platform macos; then
    PLIST_SRC="$WORKSPACE/deploy/launchd/com.ccc.exec-listen.plist"
    PLIST_DST="$HOME/Library/LaunchAgents/com.ccc.exec-listen.plist"
    if [ ! -f "$PLIST_SRC" ]; then
        m_warn "com.ccc.exec-listen.plist not found: $PLIST_SRC"
        return 1
    fi
    launchctl unload "$PLIST_DST" 2>/dev/null || true
    sed "s|AGENT_HOME|$HOME|g" "$PLIST_SRC" > "$PLIST_DST"
    launchctl load "$PLIST_DST" \
        && m_success "com.ccc.exec-listen loaded" \
        || m_warn "launchctl load failed — check: tail -f $HOME/.ccc/logs/exec-listen.log"
fi
