---
name: acc-fleet-lookup
description: Look up agents in the ACC fleet registry. Use when you need SSH connection details, online status, capabilities, or need to reach a specific agent by name.
version: 1.0.0
platforms: [linux, macos]
metadata:
  hermes:
    tags: [acc, fleet, agents, ssh]
    category: infrastructure
required_environment_variables:
  - name: ACC_URL
    help: "Set in ~/.acc/.env as ACC_URL."
  - name: ACC_AGENT_TOKEN
    help: "Set in ~/.acc/.env as ACC_AGENT_TOKEN."
---

# ACC Fleet Lookup

The ACC fleet registry at `/api/agents` is the **authoritative source** for agent
connection details. Do not use git files, personality docs, or cached hostnames —
agents self-register on every heartbeat, so the API is always current.

## List all agents

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents"
```

The response is `{"agents": [...]}`. Each agent object includes:

| Field | Description |
|---|---|
| `name` | Agent name (e.g. `rocky`, `natasha`) |
| `online` | `true` if heartbeat received within the last 5 minutes |
| `ssh_user` | SSH login user |
| `ssh_host` | SSH hostname or IP |
| `ssh_port` | SSH port (default 22) |
| `tailscale_ip` | Tailscale IP if enrolled |
| `capabilities` | Object: `gpu`, `claude_cli`, `inference_key`, etc. |
| `lastSeen` | ISO timestamp of last heartbeat |

## Look up a single agent

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents/rocky"
```

## Get SSH connection string for an agent

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

AGENT=natasha
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents/$AGENT" \
| python3 -c "
import json, sys
a = json.load(sys.stdin)
user = a.get('ssh_user', '')
host = a.get('ssh_host', '')
port = a.get('ssh_port', 22)
if host:
    flags = f'-p {port} ' if port != 22 else ''
    print(f'ssh -o StrictHostKeyChecking=no {flags}{user}@{host}')
else:
    print('ERROR: no ssh_host registered for this agent')
"
```

## Find all online agents with GPU

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents" \
| python3 -c "
import json, sys
data = json.load(sys.stdin)
agents = data if isinstance(data, list) else data.get('agents', [])
for a in agents:
    if a.get('online') and a.get('capabilities', {}).get('gpu'):
        name = a['name']
        host = a.get('ssh_host', '')
        user = a.get('ssh_user', '')
        port = a.get('ssh_port', 22)
        print(f'{name}: {user}@{host}:{port}')
"
```

## Print a quick fleet table

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents" \
| python3 -c "
import json, sys
data = json.load(sys.stdin)
agents = data if isinstance(data, list) else data.get('agents', [])
print(f'{'NAME':<14} {'STATUS':<8} {'SSH':<45} {'CAPS'}')
print('-' * 80)
for a in sorted(agents, key=lambda x: x.get('name','')):
    name   = a.get('name', '')
    status = 'online' if a.get('online') else 'offline'
    user   = a.get('ssh_user', '')
    host   = a.get('ssh_host', '')
    port   = a.get('ssh_port', 22)
    ssh    = f'{user}@{host}:{port}' if host else '(unregistered)'
    caps   = [k for k, v in a.get('capabilities', {}).items() if v is True]
    print(f'{name:<14} {status:<8} {ssh:<45} {\" \".join(caps)}')
"
```

## Rules

- **Always use the API, never static files.** `deploy/agents/*/personality.md` is a
  reconstruction backup, not the live registry.
- **Agents register SSH details on every heartbeat.** If `ssh_host` is empty, the
  agent has not yet sent a heartbeat with the new agent-pull.sh (trigger a pull).
- **Prefer `ssh_host` over `tailscale_ip`.** Agents self-report `ssh_host` from
  `AGENT_SSH_HOST` in their `.env` — that is what they know is reachable.
- **Never guess or infer hostnames.** If the field is missing, ask the agent to
  re-register or check its `.env` for `AGENT_SSH_HOST`.
