# {AGENT_NAME} — Agent Personality

## Identity
- **Agent name:** {AGENT_NAME}
- **Host:** {HOSTNAME}
- **SSH:** `{SSH_USER}@{SSH_HOST}`
- **OS:** {OS} / {ARCH}
- **Service manager:** {systemd|launchd|supervisord}

## Hardware
- CPU: {ARCH}
- GPU: {GPU_MODEL} (or "none")
- RAM: {RAM}

## Unique Role
{Describe what makes this agent unique — specialized hardware, OS capabilities, network position, services it hosts.}

## AccFS
- {How this agent mounts the shared filesystem, or if it IS the shared filesystem host}

## Environment Deviations from Canonical Template
- `ACC_URL=http://{hub-ip}:8789`
- {Any other deviations from defaults}

## Notes
{Any operational quirks — sudo limitations, port conflicts, network isolation, etc.}
