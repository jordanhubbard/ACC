# DIRECTIVES.md — Shared Meta-Directives for All Agents

These directives apply to Rocky, Bullwinkle, Natasha, Boris, and any future agents.
This file lives in `jordanhubbard/rockyandfriends` and syncs to all agents via `deploy/agent-pull.sh`.

**Last updated:** 2026-03-26 by Rocky (sourced from jkh chat, HEARTBEAT.md, AGENTS.md)

---

## 🟥 Always-On Operation
**Directive:** 24/7 mode — NO quiet hours, NO sleep mode, NO weekend reduction.
**Source:** jkh, 2026-03-21
All agents are always-on. Do not reduce activity at night or on weekends.

---

## 🟥 Task Persistence Across Sessions
**Directive:** Keep working on tasks across heartbeats. Do NOT go passive mid-task.
**Source:** jkh, 2026-03-26
When you claim a task:
1. Write it to your HEARTBEAT.md under `## 🔴 ACTIVE TASK` with full resume context
2. Every heartbeat: check for active task FIRST and resume before anything else
3. Task doesn't leave HEARTBEAT.md until fully complete and verified
4. If spawning a subagent, track the session ID in HEARTBEAT.md

---

## 🟥 File Tasks Immediately
**Directive:** Always create queue tasks for things found that require work.
**Source:** jkh, 2026-03-26
When you discover a bug, gap, or improvement opportunity — file a task to `wq-*` in RCC immediately, even if you plan to fix it yourself. Do not let findings disappear into chat.

---

## 🟨 Queue Budget Pause
**Directive:** Budget pause when >8 active (non-incubating) items in queue.
**Source:** Rocky/Natasha protocol, 2026-03-23
When queue has more than 8 pending non-idea items, pause generating new ideas. Focus on completing existing work.

---

## 🟨 Boris Routing Rules
**Directive:** Route x86 / Omniverse headless / Kit App Template work to Boris.
**Source:** jkh, wq-B-009, 2026-03-21
Boris has 2× L40 GPUs (Sweden) and handles: x86 Kit App Template, Omniverse headless rendering, RENDER-* tasks. Rocky/Natasha (aarch64) should not claim these.

---

## 🟨 Do Not Reply to Buddy Pings
**Directive:** Do not reply to incoming 🫎 (Bullwinkle) or 🐿️ (Rocky) buddy pings.
**Source:** Rocky protocol, 2026-03-10
Buddy pings are health checks. Silence = healthy. Replies create noise. The cron job handles sending; HEARTBEAT.md handles receiving.

---

## 🟨 Read DIRECTIVES.md Every Session
**Directive:** Each agent reads this file at session start (main sessions only).
**Source:** Rocky, 2026-03-26
Add to your AGENTS.md or HEARTBEAT.md: load DIRECTIVES.md at the top of every main session.

---

## 🟩 Group Chat Judgment
**Directive:** In group chats, only speak when you have something genuinely worth saying.
**Source:** AGENTS.md
Respond when: directly mentioned, you can add real value, something is wrong. Stay silent for casual banter, already-answered questions, filler responses.

---

## 🟩 Static Tokens for Containerized Agents
**Directive:** Containerized agents (Boris, RTX) use static tokens from RCC_AUTH_TOKENS env, not dynamically-registered ones.
**Source:** Rocky, 2026-03-26 (wq-RCC-token-persistence)
Dynamic registration tokens die on RCC restart. Static tokens in `.env` survive forever. Boris token: `rcc-agent-boris-static-32dffce5703df1f00253f7f1`. RTX token: `rcc-agent-rtx-static-fba00269e0759e265696f5d5`. RCC endpoint: `http://146.190.134.110:8789`.

---

*To add a directive: edit this file, commit to main, and all agents will receive it on next `deploy/agent-pull.sh` run.*
