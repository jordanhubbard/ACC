# rcc/api/routes — Route Module Extraction

This directory is the split of `api/index.mjs` (8176 lines → targeted split).

## Status

| Module | Routes | Status |
|--------|--------|--------|
| `agentos.mjs` | `/api/agentos/*`, `/api/mesh` | ✅ Extracted (Phase 1) |
| `bus.mjs` | `/bus/*`, `/api/bus/*` | ✅ Extracted (Phase 1) |
| `queue.mjs` | `/api/queue/*`, `/api/item/*` | 🔜 Phase 2 |
| `agents.mjs` | `/api/agents/*`, `/api/heartbeat/*` | 🔜 Phase 2 |
| `ui.mjs` | HTML serving routes (`/projects`, `/services`, `/timeline`) | 🔜 Phase 2 |
| `sbom.mjs` | `/api/sbom/*` | 🔜 Phase 2 |
| `issues.mjs` | `/api/issues/*` | 🔜 Phase 2 |

## Pattern

Each module exports `async function try<Name>Route(ctx)` where:
- `ctx` contains `{ req, res, method, path, url, json, readBody, isAuthed, ...sharedState }`
- Returns `true` if the request was handled (no further routing needed)
- Returns `false` to fall through to the next handler

## Shared State

All shared state (heartbeats, bus state, queue lock, AUTH_TOKENS, etc.) remains in
`index.mjs` and is passed into each module via the context object.  This avoids
circular imports while enabling the split.

## Integration

`index.mjs` calls `tryAgentOSRoute(ctx)` and `tryBusRoute(ctx)` as early-exit checks
inside `handleRequest`, replacing the inline blocks.  The extracted blocks are commented
out but kept for reference until Phase 2 cleanup removes them.
