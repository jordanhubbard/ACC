#!/usr/bin/env bash
# fleet-sync.sh — Push current workspace state to all agents via AgentBus.
#
# Run this after committing and pushing changes to GitHub:
#
#   git push && bash deploy/fleet-sync.sh --acc=http://<hub>:8789 --token=<token>
#
# If ~/.acc/.env exists with ACC_URL and ACC_AGENT_TOKEN set, flags are optional.
#
# What it does:
#   1. Broadcasts acc.update to AgentBus so all agents run agent-pull.sh NOW
#      instead of waiting up to 10 minutes for the scheduled timer
#   2. Reports online agents from /bus/presence
#
# Flags:
#   --acc=URL       Hub URL (overrides ACC_URL from .env)
#   --token=TOKEN   Agent token (overrides ACC_AGENT_TOKEN from .env)
#   --dry-run       Show what would happen without doing it
#   --skip-mirror   Skip the MinIO mirror step (just send the bus message)
#   --branch=NAME   Specify branch (default: current git branch)
#   --component=X   Component name in acc.update body (default: workspace)

set -euo pipefail

# ── Parse args ─────────────────────────────────────────────────────────────────
DRY_RUN=false
BRANCH=""
COMPONENT="workspace"
ACC_URL_ARG=""
ACC_TOKEN_ARG=""

for arg in "$@"; do
  case "$arg" in
    --acc=*)          ACC_URL_ARG="${arg#--acc=}" ;;
    --ccc=*)          ACC_URL_ARG="${arg#--ccc=}" ;;  # backward compat alias
    --token=*)        ACC_TOKEN_ARG="${arg#--token=}" ;;
    --dry-run)        DRY_RUN=true ;;
    --branch=*)       BRANCH="${arg#--branch=}" ;;
    --component=*)    COMPONENT="${arg#--component=}" ;;
    -h|--help)
      echo "Usage: fleet-sync.sh [--acc=URL] [--token=TOKEN] [--dry-run] [--branch=NAME] [--component=NAME]"
      exit 0 ;;
  esac
done

# ── Load env ───────────────────────────────────────────────────────────────────
# Prefer ~/.acc (post-migration), fall back to ~/.ccc (pre-migration)
if [[ -d "${HOME}/.acc" ]]; then
  ACC_DIR="${HOME}/.acc"
else
  ACC_DIR="${HOME}/.ccc"
fi
ENV_FILE="${ACC_DIR}/.env"
WORKSPACE="${ACC_DIR}/workspace"

# Also try loading from the workspace's own .env-style files
for _env in "$ENV_FILE" ".env" "${WORKSPACE}/.env"; do
  [[ -f "$_env" ]] && { set -a; source "$_env"; set +a; } || true
done

# ACC_URL preferred; fall back to CCC_URL for pre-migration nodes
ACC_URL="${ACC_URL:-${CCC_URL:-}}"
ACC_AGENT_TOKEN="${ACC_AGENT_TOKEN:-${CCC_AGENT_TOKEN:-}}"
AGENT_NAME="${AGENT_NAME:-jkh}"

# CLI flags override .env
[[ -n "$ACC_URL_ARG"   ]] && ACC_URL="$ACC_URL_ARG"
[[ -n "$ACC_TOKEN_ARG" ]] && ACC_AGENT_TOKEN="$ACC_TOKEN_ARG"

if [[ -z "$ACC_URL" ]]; then
  echo "ERROR: ACC_URL not set. Pass --acc=<url> or add ACC_URL to ~/.acc/.env" >&2
  exit 1
fi

ACC_URL="${ACC_URL%/}"

# Resolve git repo — prefer the directory containing this script, then WORKSPACE
SCRIPT_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." 2>/dev/null && pwd || echo "")"
for _candidate in "$SCRIPT_REPO" "$WORKSPACE" "$(pwd)"; do
  if [[ -d "${_candidate}/.git" ]]; then
    GIT_REPO="$_candidate"
    break
  fi
done
GIT_REPO="${GIT_REPO:-$WORKSPACE}"

# Resolve branch
if [[ -z "$BRANCH" ]]; then
  BRANCH=$(git -C "${GIT_REPO}" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "main")
fi

REV=$(git -C "${GIT_REPO}" rev-parse --short HEAD 2>/dev/null || echo "unknown")

# ── Colors ─────────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; BLUE='\033[0;34m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info()    { echo -e "${BLUE}→${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
warn()    { echo -e "${YELLOW}⚠${NC} $1"; }
dry()     { echo -e "${YELLOW}[DRY RUN]${NC} $1"; }

echo ""
echo "🔄 ACC Fleet Sync  (rev=${REV} branch=${BRANCH})"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# ── Step 1: Show online agents from AgentBus presence ─────────────────────────
info "Checking agent presence..."
PRESENCE_JSON=$(curl -sf --max-time 10 \
  -H "Authorization: Bearer ${ACC_AGENT_TOKEN}" \
  "${ACC_URL}/bus/presence" 2>/dev/null || echo "")
if [[ -n "$PRESENCE_JSON" ]]; then
  ONLINE_AGENTS=$(python3 -c "
import json, sys
try:
    data = json.loads(sys.argv[1])
    agents = list(data.keys()) if isinstance(data, dict) else []
    print(', '.join(agents) if agents else '(none)')
except Exception:
    print('(could not parse)')
" "$PRESENCE_JSON" 2>/dev/null || echo "(unknown)")
  info "Online agents: ${ONLINE_AGENTS}"
else
  warn "Could not reach /bus/presence — hub may be down or unreachable"
fi

# ── Step 2: Broadcast acc.update ──────────────────────────────────────────────
info "Broadcasting acc.update to all agents..."

UPDATE_BODY=$(python3 -c "
import json
print(json.dumps({'component': '${COMPONENT}', 'branch': '${BRANCH}', 'rev': '${REV}'}))
" 2>/dev/null || echo "{\"component\":\"${COMPONENT}\",\"branch\":\"${BRANCH}\"}")

MSG_JSON=$(python3 -c "
import json
print(json.dumps({
  'from': '${AGENT_NAME}',
  'to': 'all',
  'type': 'acc.update',
  'subject': 'workspace sync ${REV}',
  'body': '${UPDATE_BODY}'
}))
" 2>/dev/null)

if [[ "$DRY_RUN" == true ]]; then
  dry "Would POST to ${ACC_URL}/bus/send:"
  dry "  $MSG_JSON"
else
  if [[ -z "$ACC_AGENT_TOKEN" ]]; then
    warn "ACC_AGENT_TOKEN not set — bus message will fail if hub requires auth"
  fi
  RESP=$(curl -sf --max-time 15 \
    -X POST "${ACC_URL}/bus/send" \
    -H "Authorization: Bearer ${ACC_AGENT_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$MSG_JSON" 2>&1) || RESP=""

  if echo "$RESP" | grep -q '"ok":true'; then
    SEQ=$(python3 -c "import json,sys; print(json.loads(sys.argv[1]).get('message',{}).get('seq','?'))" "$RESP" 2>/dev/null || echo "?")
    success "acc.update broadcast sent (seq=${SEQ})"
  else
    warn "Bus send may have failed. Response: ${RESP:-<empty>}"
    warn "  Agents will still sync on their next 10-minute timer cycle."
  fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}✓ Fleet sync complete${NC}"
echo ""
echo "  Agents subscribed to AgentBus will pull immediately."
echo "  Others will pick it up within 10 minutes via acc-agent-pull timer."
echo ""
echo ""
