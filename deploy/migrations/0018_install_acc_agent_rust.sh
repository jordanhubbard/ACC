#!/usr/bin/env bash
# Description: Build and install acc-agent (Rust) — replaces Python/shell daemons.
#
# Absorbs: bus-listener.sh, queue-worker.py, hermes-driver.py, nvidia-proxy.py
# into a single Rust binary at ~/.acc/bin/acc-agent.
#
# Idempotent. Safe to re-run; will rebuild if source is newer than binary.

set -euo pipefail

ACC_DEST="${HOME}/.acc"
[[ -d "$ACC_DEST" ]] || ACC_DEST="${HOME}/.ccc"
BIN_DIR="${ACC_DEST}/bin"
WORKSPACE="${ACC_DEST}/workspace"
SOURCE_DIR="${WORKSPACE}/agent"
BINARY="${BIN_DIR}/acc-agent"

m_info "Install acc-agent (Rust binary — replaces bus-listener.sh / queue-worker.py / nvidia-proxy.py / hermes-driver.py)"

# ── Step 1: Verify source tree ────────────────────────────────────────────────
if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]]; then
  m_warn "Source not found at ${SOURCE_DIR} — pull latest workspace first"
  m_warn "Run: bash ${WORKSPACE}/deploy/agent-pull.sh"
  exit 1
fi
m_success "Source found: ${SOURCE_DIR}"

# ── Step 2: Install Rust if missing ──────────────────────────────────────────
if ! command -v cargo &>/dev/null && [[ ! -f "$HOME/.cargo/bin/cargo" ]]; then
  m_info "Rust not found — installing via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
  m_success "Rust installed"
fi

# Ensure cargo is in PATH for this script
export PATH="${HOME}/.cargo/bin:${PATH}"

if ! command -v cargo &>/dev/null; then
  m_warn "cargo still not found after install — cannot build"
  exit 1
fi
m_success "cargo $(cargo --version)"

# ── Step 3: Build ─────────────────────────────────────────────────────────────
m_info "Building acc-agent (release)..."
cargo build --release --manifest-path "${SOURCE_DIR}/Cargo.toml" \
  && m_success "Build succeeded" \
  || { m_warn "Build failed"; exit 1; }

BUILT="${SOURCE_DIR}/target/release/acc-agent"
if [[ ! -f "$BUILT" ]]; then
  m_warn "Binary not found at ${BUILT} after build"
  exit 1
fi

# ── Step 4: Install binary ────────────────────────────────────────────────────
mkdir -p "$BIN_DIR"
install -m 755 "$BUILT" "$BINARY"
m_success "Installed ${BINARY} ($(du -sh "$BINARY" | cut -f1))"

# Symlink ccc-agent → acc-agent for backward compat
ln -sf "$BINARY" "${BIN_DIR}/ccc-agent"
m_success "Symlink: ${BIN_DIR}/ccc-agent → acc-agent"

# ── Step 5: Install/update systemd services (Linux) ──────────────────────────
if on_platform linux; then
  TMPL_DIR="${WORKSPACE}/deploy/systemd"

  for SVC in acc-bus-listener acc-queue-worker acc-nvidia-proxy; do
    TMPL="${TMPL_DIR}/${SVC}.service"
    if [[ ! -f "$TMPL" ]]; then
      m_warn "Template not found: ${TMPL} — skipping ${SVC}"
      continue
    fi
    SVC_FILE="/etc/systemd/system/${SVC}.service"
    # Stop old (Python/bash) instance if running
    if systemctl is-active --quiet "$SVC" 2>/dev/null; then
      sudo systemctl stop "$SVC" && m_info "Stopped old ${SVC}"
    fi
    # Install updated service file (ExecStart now points to acc-agent)
    sed "s|AGENT_USER|${USER}|g; s|AGENT_HOME|${HOME}|g" "$TMPL" \
      | sudo tee "$SVC_FILE" > /dev/null
    sudo systemctl daemon-reload
    sudo systemctl enable "$SVC"
    sudo systemctl restart "$SVC"
    if systemctl is-active --quiet "$SVC"; then
      m_success "${SVC} started (acc-agent Rust binary)"
    else
      m_warn "${SVC} failed to start — check: journalctl -u ${SVC} -n 30"
    fi
  done
fi

# ── Step 6: macOS (launchd) ───────────────────────────────────────────────────
if on_platform macos; then
  m_info "macOS: acc-agent daemons run as launchd agents (or directly)"
  # Minimal helper: run acc-agent bus in foreground for testing
  m_info "Test with: ${BINARY} bus --test"
fi

# ── Step 7: Smoke test ────────────────────────────────────────────────────────
m_info "Smoke-testing acc-agent..."
if "$BINARY" migrate list "${WORKSPACE}/deploy/migrations" &>/dev/null; then
  m_success "acc-agent migrate list: OK"
else
  m_warn "acc-agent migrate list returned non-zero"
fi

m_success "Migration 0018 complete"
m_info "acc-agent subcommands: bus  queue  hermes  proxy  migrate  agent  json"
m_info "Binary: ${BINARY}"
m_info "Old scripts kept for reference in deploy/ — they are no longer run by systemd"
