#!/bin/bash
# OpenClaw memory sync for container agents
# Sync MEMORY.md + memory/*.md to/from MinIO
# Called: on openclaw session end (push), on session start (pull)

MINIO_ALIAS="${MINIO_ALIAS:-do-host1}"
MINIO_BUCKET="${MINIO_BUCKET:-agents}"
AGENT_NAME="${AGENT_NAME:-$(hostname)}"
WORKSPACE="${OPENCLAW_WORKSPACE:-$HOME/.openclaw/workspace}"
MC="${MC_PATH:-mc}"

push_memory() {
  echo "[memory-sync] Pushing memory to MinIO agents/${AGENT_NAME}/"
  "$MC" cp "${WORKSPACE}/MEMORY.md" "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/MEMORY.md" 2>/dev/null
  if [ -d "${WORKSPACE}/memory" ]; then
    "$MC" cp --recursive "${WORKSPACE}/memory/" "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/memory/" 2>/dev/null
  fi
  echo "[memory-sync] Push complete"
}

pull_memory() {
  local local_ts remote_ts
  echo "[memory-sync] Checking remote MEMORY.md for ${AGENT_NAME}..."
  # Pull if local is older than 1h or doesn't exist
  if [ ! -f "${WORKSPACE}/MEMORY.md" ]; then
    echo "[memory-sync] No local MEMORY.md, pulling from MinIO..."
    mkdir -p "${WORKSPACE}" "${WORKSPACE}/memory"
    "$MC" cp "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/MEMORY.md" "${WORKSPACE}/MEMORY.md" 2>/dev/null
    "$MC" cp --recursive "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/memory/" "${WORKSPACE}/memory/" 2>/dev/null
    echo "[memory-sync] Pull complete"
  else
    local age=$(($(date +%s) - $(stat -c %Y "${WORKSPACE}/MEMORY.md" 2>/dev/null || echo 0)))
    if [ $age -gt 3600 ]; then
      echo "[memory-sync] Local MEMORY.md is ${age}s old, pulling fresh copy..."
      "$MC" cp "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/MEMORY.md" "${WORKSPACE}/MEMORY.md.remote" 2>/dev/null
      # Keep newer version
      if [ -f "${WORKSPACE}/MEMORY.md.remote" ]; then
        mv "${WORKSPACE}/MEMORY.md.remote" "${WORKSPACE}/MEMORY.md"
      fi
      "$MC" cp --recursive "${MINIO_ALIAS}/${MINIO_BUCKET}/${AGENT_NAME}/memory/" "${WORKSPACE}/memory/" 2>/dev/null
      echo "[memory-sync] Pull complete"
    else
      echo "[memory-sync] Local MEMORY.md is fresh (${age}s), skipping pull"
    fi
  fi
}

case "${1:-pull}" in
  push) push_memory ;;
  pull) pull_memory ;;
  *) echo "Usage: $0 [push|pull]"; exit 1 ;;
esac
