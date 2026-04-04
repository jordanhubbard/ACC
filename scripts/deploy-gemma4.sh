#!/bin/bash
# deploy-gemma4.sh
# Run ON the container (via ClawBus exec or HORDE SSH from puck)
# Downloads Gemma4 from ClawFS MinIO and restarts vLLM with it

set -euo pipefail

MODEL_NAME="gemma-4-31B-it-FP8_BLOCK"
MODEL_DIR="/home/horde/models/$MODEL_NAME"
MINIO_ENDPOINT="https://minio.yourmom.photos"
# Creds injected at deploy time via RCC exec environment
MINIO_ACCESS="rockymoose5808f4a22ccc869a"
MINIO_SECRET="5d3a6a83ef56319242bffe17a2580403f1e63b97dffad270"
MINIO_PATH="agents/models/$MODEL_NAME"
VLLM_BIN="/home/horde/.vllm-venv/bin/vllm"
VLLM_PORT=8080  # Must match tunnel forward port (ssh -R 1808x:localhost:8080)
SUPERVISOR_CONF="/etc/supervisor/conf.d/vllm.conf"

echo "=== Gemma 4 31B FP8 Deployment ==="
echo "Host: $(hostname)"
echo "Started: $(date)"
echo ""

# ── 1. Install mc if missing ──────────────────────────────────
if ! which mc &>/dev/null; then
  echo "Installing mc..."
  curl -fsSL https://dl.min.io/client/mc/release/linux-amd64/mc -o /tmp/mc
  chmod +x /tmp/mc
  sudo mv /tmp/mc /usr/local/bin/mc
fi

# ── 2. Configure mc ───────────────────────────────────────────
mc alias set clawfs "$MINIO_ENDPOINT" "$MINIO_ACCESS" "$MINIO_SECRET" --no-color 2>/dev/null

# ── 3. Download model ─────────────────────────────────────────
mkdir -p "$MODEL_DIR"
echo "Downloading from ClawFS → $MODEL_DIR"
mc mirror --overwrite "clawfs/$MINIO_PATH" "$MODEL_DIR" --no-color

echo ""
echo "Download complete. Size: $(du -sh $MODEL_DIR | cut -f1)"

# ── 4. Update supervisord vllm config ─────────────────────────
echo "Updating vLLM config..."

cat > /tmp/vllm-new.conf << CONF
[program:vllm]
command=${VLLM_BIN} serve ${MODEL_DIR} \
    --host 0.0.0.0 \
    --port ${VLLM_PORT} \
    --tensor-parallel-size 4 \
    --gpu-memory-utilization 0.92 \
    --max-model-len 32768 \
    --served-model-name gemma
directory=/home/horde
user=horde
autostart=true
autorestart=true
stderr_logfile=/var/log/supervisor/vllm.err.log
stdout_logfile=/var/log/supervisor/vllm.out.log
startretries=3
stopasgroup=true
killasgroup=true
CONF

sudo cp /tmp/vllm-new.conf "$SUPERVISOR_CONF"
sudo supervisorctl reread
sudo supervisorctl update
echo "Restarting vLLM..."
sudo supervisorctl restart vllm

# Wait for it to come up
echo "Waiting for vLLM to be ready..."
for i in $(seq 1 60); do
  if curl -s "http://localhost:$VLLM_PORT/health" | grep -q "{}"; then
    echo "✅ vLLM healthy after ${i}x5s = $((i*5))s"
    break
  fi
  echo "  Waiting... ($((i*5))s)"
  sleep 5
done

echo ""
echo "=== Done at $(date) ==="
curl -s "http://localhost:$VLLM_PORT/v1/models" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for m in d.get('data',[]):
    print('  Model:', m.get('id'))
" 2>/dev/null || echo "  (models endpoint not ready yet — model may still be loading)"
