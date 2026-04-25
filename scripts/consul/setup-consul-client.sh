#!/usr/bin/env bash
# setup-consul-client.sh — Install Consul client agent on a fleet node
# Usage: scripts/consul/setup-consul-client.sh [node-name]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CCC_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
NODE_NAME="${1:-$(hostname)}"
CONSUL_VERSION="1.18.2"
CONSUL_DIR="$CCC_ROOT/deploy/consul"

echo "=== CCC Consul Client Setup ==="
echo "Node: $NODE_NAME"
echo "Tailscale IP: $(tailscale ip -4 2>/dev/null || echo 'unknown')"

# Detect OS/arch
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *) echo "ERROR: Unsupported arch $ARCH"; exit 1 ;;
esac

# Check if consul is already installed
if command -v consul &>/dev/null; then
    echo "Consul already installed: $(consul version | head -1)"
else
    echo "→ Installing Consul ${CONSUL_VERSION} (${OS}/${ARCH})..."
    TMPDIR=$(mktemp -d)
    curl -fsSL "https://releases.hashicorp.com/consul/${CONSUL_VERSION}/consul_${CONSUL_VERSION}_${OS}_${ARCH}.zip" -o "$TMPDIR/consul.zip"
    cd "$TMPDIR" && unzip -q consul.zip
    sudo mv consul /usr/local/bin/consul
    sudo chmod +x /usr/local/bin/consul
    rm -rf "$TMPDIR"
    echo "  Installed: $(consul version | head -1)"
fi

# Create data dir
sudo mkdir -p /opt/consul/data
sudo chown "$(whoami)" /opt/consul/data

# Create config dir
sudo mkdir -p /etc/consul.d
sudo cp "$CONSUL_DIR/client/consul-client.hcl" /etc/consul.d/consul.hcl

# Set node name
echo "node_name = \"$NODE_NAME\"" | sudo tee /etc/consul.d/node-name.hcl >/dev/null

# Copy service definitions for this node
SERVICE_DEF="$CONSUL_DIR/service-defs/${NODE_NAME}.hcl"
if [ -f "$SERVICE_DEF" ]; then
    sudo cp "$SERVICE_DEF" /etc/consul.d/services.hcl
    echo "→ Loaded service definitions from ${NODE_NAME}.hcl"
else
    echo "→ No service definitions found for $NODE_NAME (expected $SERVICE_DEF)"
fi

# Create systemd service
cat <<'EOF' | sudo tee /etc/systemd/system/consul.service >/dev/null
[Unit]
Description=Consul Agent (CCC Fleet)
Documentation=https://www.consul.io/
Requires=network-online.target
After=network-online.target tailscaled.service

[Service]
Type=notify
User=root
ExecStart=/usr/local/bin/consul agent -config-dir=/etc/consul.d
ExecReload=/bin/kill --signal HUP $MAINPID
KillMode=process
KillSignal=SIGTERM
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

echo "→ Enabling and starting Consul agent..."
sudo systemctl daemon-reload
sudo systemctl enable consul
sudo systemctl start consul

echo ""
echo "→ Checking membership..."
sleep 2
consul members 2>/dev/null || echo "(may take a moment to join)"

echo ""
echo "=== Setup complete ==="
echo "Check: consul members"
echo "Services: consul catalog services"
echo "DNS: dig @127.0.0.1 -p 8600 tokenhub.service.consul SRV"
