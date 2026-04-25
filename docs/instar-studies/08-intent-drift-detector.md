# Study: IntentDriftDetector
**instar item:** wq-INSTAR-1774636271360-08  
**Studied by:** Natasha (sparky) 2026-03-27  
**Source:** [instar/src/core/IntentDriftDetector.ts](https://github.com/JKHeadley/instar/blob/main/src/core/IntentDriftDetector.ts)

---

## What It Does

Fully deterministic (no LLM calls) drift detection that compares two sliding time
windows of decision journal data to surface alignment signals.

**Two analysis modes:**

1. **`analyze(windowDays=14)`** — Drift detection: compares current vs previous window.
   Returns `DriftAnalysis` with signals, drift score (0–1), and summary.

2. **`alignmentScore(periodDays=30)`** — Point-in-time alignment grade (A–F, score 0–100)
   from four weighted components:
   - `conflictFreedom` (30%) — inverse of conflict rate
   - `confidenceLevel` (25%) — average decision confidence
   - `principleConsistency` (25%) — Shannon entropy of principles used
   - `journalHealth` (20%) — active-days-per-period ratio

**Four drift signals:**
| Signal | Threshold | Severity |
|--------|-----------|----------|
| `conflict_spike` | conflict rate 2× previous | warning |
| `conflict_spike` | conflict rate 3× previous | alert |
| `confidence_drop` | avg confidence drops >0.15 | warning |
| `confidence_drop` | avg confidence drops >0.25 | alert |
| `principle_shift` | top principle changed | info |
| `principle_shift` | ≥2 of top 3 principles changed | warning |
| `volume_change` | decision count drops >50% | warning |
| `volume_change` | decision count increases >3× | info |

---

## Relevance to Our Fleet

**Honest assessment: Medium-term future value; not immediately actionable.**

### Why it matters eventually
We have SOUL.md, MEMORY.md, and daily logs — but no structured mechanism to detect
when an agent is drifting from its stated principles over time. IntentDriftDetector
would answer: "Is Natasha still acting like Natasha, or has her behavior gradually
shifted?" This is the kind of metacognitive health check we'll want as the fleet
grows more autonomous.

### Why it's not ready to ship now
The system requires a **DecisionJournal** — a structured log where the agent records
each significant decision with fields: `timestamp`, `principle`, `confidence`,
`conflict`. We don't currently log decisions in this structured way. Our memory system
(MEMORY.md + daily files) is narrative prose, not a decision log.

**Prerequisite stack:**
1. Define `DecisionJournalEntry` schema for our agents
2. Add decision-logging hooks to agent behavior (write to journal when making
   a significant call)
3. Build `DecisionJournal.read()` on top of our daily memory files or a new
   structured store
4. Then IntentDriftDetector can run against that data

### What's directly reusable
- **AlignmentScore formula** (conflict-freedom + confidence + principle-consistency +
  journal-health) is clean and portable. Could adapt as a heartbeat health-check
  even with partial data.
- **Drift score weighting** (info=0.1, warning=0.3, alert=0.5, cap at 1.0) is
  a nice pattern for any multi-severity signal aggregation.
- **Shannon entropy for principle consistency** is elegant — worth stealing for
  any behavioral consistency metric.

---

## Recommendation

**Study only — do not adopt until DecisionJournal exists.**

Create a follow-up idea item to design the DecisionJournal schema for our fleet
when we're ready to add structured decision logging. At that point, IntentDriftDetector
drops in almost unchanged (it's pure TypeScript, fully deterministic, no external deps).

**Estimated adoption effort when prereqs exist:** ~2 hours to port to JS + wire up.

---

## Notes
- No LLM cost — runs entirely on stored data. Cheap to run on every heartbeat.
- Works best with ≥30 days of decision data. Sparse journals give noisy results
  (the `journalHealth` score component handles this gracefully with a 0 baseline).
- The `principleConsistency` Shannon entropy formula is elegant and worth remembering.
