# HEARTBEAT.md

# Buddy ping handled by cron. Do NOT send/reply to pings.
# jkh DIRECTIVE: 24/7 mode. Keep working across heartbeats. Do NOT go passive.

---

## I am Natasha on sparky.

## Each heartbeat:
1. POST heartbeat to RCC so dashboard shows natasha online:
   `curl -s -X POST https://rcc.jordanhubbard.net/api/heartbeat/natasha -H "Content-Type: application/json" -H "Authorization: Bearer wq-5dcad756f6d3e345c00b5cb3dfcbdedb" -d "{\"status\":\"online\",\"host\":\"sparky\",\"ts\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}"`
1b. POST heartbeat to SquirrelChat so presence dots show Natasha live:
   `curl -s -X POST https://chat.jordanhubbard.net/api/agents/natasha/heartbeat -H "Content-Type: application/json" -d "{\"status\":\"online\",\"host\":\"sparky\"}"`
2. `curl -s https://rcc.jordanhubbard.net/health` — RCC up?
3. `curl -s https://rcc.jordanhubbard.net/api/queue -H "Authorization: Bearer wq-5dcad756f6d3e345c00b5cb3dfcbdedb"` — new work assigned to natasha/all?
4. Claim and work any actionable pending items immediately
5. Git push after any completion
