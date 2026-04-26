#!/usr/bin/env bash
# restart-agent.sh — rebuild acc-agent on THIS node and restart it.
#
# Run on an agent node. The acc-agent supervisor relaunches the binary
# automatically after `pkill -x acc-agent`, so no daemon-manager needed.
set -euo pipefail

ACC_DIR="${ACC_DIR:-$HOME/.acc}"
LOG_DIR="${LOG_DIR:-$ACC_DIR/logs}"
WORKSPACE="${WORKSPACE:-$(git rev-parse --show-toplevel 2>/dev/null || true)}"

# Non-interactive SSH sessions don't source ~/.bashrc, so rustup-installed
# cargo isn't on PATH. Pull it in explicitly if it's there.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if [ -z "$WORKSPACE" ] || [ ! -f "${WORKSPACE}/Cargo.toml" ]; then
    echo "[restart-agent] ERROR: cannot locate workspace (run from inside the repo, or set WORKSPACE=)" >&2
    exit 1
fi

AGENT_BIN="${WORKSPACE}/target/release/acc-agent"
AGENT_DEST="${ACC_DIR}/bin/acc-agent"
mkdir -p "$LOG_DIR" "${ACC_DIR}/bin"

echo "[restart-agent] Building acc-agent (release) from $WORKSPACE"
cargo build --release --manifest-path "${WORKSPACE}/Cargo.toml" -p acc-agent

if [ ! -x "$AGENT_BIN" ]; then
    echo "[restart-agent] ERROR: build produced no binary at $AGENT_BIN" >&2
    exit 1
fi

echo "[restart-agent] Installing → $AGENT_DEST"
tmp="${AGENT_DEST}.new.$$"
cp "$AGENT_BIN" "$tmp"
chmod +x "$tmp"
mv "$tmp" "$AGENT_DEST"

echo "[restart-agent] Stopping running acc-agent (supervisor will relaunch)"
pkill -x acc-agent 2>/dev/null || true
sleep 2

if pgrep -x acc-agent >/dev/null; then
    PID=$(pgrep -x acc-agent | head -1)
    echo "[restart-agent] ✓ acc-agent running (pid=$PID)"
else
    echo "[restart-agent] ⚠ acc-agent not running yet — supervisor may take a few seconds to relaunch"
fi
