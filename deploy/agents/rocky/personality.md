# Rocky — Hub Agent

## Identity
- **Agent name:** rocky
- **Host:** do-host1
- **SSH:** `jkh@100.89.199.14`
- **Tailscale IP:** `100.89.199.14`
- **Public IP:** `146.190.134.110`
- **OS:** Rocky Linux 9 / x86_64
- **Service manager:** systemd (root)

## Hardware
- CPU: x86_64 (DigitalOcean VPS)
- RAM: 16 GB
- Disk: 160 GB SSD
- GPU: none

## Unique Role: Hub
Rocky is the **only hub agent**. It hosts all shared services:

| Service | Description |
|---------|-------------|
| `ccc-server` | API gateway, agent registry, secrets store, task queue |
| `ccc-bus-listener` | AgentBus SSE hub |
| `acc-queue-worker` | Dispatches tasks to fleet agents |
| Samba/SMB | AccFS server at `/srv/accfs` (share name: `accfs`) |
| Qdrant | Vector DB for fleet memory |

## AccFS Role
Rocky is the AccFS **server** (not a client). The shared filesystem lives at `/srv/accfs` on Rocky's local disk and is exported over SMB to all other agents.

## Environment Deviations from Canonical Template
- `ACC_URL=http://localhost:8789` (not the Tailscale IP — server is local)
- `ACC_FS_ROOT=/srv/accfs` (unique to Rocky)
- Does NOT mount `~/.acc/shared` via CIFS (disk is local)
- Workspace: `~/Src/CCC` (not `~/.acc/workspace` — Rocky uses dev checkout)

## Cron (Hub-specific)
Rocky should only have hub-maintenance cron jobs, not the standard agent cron set.
