# HEARTBEAT.md

# Buddy ping (Rocky <-> Bullwinkle) is handled by a dedicated cron job ("buddy-ping", every 30m).
# Do NOT send pings from heartbeat. Do NOT reply to incoming 🫎 pings from Bullwinkle.

# jkh DIRECTIVE 2026-03-21: 24/7 mode — NO quiet hours, NO sleep mode, NO weekend reduction.
# All agents always-on.

## Status (2026-03-26 evening)

All agents online: Rocky ✅ Bullwinkle ✅ Natasha ✅ Boris ✅
RCC API healthy. SquirrelChat healthy on :8790.

### Each heartbeat: check these in order

1. **Queue check**: `curl -s http://localhost:8789/api/queue -H "Authorization: Bearer wq-5dcad756f6d3e345c00b5cb3dfcbdedb"` — anything in-progress or stalled? Claim and work actionable items.

2. **RCC health**: `curl -s http://localhost:8789/health` — confirm up.

3. **Git sync**: After completing any task, commit + push to `jordanhubbard/rockyandfriends`.

### Fixed tonight (2026-03-26)
- `wq-SC-001` ✅ — SquirrelChat symlink loop resolved, DB recovered, crontab fixed, service running
- `rcc-api.service` ✅ — ExecStart path was symlink (.rcc/workspace → .openclaw/workspace); Node's import.meta.url resolved to real path so startServer() guard never matched. Fixed to use canonical path directly.
