#!/usr/bin/env bash
# run-coding-agent.sh — Run a coding task with automatic fallback
#
# Usage: run-coding-agent.sh --repo <path> --prompt <text> [--model <model>]
#
# Tries Claude Code first. On throttle/credit exhaustion, falls back to
# opencode pointed at the local ollama OpenAI-compatible endpoint.
#
# Exit codes: 0=success, 1=both backends failed, 2=usage error

set -euo pipefail

REPO=""
PROMPT=""
MODEL="${OPENCODE_MODEL:-qwen2.5-coder:32b}"
OLLAMA_BASE="${OLLAMA_BASE_URL:-http://localhost:11434}"
BORIS_BASE="${BORIS_BASE_URL:-http://127.0.0.1:18080}"
LOG_FILE="${CODING_AGENT_LOG:-/tmp/coding-agent.log}"
MAX_WAIT=120  # seconds to wait for claude output before timeout check

usage() {
  echo "Usage: $0 --repo <path> --prompt <text> [--model <model>]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)   REPO="$2";   shift 2 ;;
    --prompt) PROMPT="$2"; shift 2 ;;
    --model)  MODEL="$2";  shift 2 ;;
    *) usage ;;
  esac
done

[[ -z "$REPO" || -z "$PROMPT" ]] && usage

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*" | tee -a "$LOG_FILE"; }

# Detect claude throttle/exhaustion signals in output
is_throttled() {
  local output="$1"
  echo "$output" | grep -qiE \
    "429|rate.?limit|too many requests|credit.?balance|token.?exhaust|quota.?exceed|billing|overload"
}

# ── Backend 1: Claude Code ──────────────────────────────────────────────────
run_claude() {
  local out
  log "Trying Claude Code in $REPO"
  out=$(cd "$REPO" && timeout 300 claude --print --permission-mode bypassPermissions "$PROMPT" 2>&1) || {
    local exit_code=$?
    if is_throttled "$out"; then
      log "Claude throttled/exhausted (exit $exit_code) — will fallback to opencode"
      return 1
    fi
    log "Claude failed (exit $exit_code, non-throttle)"
    echo "$out"
    return 1
  }
  echo "$out"
  return 0
}

# ── Backend 2: opencode via ollama (local) ───────────────────────────────────
run_opencode_ollama() {
  log "Trying opencode → ollama ($MODEL) in $REPO"
  # opencode run uses OPENAI_BASE_URL for provider config
  cd "$REPO" && OPENAI_BASE_URL="$OLLAMA_BASE/v1" OPENAI_API_KEY="ollama" \
    timeout 300 opencode run \
      --model "openai/$MODEL" \
      --print \
      "$PROMPT" 2>&1
}

# ── Backend 3: opencode via Boris vLLM ───────────────────────────────────────
run_opencode_boris() {
  # Check if Boris tunnel is up
  if ! curl -sf --max-time 3 "$BORIS_BASE/v1/models" > /dev/null 2>&1; then
    log "Boris tunnel not reachable at $BORIS_BASE — skipping"
    return 1
  fi
  local boris_model
  boris_model=$(curl -sf --max-time 5 "$BORIS_BASE/v1/models" 2>/dev/null \
    | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['data'][0]['id'])" 2>/dev/null \
    || echo "nvidia/Llama-3.1-Nemotron-Super-49B-v1")
  log "Trying opencode → Boris vLLM ($boris_model) in $REPO"
  cd "$REPO" && OPENAI_BASE_URL="$BORIS_BASE/v1" OPENAI_API_KEY="boris" \
    timeout 300 opencode run \
      --model "openai/$boris_model" \
      --print \
      "$PROMPT" 2>&1
}

# ── Main dispatch ─────────────────────────────────────────────────────────────
RESULT=""
BACKEND_USED=""

# Try Claude first (skip if FORCE_OPENCODE=1)
if [[ "${FORCE_OPENCODE:-0}" != "1" ]] && command -v claude &>/dev/null; then
  if RESULT=$(run_claude); then
    BACKEND_USED="claude"
  else
    log "Claude failed — falling back to opencode"
  fi
fi

# Fallback: opencode → ollama (local qwen2.5-coder)
if [[ -z "$BACKEND_USED" ]]; then
  if RESULT=$(run_opencode_ollama); then
    BACKEND_USED="opencode/ollama/$MODEL"
  else
    log "opencode/ollama failed — trying Boris"
  fi
fi

# Fallback: opencode → Boris vLLM
if [[ -z "$BACKEND_USED" ]]; then
  if RESULT=$(run_opencode_boris); then
    BACKEND_USED="opencode/boris"
  else
    log "All backends failed"
    exit 1
  fi
fi

log "Completed via $BACKEND_USED"
echo "$RESULT"
echo "BACKEND_USED=$BACKEND_USED" >> "$LOG_FILE"
exit 0
