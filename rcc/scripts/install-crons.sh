#!/usr/bin/env bash
# rcc/scripts/install-crons.sh — Register OpenClaw cron jobs
set -euo pipefail

echo "[install-crons] Registering memory-compact cron..."
openclaw cron add \
  --name memory-compact \
  --cron "0 3 * * *" \
  --tz "America/Los_Angeles" \
  --message "Run the nightly memory compaction script: node rcc/scripts/memory-compact.mjs" \
  --session isolated \
  --description "Nightly memory compaction (COHERENCE-001)"

echo "[install-crons] Done."
