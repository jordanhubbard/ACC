#!/usr/bin/env bash
# 0024_replace_fleet_monitor_wrapper.sh
#
# Replaces the acc-fleet-monitor.py subprocess wrapper in ~/.hermes/scripts/
# with a direct copy of the canonical ACC script (scripts/cron-fleet-monitor.py)
# and updates the cron job's script: field in jobs.json to reference it directly.
#
# Background: the wrapper was created to bridge ~/.hermes/scripts/ to
# ~/Src/ACC/scripts/cron-fleet-monitor.py via subprocess.  This adds an
# unnecessary layer of indirection — if the ACC path changes the wrapper
# silently breaks.  The correct approach is to copy the canonical script
# into ~/.hermes/scripts/ under its real name and update jobs.json to point
# to it directly.  agent-pull.sh is also updated to keep this copy in sync
# on every workspace pull.
set -euo pipefail

ACC_DIR="${HOME}/.acc"
WORKSPACE="${ACC_DIR}/workspace"
HERMES_SCRIPTS="${HOME}/.hermes/scripts"
JOBS_FILE="${HOME}/.hermes/cron/jobs.json"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
ok()   { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

if [[ ! -d "$WORKSPACE/.git" ]]; then
    warn "Workspace not found at $WORKSPACE — run setup-node.sh first"
    exit 1
fi

SRC="${WORKSPACE}/scripts/cron-fleet-monitor.py"
if [[ ! -f "$SRC" ]]; then
    warn "Canonical script not found at $SRC — pull latest workspace and retry"
    exit 1
fi

mkdir -p "$HERMES_SCRIPTS"

# ── 1. Copy the canonical script directly into ~/.hermes/scripts/ ────────────
cp "$SRC" "${HERMES_SCRIPTS}/cron-fleet-monitor.py"
chmod 755 "${HERMES_SCRIPTS}/cron-fleet-monitor.py"
ok "Copied scripts/cron-fleet-monitor.py → ${HERMES_SCRIPTS}/cron-fleet-monitor.py"

# ── 2. Remove the old subprocess wrapper (if present) ────────────────────────
WRAPPER="${HERMES_SCRIPTS}/acc-fleet-monitor.py"
if [[ -f "$WRAPPER" ]]; then
    rm -f "$WRAPPER"
    ok "Removed stale wrapper: $WRAPPER"
else
    ok "Wrapper not present — nothing to remove"
fi

# ── 3. Patch jobs.json: update script: field for acc-fleet-monitor job ────────
if [[ ! -f "$JOBS_FILE" ]]; then
    warn "jobs.json not found at $JOBS_FILE — skipping jobs.json patch"
else
    # Use Python for safe, idempotent JSON surgery
    python3 - "$JOBS_FILE" << 'PYEOF'
import json, sys

jobs_file = sys.argv[1]

with open(jobs_file) as f:
    data = json.load(f)

changed = False
for job in data.get("jobs", []):
    if job.get("script") == "acc-fleet-monitor.py":
        job["script"] = "cron-fleet-monitor.py"
        changed = True

if changed:
    with open(jobs_file, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    print("jobs.json updated: acc-fleet-monitor.py → cron-fleet-monitor.py")
else:
    print("jobs.json already up to date (no changes needed)")
PYEOF
    ok "jobs.json patched"
fi

ok "Migration 0024 complete — fleet monitor cron job now references cron-fleet-monitor.py directly"
