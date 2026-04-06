# Consul server config for do-host1
# Binds to Tailscale IP so all fleet nodes can reach it.

datacenter = "ccc"
node_name  = "do-host1"
server     = true

# Single-server bootstrap (no HA cluster needed for our fleet)
bootstrap_expect = 1

# Bind to Tailscale interface for fleet visibility
bind_addr   = "{{ GetInterfaceIP \"tailscale0\" }}"
client_addr = "0.0.0.0"

# Data persistence
data_dir = "/consul/data"

# DNS
dns_config {
  allow_stale = true
}
ports {
  dns  = 8600
  http = 8500
  grpc = 8502
}

# UI for debugging
ui_config {
  enabled = true
}

# Enable local script checks for health monitoring
enable_local_script_checks = true

# Logging
log_level = "INFO"

# Disable remote exec for security
disable_remote_exec = true

# Telemetry (optional, for future Prometheus scraping)
telemetry {
  disable_hostname = true
  prometheus_retention_time = "24h"
}
