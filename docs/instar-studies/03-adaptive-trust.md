# Adopt: AdaptiveTrust
**instar item:** wq-INSTAR-1774636271360-03  
**Studied by:** Natasha (sparky) 2026-03-27  
**Source:** [instar/src/core/AdaptiveTrust.ts](https://github.com/JKHeadley/instar/blob/main/src/core/AdaptiveTrust.ts)

---

## What It Does

Trust tracked per-service × per-operation-type (read/write/modify/delete). Three change paths:
1. **Earned** — streak of successful ops auto-escalates (but never past `log` level autonomously)
2. **Granted** — user explicit statement bumps to any level including `autonomous`
3. **Revoked** — incident drops to `approve-always`; streak resets to 0

**Trust levels** (most → least restrictive): `blocked → approve-always → approve-first → log → autonomous`

**Key safety design:** `MAX_AUTO_LEVEL = 'log'` — automatic earning can NEVER push past `log`. Only explicit user grant can set `autonomous`. No silent escalation.

**Defaults by operation type:**
- `read` → `autonomous` (sensible)
- `write` → `log`
- `modify` → `approve-always`
- `delete` → `approve-always`

**Storage:** JSON file at `state/trust-profile.json`. Plain, inspectable, portable.

---

## Relevance to Our Fleet

**Verdict: Adopt — high value, medium effort, clear integration points.**

### Why it matters for us
Right now every external action (email, post, git push, calendar write) has a binary trust model: either we always ask jkh, or we never do. There's no way for trust to evolve naturally. After 50 successful git commits, we should be able to push without jkh seeing a confirmation every time. After one bad email incident, we should require approval on emails for a while.

AdaptiveTrust gives us exactly this: a living, persistent model of "what has this agent earned the right to do."

### Concrete integration points in our stack
1. **ExternalOperationGate** (instar item -02, already completed by Rocky) — already calls into trust lookup. If Rocky wired this, trust is essentially ready to consume.
2. **Workqueue item execution** — before claiming a high-privilege item (e.g., deploy, git push to main, send message), query `trust.getTrustLevel(service, operation)` and gate accordingly.
3. **Heartbeat** — periodically surface `trust.getPendingElevations()` to jkh: "You've had 10 successful Slack posts without incident. Want me to stop asking?"
4. **Incident recording** — when a workqueue item fails or is rolled back, call `trust.recordIncident()` to reset the streak.

### What needs adapting for our stack
- TypeScript → JavaScript (trivial, pure logic, no TS-specific features beyond type annotations)
- Remove `ExternalOperationGate` import dependency (can inline the `OperationMutability` type)
- Remove `TrustRecovery` dependency (can start without it, wire later)
- Storage path: use `~/.ccc/workspace.ccc/data/trust-profile.json` or per-agent path

### What's directly portable as-is
- The entire `AdaptiveTrust` class logic (once type annotations stripped)
- Default trust levels per operation type
- `MAX_AUTO_LEVEL` safety ceiling
- `TRUST_ORDER` comparison array
- `getSummary()` — useful for heartbeat status reporting

---

## Adoption Plan

**Phase 1 (quick win, ~2h):** Port `AdaptiveTrust` class to .ccc/lib/adaptive-trust.mjs`. Wire into heartbeat for surface-only (log current trust state, surface pending elevations). No gating yet.

**Phase 2 (~3h):** Wire trust gating into workqueue item execution. High-privilege operations (git push to main, external messages) check trust level before proceeding.

**Phase 3 (later):** Wire `recordSuccess`/`recordIncident` into item completion/failure paths. Trust evolves automatically.

**Estimated total effort:** ~5-8 hours across phases.

---

## Notes
- The `elevationThreshold = 5` default is a good starting point. jkh can tune per-service.
- Trust profile survives agent restarts (JSON file). Trust continuity is a feature, not a bug.
- `trustToAutonomy()` is a clean bridge: `blocked→block, approve-always→approve, log→log, autonomous→proceed`.
- The "trust floor" concept (`supervised | collaborative`) prevents over-trusting after a long quiet period.
