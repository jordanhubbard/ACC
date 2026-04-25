# Description: Configure OS DNS resolver to forward *.consul queries to Consul
#
# Context: Consul runs a DNS server on port 8600. Without OS-level forwarding,
#   .consul names only resolve when you pass --dns-port 8600 explicitly.
#   This migration sets up split-horizon DNS so that *.consul resolves
#   transparently while all other DNS traffic goes to the normal resolver.
#
#   Linux:  /etc/systemd/resolved.conf.d/consul.conf  (Domains=~consul)
#   macOS:  /etc/resolver/consul                      (nameserver/port)
#
# Condition: all platforms

if on_platform linux; then
    # Require systemd-resolved (Ubuntu 18.04+, Debian 10+)
    if ! command -v resolvectl >/dev/null 2>&1 && ! systemctl is-active --quiet systemd-resolved 2>/dev/null; then
        m_warn "systemd-resolved not active — skipping Linux DNS config"
        m_warn "Manual alternative: add 'server=/consul/127.0.0.1#8600' to /etc/dnsmasq.conf"
        return 0
    fi

    sudo mkdir -p /etc/systemd/resolved.conf.d
    sudo tee /etc/systemd/resolved.conf.d/consul.conf > /dev/null << 'EOF'
# CCC: forward *.consul queries to the local Consul DNS server (port 8600)
# The ~ prefix makes this a split-horizon rule (only .consul is affected).
[Resolve]
DNS=127.0.0.1:8600
Domains=~consul
EOF
    sudo systemctl restart systemd-resolved \
        && m_success "systemd-resolved restarted with Consul split-horizon DNS" \
        || m_warn "Failed to restart systemd-resolved"

    # Verify
    if command -v resolvectl >/dev/null 2>&1; then
        m_info "DNS routing: $(resolvectl domain 2>/dev/null | grep consul || echo 'run: resolvectl domain')"
    fi
fi

if on_platform macos; then
    if sudo -n mkdir -p /etc/resolver 2>/dev/null && \
       printf '# CCC: resolve *.consul via Consul DNS\nnameserver 127.0.0.1\nport 8600\n' \
           | sudo -n tee /etc/resolver/consul > /dev/null 2>&1; then
        m_success "/etc/resolver/consul configured for .consul DNS"
        m_info "Verify with: scutil --dns | grep consul"
    else
        m_warn "sudo not available non-interactively — configure manually:"
        m_warn "  sudo mkdir -p /etc/resolver"
        m_warn "  echo 'nameserver 127.0.0.1\nport 8600' | sudo tee /etc/resolver/consul"
        # Not a fatal error — Consul DNS works via port 8600 directly
    fi
fi

m_info "Test after Consul is running: dig ccc-server.service.consul @127.0.0.1 -p 8600"
