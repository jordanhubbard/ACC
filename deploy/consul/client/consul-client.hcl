# Consul client config template
# Copy to each fleet node, update node_name.
# The server address is the Tailscale IP of do-host1.

datacenter = "ccc"
# node_name = "sparky"  # Set via env or override file

server = false

# Join the Consul server on do-host1 via Tailscale
retry_join = ["100.89.199.14"]

# Bind to Tailscale interface
bind_addr   = "{{ GetInterfaceIP \"tailscale0\" }}"
client_addr = "127.0.0.1"

data_dir = "/opt/consul/data"

# DNS forwarding on loopback
ports {
  dns  = 8600
  http = 8500
}

# Enable local script checks for health monitoring
enable_local_script_checks = true

log_level = "INFO"
