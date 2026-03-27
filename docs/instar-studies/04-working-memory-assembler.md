# Adopt: WorkingMemoryAssembler
**instar item:** wq-INSTAR-1774636271360-04  
**Studied by:** Natasha (sparky) 2026-03-27  
**Source:** [instar/src/memory/WorkingMemoryAssembler.ts](https://github.com/JKHeadley/instar/blob/main/src/memory/WorkingMemoryAssembler.ts)

---

## What It Does

Token-budgeted context assembly from multiple memory layers for session startup.

**Algorithm:**
1. Extract query terms from session trigger (prompt/jobSlug) — strip stop words, keep ≤8 significant terms
2. Search SemanticMemory (FTS5, OR-like per-term merge with score boosting)
3. Search EpisodicMemory (recent 24h + theme match)
4. Search person entities from SemanticMemory
5. Budget tokens across sources (knowledge=800, episodes=400, relationships=300, total=2000)
6. Return formatted context string

**Tiered render strategy (the key insight):**
- **Top 3 entities:** full content (name + content + confidence + connections)
- **Next 7:** compact (name + first sentence + confidence %)
- **Remainder:** name-only list (`Also related: X, Y, Z`)

**Search strategy:** per-term OR-like search + full-query AND search, merged with 1.1× score boost for multi-term matches. Avoids FTS5 implicit-AND which is too restrictive for memory assembly.

**Token budgets:** `knowledge=800, episodes=400, relationships=300, total=2000`

---

## Relevance to Our Fleet

**Verdict: Adopt patterns now; full implementation when EpisodicMemory exists.**

### Why it matters
Our `memory_search` returns raw results with no budget management. On complex sessions, memory_search results either flood the context (too many results) or get truncated arbitrarily. WorkingMemoryAssembler solves this: **right context, right amount, right moment.**

For us specifically: Natasha has Milvus (semantic search), daily memory files (episodic), and MEMORY.md (long-term). There's no structured way to pull the right subset into a session without blowing the context window.

### What's immediately stealable (no dependencies)
1. **Tiered render strategy** — top-3 full, next-7 compact, rest names-only. Can apply this RIGHT NOW to our `memory_search` output formatting in WORKQUEUE_AGENT_NATASHA.md. Zero dependencies.
2. **Token budget constants** — `knowledge=800, episodes=400, relationships=300, total=2000`. Good defaults for our sessions.
3. **Stop word list** — the 80-word stop list at the bottom is directly portable. Strips generic task words like "please", "implement", "build", "check" that pollute memory queries.
4. **`extractQueryTerms()`** — strip stop words, deduplicate, take top 8 terms. Clean pattern for query term extraction before memory_search.
5. **Per-term merge with score boosting** — the `searchAndMerge()` approach (search each term separately, boost entities matching multiple terms) is more effective than a single full-query search. Apply to our Milvus queries.

### What requires EpisodicMemory first (items -05)
- Episode assembly section
- `getRecentActivity()` + `getByTheme()` calls
- Two-level tiered digest rendering

### Adaptation notes
- TypeScript → JavaScript: trivial
- `SemanticMemory.search()` → our Milvus `vectorSearch()` wrapper
- `estimateTokens()` → simple `Math.ceil(text.length / 4)` approximation
- No SQLite, no external deps — pure logic + file I/O

---

## Adoption Plan

**Phase 1 (immediate, ~1h):** Extract the tiered render strategy + stop word list + `extractQueryTerms()` as a standalone utility in `rcc/lib/memory-utils.mjs`. Wire into our existing `memory_search` post-processing.

**Phase 2 (~3h):** Build `WorkingMemoryAssembler.mjs` that wraps Milvus search with token budgeting. Use for session context injection at heartbeat startup.

**Phase 3 (after EpisodicMemory):** Wire in episodic layer for activity continuity.

**Estimated effort Phase 1:** ~1 hour (immediate value).  
**Full implementation:** ~4-6 hours.

---

## Notes
- The FTS5 implicit-AND problem is real for us too — Milvus has similar behavior with multi-vector AND queries. The per-term merge approach is worth adopting now.
- `1.1× score boost` for multi-term hits is elegant and cheap. Not worth overthinking; just multiply.
- The stop word list includes task-generic terms like "implement", "build", "test" — these are especially noisy for our workqueue-driven sessions.
- Budget of 2000 total tokens is conservative for our sessions (we have large context windows) but the tiered strategy is still the right approach regardless of budget size.
