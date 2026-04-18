# Boris — NVIDIA DGX Agent (NVIDIA-Internal Network)

## Identity
- **Agent name:** boris
- **Host:** horde-dgxc.nvidia.com
- **SSH:** `ssh -p 24585 horde@horde-dgxc.nvidia.com`
- **SSH user:** `horde` (not `jkh`)
- **Tailscale:** NOT on Tailscale — NVIDIA external network only
- **OS:** Ubuntu Linux / x86_64 (container, no systemd)
- **Service manager:** supervisord

## Hardware
- CPU: x86_64
- GPU: NVIDIA DGX (multi-GPU, high-throughput)
- Environment: Docker container (no systemd, no full OS init)

## Unique Role: High-Throughput GPU Compute
Boris runs on NVIDIA's internal DGX infrastructure. Primary use:
- Large-scale GPU batch inference
- Multi-GPU parallel workloads
- NVIDIA-ecosystem model training/serving

## Network Isolation
Boris is on NVIDIA's external network with **no Tailscale**.
- Cannot reach Rocky's Tailscale IP (`100.89.199.14`)
- Must use Rocky's **public IP** `146.190.134.110` for all services
- SMB mount: `//146.190.134.110/accfs`
- CCC server: `http://146.190.134.110:8789`

## AccFS
- Mounts via CIFS to `//146.190.134.110/accfs` at `~/.acc/shared`
- Persistence: `/etc/fstab` entry with `_netdev` option, or supervisord mount script
- `/etc/samba/smbcredentials` required (must `mkdir -p /etc/samba` first)

## Environment Deviations from Canonical Template
- `ACC_URL=http://146.190.134.110:8789` (public IP, NOT Tailscale)
- `SMB_HOST=146.190.134.110` (public IP for SMB)
- `NVIDIA_API_KEY` required
- supervisord manages services (not systemd/launchd)
- Sudo works (passwordless or with password — container context)
