# Consul Service Discovery — CCC Fleet

Replaces hardcoded IPs/ports with DNS-based service discovery across the fleet.

## Fleet Topology (Tailscale)

| Host      | Tailscale IP     | Role           | Location        |
|-----------|------------------|----------------|-----------------|
| do-host1  | 100.89.199.14    | Consul server  | DigitalOcean    |
| sparky    | 100.87.229.125   | Consul client  | DGX Spark local |
| puck      | 100.87.68.11     | Consul client  | Mac local       |
| boris     | (Sweden fleet)   | Consul client  | NVIDIA DGX-C    |

## Services Registry

### do-host1 (100.89.199.14)
| Service          | Port  | Protocol | Health Check            |
|------------------|-------|----------|-------------------------|
| rcc-hub          | 8789  | HTTP     | GET /health             |
| clawbus          | 8789  | HTTP     | GET /api/bus/stream     |
| tokenhub         | 8090  | HTTP     | GET /health             |
| squirrelchat     | 8793  | HTTP     | GET /                   |
| qdrant           | 6333  | HTTP     | GET /collections        |
| milvus           | 19530 | gRPC     | TCP connect             |
| minio            | 9000  | HTTP     | GET /minio/health/live  |
| searxng          | 8888  | HTTP     | GET /                   |
| clawchat         | 8790  | HTTP     | GET /                   |

### sparky (100.87.229.125)
| Service          | Port  | Protocol | Health Check            |
|------------------|-------|----------|-------------------------|
| whisper-api      | 8792  | HTTP     | GET /health             |
| clawfs           | 8791  | HTTP     | GET /health             |
| usdagent         | 8000  | HTTP     | GET /health             |
| ollama           | 11434 | HTTP     | GET /                   |

### boris (Sweden fleet)
| Service          | Port  | Protocol | Health Check            |
|------------------|-------|----------|-------------------------|
| boris-vllm       | 18080 | HTTP     | GET /v1/models          |

## Quick Start

### 1. Deploy Consul Server (do-host1)
```bash
# From CCC root on do-host1:
scripts/consul/setup-consul-server.sh
# This runs docker compose and waits for leader election
```

### 2. Deploy Client Agents (sparky, puck, boris, etc.)
```bash
# SSH to a fleet node, copy CCC, then:
scripts/consul/setup-consul-client.sh sparky
# Installs binary, copies configs + service defs, starts systemd unit
```

### 3. Set Up DNS Forwarding
```bash
scripts/consul/setup-dns-forwarding.sh
# Configures systemd-resolved to forward .consul queries
```

## CCC Scripts

| Script                           | Purpose                                |
|----------------------------------|----------------------------------------|
| `scripts/consul/setup-consul-server.sh`  | Deploy Consul server on do-host1       |
| `scripts/consul/setup-consul-client.sh`  | Deploy Consul client on fleet node     |
| `scripts/consul/setup-dns-forwarding.sh` | Configure .consul DNS via resolved     |
| `scripts/consul/consul-lookup.sh`        | Look up a service (address/url/json)   |
| `scripts/consul/consul-services.sh`      | List all services with health status   |

### Lookup Examples
```bash
# Get service address
scripts/consul/consul-lookup.sh tokenhub          # → 100.89.199.14:8090
scripts/consul/consul-lookup.sh tokenhub url       # → http://100.89.199.14:8090
scripts/consul/consul-lookup.sh whisper-api all    # → full details + metadata

# List everything
scripts/consul/consul-services.sh                 # all services
scripts/consul/consul-services.sh --healthy        # only passing checks

# Raw DNS
dig @127.0.0.1 -p 8600 tokenhub.service.consul SRV
```

## DNS Integration

Once DNS forwarding is set up, services resolve natively:
- `tokenhub.service.consul` → 100.89.199.14:8090
- `qdrant.service.consul` → 100.89.199.14:6333
- `whisper-api.service.consul` → 100.87.229.125:8792
- `boris-vllm.service.consul` → boris:18080

## Config Files

```
deploy/consul/
├── README.md                    # This file
├── server/
│   ├── consul-server.hcl       # Server config (datacenter=ccc, bootstrap)
│   └── docker-compose.yml      # Docker Compose for server
├── client/
│   └── consul-client.hcl       # Client template (retry_join do-host1)
└── service-defs/
    ├── do-host1.hcl            # 9 services (rcc-hub, clawbus, tokenhub, etc.)
    ├── sparky.hcl              # 4 services (whisper, clawfs, usdagent, ollama)
    └── boris.hcl               # 1 service (boris-vllm)
```

## Adding a New Service

1. Add a `services {}` block to the appropriate `service-defs/<host>.hcl`
2. Reload Consul: `consul reload` (client) or restart the container (server)
3. Verify: `scripts/consul/consul-lookup.sh <service-name>`
