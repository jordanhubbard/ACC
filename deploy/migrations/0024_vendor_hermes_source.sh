#!/usr/bin/env bash
# Migration 0024 — vendored hermes source is now at ACC/hermes/
# This migration:
#   1. Symlinks ~/.local/bin/hermes → ACC/hermes/hermes (vendored source)
#   2. Installs ACC-specific plugins into ~/.hermes/plugins/
#   3. Removes any stale reference to the old separate hermes-agent checkout
set -euo pipefail

ACC_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERMES_SRC="${ACC_ROOT}/hermes"
PLUGINS_SRC="${HERMES_SRC}/contrib/plugins"
HERMES_HOME="${HOME}/.hermes"

echo "[0024] Linking hermes from vendored source: ${HERMES_SRC}"
mkdir -p "${HOME}/.local/bin"
ln -sf "${HERMES_SRC}/hermes" "${HOME}/.local/bin/hermes"
echo "[0024]   → ~/.local/bin/hermes -> ${HERMES_SRC}/hermes"

echo "[0024] Installing ACC plugins..."
mkdir -p "${HERMES_HOME}/plugins"
for plugin_dir in "${PLUGINS_SRC}"/*/; do
    plugin_name="$(basename "${plugin_dir}")"
    dst="${HERMES_HOME}/plugins/${plugin_name}"
    rm -rf "${dst}"
    cp -r "${plugin_dir}" "${dst}"
    echo "  ✓ ${plugin_name}"
done

echo "[0024] Done. hermes version: $(~/.local/bin/hermes --version 2>/dev/null || echo 'unknown')"
echo "[0024] Active plugins: $(ls "${HERMES_HOME}/plugins/" 2>/dev/null | tr '\n' ' ')"
