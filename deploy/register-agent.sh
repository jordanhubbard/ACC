#!/bin/bash
# register-agent.sh — Register this node with CCC
# Run after setup-node.sh and filling in .env

set -e

CCC_DIR="$HOME/.ccc"
ENV_FILE="$CCC_DIR/.env"

if [ -f "$ENV_FILE" ]; then
  set -a; source "$ENV_FILE"; set +a
fi

if [ -z "$CCC_URL" ] || [ -z "$AGENT_NAME" ]; then
  echo "ERROR: Set CCC_URL and AGENT_NAME in $ENV_FILE first"
  exit 1
fi

echo "Registering agent '$AGENT_NAME' with $CCC_URL..."

# Prompt for admin token if not set
if [ -z "$CCC_ADMIN_TOKEN" ]; then
  read -rsp "CCC admin token: " CCC_ADMIN_TOKEN; echo
fi

RESPONSE=$(curl -s -X POST "$CCC_URL/api/agents/register" \
  -H "Authorization: Bearer $CCC_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"name\":\"$AGENT_NAME\",
    \"host\":\"${AGENT_HOST:-$(hostname)}\",
    \"type\":\"${AGENT_TYPE:-full}\",
    \"capabilities\":{
      \"claude_cli\":${AGENT_CLAUDE_CLI:-false},
      \"claude_cli_model\":\"${AGENT_CLAUDE_MODEL:-claude-sonnet-4-6}\",
      \"inference_key\":true,
      \"gpu\":${AGENT_HAS_GPU:-false},
      \"gpu_model\":\"${AGENT_GPU_MODEL:-}\",
      \"gpu_count\":${AGENT_GPU_COUNT:-0},
      \"gpu_vram_gb\":${AGENT_GPU_VRAM_GB:-0},
      \\\"vllm\\\":${VLLM_ENABLED:-false},
      \\\"vllm_model\\\":\\\"${VLLM_MODEL:-}\\\",
      \\\"vllm_served_name\\\":\\\"${VLLM_SERVED_NAME:-}\\\",
      \\\"vllm_port\\\":${VLLM_PORT:-8000}
    },
    \"billing\":{
      \"claude_cli\":\"fixed\",
      \"inference_key\":\"metered\",
      \"gpu\":\"fixed\"
    }
  }")

CCC_AGENT="${CCC_AGENT:-$HOME/.ccc/bin/ccc-agent}"
[ ! -x "$CCC_AGENT" ] && CCC_AGENT="$(command -v ccc-agent 2>/dev/null || echo "")"

TOKEN=$(echo "$RESPONSE" | "$CCC_AGENT" json get .token 2>/dev/null || echo "")

if [ -n "$TOKEN" ]; then
  # Update .env with the issued token
  if grep -q "^CCC_AGENT_TOKEN=" "$ENV_FILE"; then
    sed -i "s|^CCC_AGENT_TOKEN=.*|CCC_AGENT_TOKEN=$TOKEN|" "$ENV_FILE"
  else
    echo "CCC_AGENT_TOKEN=$TOKEN" >> "$ENV_FILE"
  fi
  echo "✓ Registered! Agent token saved to $ENV_FILE"
  echo "  Token: $TOKEN"
else
  echo "Registration response: $RESPONSE"
  echo "ERROR: No token in response. Check CCC_URL and admin token."
  exit 1
fi
