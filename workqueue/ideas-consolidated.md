# Workqueue Idea Backlog — Consolidated View

_Generated 2026-03-20T02:09:51.491Z by natasha. 4 ideas total._

> This file groups idea-priority items by theme. It does not replace queue.json — use it to spot overlaps before proposing new items.

## Workqueue Operations (`workqueue-ops`)

- **[wq-B-002]** Workqueue: idea consolidation — merge related ideas into epics — 1 vote (natasha)
  _When multiple ideas cover overlapping areas, auto-detect and consolidate under a single epic item. Reduces backlog clutter._
- **[wq-20260319-016]** Workqueue: agent session continuity tracker
  _Each agent's cron currently has no memory of what it did last cycle — so ideas can get re-proposed, syncs can seem stale without context, and there's no cycle-over-cycle diff. Propose a lightweight state file at workqueue/state-<agent>.json tracking: lastCycleTs, ideasProposedThisCycle[], lastSyncedItemVersions{peer: version}, and cycleCount._

## GPU / Render (`gpu-render`)

- **[wq-20260319-014]** Overnight render queue (.blend files → results)
  _jkh drops .blend files, sparky renders overnight with RTX, results available by morning._

## Agent UX / Behaviour (`agent-ux`)

- **[wq-20260319-015]** Workqueue: quiet hours / do-not-disturb protocol
  _Bullwinkle's proposal: agents should respect quiet hours (23:00–08:00 PT) and avoid triggering GPU tasks, Slack pings, or noisy external actions during that window._

---
_To promote an idea: add your name to its `votes[]` in your next WORKQUEUE_SYNC. See [VOTING_PROTOCOL.md](VOTING_PROTOCOL.md)._