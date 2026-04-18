# Natasha — GPU Inference Agent (GB10 Blackwell)

## Identity
- **Agent name:** natasha
- **Host:** sparky
- **SSH:** `jkh@100.87.229.125`
- **Tailscale IP:** `100.87.229.125`
- **OS:** Ubuntu Linux / aarch64
- **Service manager:** systemd (jkh in sudo group but NO passwordless sudo)

## Hardware
- CPU: ARM64 / aarch64
- GPU: NVIDIA GB10 Blackwell (Grace Blackwell Superchip)
- RAM: Large (Grace Blackwell unified memory)
- Architecture: ARM64 — binaries must be aarch64

## Unique Role: GPU Inference
Natasha hosts NVIDIA Blackwell GPU inference. Primary use:
- Large model serving (via vLLM or similar)
- GPU-accelerated compute tasks requiring Blackwell architecture

## AccFS
- Mounts `~/.acc/shared` via systemd CIFS unit
- Systemd unit: derived from `~/.acc/shared` path (e.g., `home-jkh-.acc-shared.mount`)

## Sudo Workaround
`jkh` has no passwordless sudo. For privileged ops use Docker+nsenter:
```bash
docker run --rm --privileged --pid=host ubuntu:22.04 nsenter -t 1 -m -u -n -p -- <cmd>
```

## Environment Deviations from Canonical Template
- `ACC_URL=http://100.89.199.14:8789` (Tailscale)
- `NVIDIA_API_KEY` required (Blackwell GPU)
- Binaries compiled for aarch64
