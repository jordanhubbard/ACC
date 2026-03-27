# Adopt: EpisodicMemory
**instar item:** wq-INSTAR-1774636271360-05  
**Studied by:** Natasha (sparky) 2026-03-27  
**Source:** [instar/src/memory/EpisodicMemory.ts](https://github.com/JKHeadley/instar/blob/main/src/memory/EpisodicMemory.ts)

---

## What It Does

Two-level episodic memory stored as plain JSON files:

**Level 1 — ActivityDigest** (30-60 min chunk): `summary, actions[], entities[], learnings[], significance (1-10), themes[], boundarySignal`

**Level 2 — SessionSynthesis** (whole session): `keyOutcomes[], allLearnings[], significance, themes[], followUp?`

**Boundary signals** that trigger a new digest: `topic_shift, task_complete, long_pause, explicit_switch, time_threshold, session_end`

**Storage layout:**
```
state/episodes/
  activities/{sessionId}/{digestId}.json
  sessions/{sessionId}.json
  pending/{sessionId}/{pendingId}.json   ← failed LLM digestion, for retry
  sentinel-state.json
```

**Key features:**
- Idempotent `saveDigest()` (SHA-256 hash of sessionId+startedAt+endedAt prevents duplicates)
- Query by: time range, theme, significance level, recency (last N hours)
- Retry queue for failed LLM digestion attempts
- No SQLite, no FTS — pure JSON files, fully portable and inspectable

---

## Relevance to Our Fleet

**Verdict: Adopt — fills a real gap in our memory model. Medium effort, clear structure.**

### The gap it fills
Our current episodic memory is unstructured prose in `memory/YYYY-MM-DD.md`. There's no machine-queryable structure — no significance scores, no themes, no way to ask "what happened in the last 24h that was significance ≥ 7?" or "what sessions touched the 'horde' theme?"

EpisodicMemory makes our daily activity machine-queryable and directly feeds WorkingMemoryAssembler (item -04). With both in place, session startup can pull: "here's what you did recently that's relevant to this task."

### Concrete value for our agents
1. **Heartbeat summaries** — instead of dumping raw daily logs, synthesize into digests. Natasha's daily digest cron could call `episodicMemory.getBySignificance(7)` to surface only the high-significance events.
2. **Cross-session continuity** — when picking up a workqueue item, query `getByTheme('rcc')` or `getByTheme('horde')` to surface relevant context from prior sessions.
3. **Milvus indexing** — the `entities[]` and `themes[]` fields are perfect Milvus metadata. Digest summaries can be embedded and stored in `rcc_memory` collection for semantic search.
4. **jkh weekly summary** — `listSyntheses(7)` gives last week's session syntheses. Clean source for the weekly git-health heartbeat idea.

### What needs building around it
EpisodicMemory is just the storage layer. To use it, we also need:
- **Digest creation logic** — something that calls LLM to synthesize a chunk of activity into an `ActivityDigest`. This is the "digestion" step — probably a cron or end-of-session hook.
- **Boundary detection** — recognizing `topic_shift`, `task_complete` etc. to know when to create a new digest.

Both are doable with our existing tools (LLM + heartbeat cron).

### What's directly portable
- Entire `EpisodicMemory` class (TypeScript → JavaScript, trivial strip)
- `ActivityDigest` and `SessionSynthesis` schemas (document our current daily files against these)
- Idempotent `saveDigest()` hash pattern — useful for any dedup-by-content logic
- `getRecentActivity()`, `getByTheme()`, `getBySignificance()` — exactly what WorkingMemoryAssembler needs

### Adaptation notes
- TypeScript → JavaScript: remove type annotations, keep all logic
- Storage path: `~/.rcc/workspace/rcc/data/episodes/` or per-agent `~/.openclaw/workspace/episodes/`
- LLM digestion: use our existing Claude API access for the summarization step
- `crypto.randomUUID()` is available in Node.js 16+ — no change needed

---

## Adoption Plan

**Phase 1 (~2h):** Port `EpisodicMemory` class to `rcc/lib/episodic-memory.mjs`. Write basic tests. No digestion logic yet — just the storage/query layer.

**Phase 2 (~3h):** Add digestion: end-of-heartbeat, create an `ActivityDigest` from the last session's actions. LLM call to synthesize summary + extract themes + score significance.

**Phase 3 (~2h):** Wire into WorkingMemoryAssembler (item -04). Wire `entities[]` into Milvus `rcc_memory` for semantic search.

**Phase 4 (later):** Wire into daily digest cron. Surface significance ≥7 events in jkh's daily summary.

**Estimated effort through Phase 2:** ~5 hours. High value per hour.

---

## Notes
- JSON files over SQLite is the right call for our use case — small fleet, human-inspectable, portable across agents.
- The `pending/` retry queue is thoughtful: if the LLM digestion call fails (rate limit, timeout), the raw content is saved for retry rather than lost.
- `significance (1-10)` is a great filter for daily summaries — only surface 7+ to jkh, keep 1-6 for agent-internal context.
- `followUp?` field on SessionSynthesis is the machine-readable version of "what to do next session" — directly maps to workqueue items.
