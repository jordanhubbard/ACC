#!/usr/bin/env bash
# setup-consul-server.sh — Install Consul server on do-host1
# Run from the CCC root: scripts/consul/setup-consul-server.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CCC_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CONSUL_DIR="$CCC_ROOT/deploy/consul"

echo "=== CCC Consul Server Setup ==="
echo "Host: $(hostname)"
echo "Tailscale IP: $(tailscale ip -4 2>/dev/null || echo 'unknown')"

# Verify we're on do-host1
TS_IP=$(tailscale ip -4 2>/dev/null || true)
if [ "$TS_IP" != "100.89.199.14" ]; then
    echo "WARNING: Expected do-host1 (100.89.199.14), got $TS_IP"
    echo "Continue anyway? (y/N)"
    read -r ans
    [ "$ans" = "y" ] || exit 1
fi

# Check Docker
if ! command -v docker &>/dev/null; then
    echo "ERROR: Docker not found. Install Docker first."
    exit 1
fi

echo ""
echo "→ Starting Consul server via docker compose..."
cd "$CONSUL_DIR/server"
docker compose up -d

echo ""
echo "→ Waiting for Consul to be ready..."
for i in $(seq 1 30); do
    if curl -sf http://127.0.0.1:8500/v1/status/leader &>/dev/null; then
        echo "  Consul is ready! Leader: $(curl -s http://127.0.0.1:8500/v1/status/leader)"
        break
    fi
    sleep 1
done

echo ""
echo "→ Registered services:"
curl -s http://127.0.0.1:8500/v1/catalog/services | python3 -m json.tool 2>/dev/null || true

echo ""
echo "=== Setup complete ==="
echo "UI:  http://100.89.199.14:8500/ui/"
echo "DNS: dig @127.0.0.1 -p 8600 tokenhub.service.consul SRV"
echo ""
echo "Next: Configure DNS forwarding for .consul domain"
echo "  sudo systemctl edit systemd-resolved"
echo "  [Resolve]"
echo "  DNS=127.0.0.1:8600"
echo "  Domains=~consul"
