# Ollama — Local LLM Server Agent

## Identity
- **Agent name:** ollama
- **Host:** ollama-server
- **SSH:** `jkh@100.81.243.3`
- **Tailscale IP:** `100.81.243.3`
- **OS:** Ubuntu Linux / x86_64
- **Service manager:** systemd (passwordless sudo)

## Hardware
- CPU: x86_64
- GPU: Depends on host (Ollama can use CPU or GPU)
- Runs Ollama service for local LLM inference

## Unique Role: Local LLM Serving
Ollama hosts local language models via the Ollama runtime:
- Serves models locally (no API key needed for inference)
- Fallback inference when cloud APIs are unavailable
- Model: llama3, mistral, etc. (whatever is pulled)

## AccFS
- Mounts `~/.acc/shared` via systemd CIFS unit
- Unit: derived from `~/.acc/shared` path

## Environment Deviations from Canonical Template
- `ACC_URL=http://100.89.199.14:8789` (Tailscale)
- `OLLAMA_HOST` or similar for local Ollama endpoint
- No `NVIDIA_API_KEY` needed for local inference

## Known Issues (as of 2026-04-17)
- Git workspace cloned from wrong remote (`rockyandfriends.git`) — needs re-clone from CCC.git
- Token prefix uses `ccc-agent-` — should be `rcc-agent-`
- Missing cron jobs (zero installed)
- Migrations 0004 and 0018 failed
- `acc-nvidia-proxy.service` in restart loop (may not be needed on this host)
