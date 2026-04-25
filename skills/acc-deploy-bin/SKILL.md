---
name: acc-deploy-bin
description: Deploy scripts and binaries to ACC agent nodes. Use when pushing new tools to the fleet, understanding when agent-pull.sh installs scripts automatically vs. when manual push is required.
version: 1.0.0
platforms: [linux, macos]
metadata:
  hermes:
    tags: [acc, deploy, scripts, fleet]
    category: infrastructure
---

# ACC Deploy Bin

Scripts installed to `~/.local/bin/` on all agents come from `deploy/bin/` in
this repo. The install is triggered by `agent-pull.sh` — but only under specific
conditions. Understanding those conditions is critical to knowing when the pull
mechanism works and when you must push manually.

## How automatic install works

`agent-pull.sh` installs `deploy/bin/` scripts when **all three** of these are true:

1. A git pull just completed (there were new commits)
2. The diff of that pull includes at least one file under `deploy/bin/`
3. The version of `agent-pull.sh` **running this pull** already contains the install block

Condition 3 is the trap: if you add the install block to `agent-pull.sh` in the
same commit as new scripts in `deploy/bin/`, the **old** `agent-pull.sh` runs the
pull. The new `agent-pull.sh` lands in the workspace, but the install block never
fires for this pull.

```
Commit adds: deploy/agent-pull.sh (with install block) + deploy/bin/new-tool

Agent runs OLD agent-pull.sh → pulls the commit → detects deploy/bin/new-tool in diff
  → OLD agent-pull.sh has no install block → new-tool NOT installed
  → NEW agent-pull.sh is now in workspace

Next pull (no deploy/bin/ change) → NEW agent-pull.sh runs → no deploy/bin/ in diff
  → new-tool still NOT installed
```

**Result: scripts in `deploy/bin/` are never installed automatically on the first deploy.**

## When to use manual install

Always push manually after adding new scripts to `deploy/bin/` for the first
time. The pull mechanism is reliable for *updates* to existing scripts, not for
*bootstrap*.

## Manual install — all agents

Get SSH details from the fleet registry (never hardcode):

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

# Print install command for each online agent
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents" \
| python3 -c "
import json, sys
data = json.load(sys.stdin)
agents = data if isinstance(data, list) else data.get('agents', [])
for a in agents:
    if not a.get('online'):
        continue
    user = a.get('ssh_user', '')
    host = a.get('ssh_host', '')
    port = a.get('ssh_port', 22)
    name = a.get('name', '')
    if host:
        port_flag = f'-p {port} ' if port != 22 else ''
        print(f'ssh -o StrictHostKeyChecking=no {port_flag}{user}@{host}')
"
```

Then for each agent (run in parallel with `&`):

```bash
WORKSPACE_PATH=~/.acc/workspace   # or ~/.ccc/workspace on older agents

ssh -o StrictHostKeyChecking=no [-p PORT] USER@HOST "
  mkdir -p ~/.local/bin
  for f in ${WORKSPACE_PATH}/deploy/bin/*; do
    [[ -f \"\$f\" ]] && install -m 755 \"\$f\" ~/.local/bin/\$(basename \"\$f\")
  done
  echo \"Installed: \$(ls ~/.local/bin/ | grep -E 'acc-'  | tr '\n' ' ')\"
"
```

## Manual install — single script

```bash
SCRIPT=acc-file-task

ssh -o StrictHostKeyChecking=no USER@HOST "
  install -m 755 ~/.acc/workspace/deploy/bin/$SCRIPT ~/.local/bin/$SCRIPT
  echo \"OK: \$(ls -la ~/.local/bin/$SCRIPT)\"
"
```

## Verify installation

```bash
ssh -o StrictHostKeyChecking=no USER@HOST 'ls -la ~/.local/bin/acc-*'
```

Or use the fleet API to confirm the agent is online and recently heartbeated:

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/agents/AGENT_NAME" \
  | python3 -m json.tool | grep lastSeen
```

## Future-proofing: bootstrap idempotency

To make `agent-pull.sh` install `deploy/bin/` on every pull regardless of diff
(useful for ensuring new agents are always fully provisioned), add an unconditional
install block inside the pull script, not gated on `$CHANGED`. This is a potential
improvement but not yet implemented.

## Rules

- **Get SSH details from `/api/agents`**, not from git files or memory.
- **Always install manually after first-time addition** of a script to `deploy/bin/`.
- **The pull mechanism handles updates reliably** — once a script exists on agents,
  future changes to it in `deploy/bin/` will be installed automatically.
- **Run installs in parallel** using `&` + `wait` when targeting multiple agents —
  don't loop sequentially.
