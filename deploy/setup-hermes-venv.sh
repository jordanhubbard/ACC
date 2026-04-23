#!/usr/bin/env bash
# Creates ~/.acc/hermes-venv and installs hermes (editable) into it.
# Safe to re-run — upgrades pip and reinstalls if the venv already exists.
set -euo pipefail

VENV="${HOME}/.acc/hermes-venv"
HERMES_SRC="${HOME}/Src/ACC/hermes"

if [ ! -d "${HERMES_SRC}" ]; then
    echo "[hermes-venv] ERROR: hermes source not found at ${HERMES_SRC}" >&2
    exit 1
fi

echo "[hermes-venv] Creating venv at ${VENV}"
python3 -m venv "${VENV}"

echo "[hermes-venv] Upgrading pip"
"${VENV}/bin/pip" install --quiet --upgrade pip

echo "[hermes-venv] Installing hermes from ${HERMES_SRC}"
"${VENV}/bin/pip" install --quiet -e "${HERMES_SRC}"

echo "[hermes-venv] Installing hermes wrapper at ~/.local/bin/hermes"
mkdir -p "${HOME}/.local/bin"
cat > "${HOME}/.local/bin/hermes" << 'WRAPPER'
#!/usr/bin/env bash
VENV="${HOME}/.acc/hermes-venv"
HERMES_SRC="${HOME}/Src/ACC/hermes"
if [ ! -x "${VENV}/bin/hermes" ]; then
    echo "[hermes] venv missing — run: bash ~/Src/ACC/deploy/setup-hermes-venv.sh" >&2
    exit 1
fi
exec "${VENV}/bin/hermes" "$@"
WRAPPER
chmod +x "${HOME}/.local/bin/hermes"

echo "[hermes-venv] Done. Run 'hermes --version' to verify."
