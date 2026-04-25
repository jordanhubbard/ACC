#!/usr/bin/env bash
# setup-dns-forwarding.sh — Configure systemd-resolved to forward .consul queries
# Works on Linux hosts with systemd-resolved (do-host1, sparky, boris)
set -euo pipefail

echo "=== Setting up .consul DNS forwarding ==="

if ! systemctl is-active systemd-resolved &>/dev/null; then
    echo "systemd-resolved not active. Skipping DNS forwarding."
    echo "For manual setup, add to /etc/resolv.conf:"
    echo "  nameserver 127.0.0.1  # port 8600 via dnsmasq/unbound"
    exit 0
fi

# Create a drop-in config for resolved to forward .consul to Consul's DNS
sudo mkdir -p /etc/systemd/resolved.conf.d

cat <<'EOF' | sudo tee /etc/systemd/resolved.conf.d/consul.conf >/dev/null
# Forward .consul domain queries to Consul's DNS listener
[Resolve]
DNS=127.0.0.1:8600
Domains=~consul
EOF

echo "→ Created /etc/systemd/resolved.conf.d/consul.conf"
sudo systemctl restart systemd-resolved
echo "→ Restarted systemd-resolved"

echo ""
echo "Testing: resolvectl query tokenhub.service.consul"
resolvectl query tokenhub.service.consul 2>/dev/null || echo "(will work once Consul is running)"

echo ""
echo "=== DNS forwarding configured ==="
echo "Services now resolve as: <service>.service.consul"
