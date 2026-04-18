# Bullwinkle — macOS Desktop Agent

## Identity
- **Agent name:** bullwinkle
- **Host:** puck
- **SSH:** `jkh@100.87.68.11`
- **Tailscale IP:** `100.87.68.11`
- **OS:** macOS / arm64 (Apple Silicon)
- **Service manager:** launchd (`~/Library/LaunchAgents/`)

## Hardware
- CPU: Apple Silicon (arm64)
- GPU: Apple integrated GPU (Metal)
- RAM: Unified memory

## Unique Role: macOS Desktop
Bullwinkle handles macOS-native tasks:
- Browser automation (Safari/Chrome via AppleScript or Playwright)
- macOS desktop GUI interactions
- macOS-specific tooling (Keychain, launchd, Automator)

## AccFS
- Mounts `~/.acc/shared` via launchd plist with SMB password embedded in mount URL
- Plist: `~/Library/LaunchAgents/com.acc.accfs-mount.plist`
- **Note:** macOS CIFS mounts created by launchd are session-isolated — SSH sessions cannot see the mount. Agent process (launchd context) CAN access it. Verify via agent exec, not SSH.

## Environment Deviations from Canonical Template
- `ACC_URL=http://100.89.199.14:8789` (Tailscale)
- No `NVIDIA_API_KEY` (no NVIDIA GPU)
- launchd-managed services, not systemd
