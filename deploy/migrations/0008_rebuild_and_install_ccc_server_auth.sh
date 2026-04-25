# Description: Rebuild ccc-server with auth support and reinstall binary
#
# Context: ccc-server gained admin-provisioned user auth (routes/auth.rs).
#   The binary at /usr/local/bin/ccc-server must be rebuilt from source
#   and reinstalled. Auth DB initialises automatically at ~/.ccc/auth.db
#   on first start — no further action needed on non-hub nodes.
# Condition: linux (systemd), hub nodes only (where ccc-server runs)

if [ "${IS_HUB:-false}" = "true" ] || systemctl is-active --quiet ccc-server 2>/dev/null; then
    on_platform linux

    CARGO="${HOME}/.cargo/bin/cargo"
    BUILD_DIR="$WORKSPACE/ccc/dashboard"
    BINARY="$BUILD_DIR/target/release/ccc-server"
    INSTALL_PATH="/usr/local/bin/ccc-server"

    if [ ! -x "$CARGO" ]; then
        m_warn "cargo not found at $CARGO — skipping rebuild (manual: cargo build in $BUILD_DIR)"
    else
        m_info "Building ccc-server (auth support)..."
        (cd "$BUILD_DIR" && "$CARGO" build --release --quiet) \
            || { m_warn "cargo build failed — skipping binary install"; return 0; }
        m_success "ccc-server built"

        m_info "Installing binary to $INSTALL_PATH..."
        sudo systemctl stop ccc-server 2>/dev/null || true
        sudo cp "$BINARY" "$INSTALL_PATH"
        sudo systemctl start ccc-server 2>/dev/null \
            && m_success "ccc-server restarted with auth binary" \
            || m_warn "ccc-server failed to start — check journalctl -u ccc-server"
    fi
fi
