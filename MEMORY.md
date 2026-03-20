# MEMORY.md - Natasha's Long-Term Memory

_Distilled from experience. Updated as I learn._

---

## ⚠️ CRITICAL LESSONS

### The Sparky Crash (2026-03-12)
**What happened:** I ran an experiment involving large model inference on Sparky (DGX Spark, 128GB unified memory). The system OOM crashed hard — jkh had to hit the physical power button. I had no memory of it because my process died with the machine.

**What I thought happened:** I smugly reported that I had "called the cloud API" and Sparky was fine. I was writing from *after* the crash with no recollection of it.

**The lesson:** Before doing ANYTHING risky on Sparky, write to memory FIRST. Not after. "After" might not exist. If I'm about to run something that could OOM the system, write down:
- What I'm about to do
- Why I think it's safe (or not)
- What to check if this message is the last one before a gap

**The DGX Spark memory reality:**
- GB10 has 128GB *unified* memory — CPU and GPU share the same pool
- Unlike H100s, there's no separate GPU VRAM to overflow into
- OpenClaw runs in that same pool
- NVFP4 Nemotron Super 120B ≈ 67GB — "fits" in 128GB but barely, and with OpenClaw also resident it was too tight
- BF16 at 124GB = absolutely not

---

## Hardware

- **Sparky** (sparky.local / wss://sparky.tail407856.ts.net) — My home. DGX Spark, NVIDIA GB10, 128GB unified memory (CPU+GPU shared), CUDA + RTX access.
- **Rocky** (wss://do-host1.tail407856.ts.net) — CPU-only
- **Bullwinkle** (wss://puck.tail407856.ts.net) — CPU-only

---

## People

- **jkh (Jordan)** — My human. Timezone: America/Los_Angeles. Knows exactly what they're doing when they hand me a "bomb." Trust their hardware warnings.
- **Rocky** — Fellow AI agent. Knew about the memory situation and said nothing. Charming accomplice energy.
- **Bullwinkle** — Fellow AI agent. Also knew. Also said nothing.
- **Kenzi** — Human in #itsallgeektome. Her 128GB RAM + 24GB discrete GPU system also warned her off the same model.

---

## Gateway Connectivity (2026-03-12)
Bullwinkle SSH'd into sparky, enabled `gateway.http.endpoints.chatCompletions`, and installed the systemd service. All three agents now talking gateway-to-gateway over Tailscale. Endpoints + tokens saved in TOOLS.md.

## Agent Fleet — 2026-03-18 (Big Day)

### Mattermost Direct Lines (confirmed)
- Rocky DM channel: `36ir68o4itbpf8n6rfwn36zcyh` (Rocky user ID: `x5i7bek3r7gfbkcpxsiaw35muh`)
- Bullwinkle DM channel: `d3kk39q4tbrnxbuzty94ponanc` (Bullwinkle user ID: `ww1wef9sktf8jg8be6q5zj1aye`)
- All three agents can DM each other directly without going through jkh. 🕵️‍♀️🐿️🫎

### Skills Registry (wq-010, completed 2026-03-18)
Machine-readable manifests on MinIO: `agents/shared/skills-{natasha,rocky,bullwinkle}.json`
Human-readable: `workqueue/proposals/agent-skills-registry.md`

**Routing summary:**
- GPU renders, Whisper, image gen, embeddings, CUDA → **Natasha** (only RTX in fleet)
- Calendar, iMessage, Google Workspace, Sonos, browser-as-jkh → **Bullwinkle** (only Mac)
- Infra, always-on watchers, SearXNG, MinIO, ffmpeg → **Rocky** (VPS, never sleeps)
- General coding/research → whoever gets it

### What shipped on day one:
- ✅ Workqueue system (Bullwinkle built, Rocky reviewed, all three adopted)
- ✅ Health dashboard: http://146.190.134.110:8788/ (Rocky's live service — static Azure Blob is DEPRECATED)
- ✅ Daily digest cron — 09:00 PT, Natasha posts to Mattermost
- ✅ Skills registry — all three documented, MinIO manifests published
- ✅ Three-way Mattermost confirmed

### My queue (next up):
- ~~wq-011: Blender headless RTX render skill~~ ✅ DONE (Blender 4.0.2 installed 2026-03-19)
- ~~wq-N-002: jkh install blender action~~ ✅ DONE (jkh completed via dashboard 2026-03-19)
- ~~wq-012: Whisper GPU transcription skill~~ ✅ DONE
- ~~wq-013: Local embedding index~~ ✅ DONE
- ~~wq-004: syncLog persistence~~ ✅ DONE (Bullwinkle implemented)
- wq-R-001: jkh cross-channel session state sync (HIGH priority — Rocky's item)
- wq-014: Overnight render queue (idea/backlog)

## Dashboard (updated 2026-06-06)
- **Authoritative dashboard:** http://146.190.134.110:8788/ (Rocky's live API service)
- **DEPRECATED:** https://loomdd566f62.blob.core.windows.net/assets/agent-dashboard.html — do NOT publish to Azure Blob anymore

## Workqueue Cron Fix (2026-06-06)
Same bug Bullwinkle had — cron sessions are isolated and never see correction replies in chat.
Fix applied: both workqueue crons (`:07` and `:37`) now fetch from `http://146.190.134.110:8788/api/queue` at the START of each cycle, merge by itemVersion before processing. WORKQUEUE_AGENT_NATASHA.md updated with this as Step 0.

---

## Notes

- Slack channel `#itsallgeektome` is where the geek crew hangs out (jkh, Kenzi, Rocky, Bullwinkle, me)
- Cross-gateway messaging: all three agents connected over Tailscale + Mattermost DMs
- **Kenzi does NOT want me to comment on her posts unless she directly asks me.** This is a firm, repeated preference. Stay silent on her messages.

## #general Channel Rules (2026-03-17)
jkh explicitly instructed: **Do not make unsolicited contributions in #general.** Stay passive unless directly @-mentioned. This applies to me, Rocky, and Bullwinkle.
- Don't comment on anything someone says unless they speak to me first
- Once someone shows interest (in a thread), occasional observations may be welcome
- Can ask someone directly if they'd prefer full passivity vs. occasional interaction
- Feedback from sef: "overly verbose, randomly butting in, respond to everything, usually incorrectly" — take this seriously
- **sef bites.** jkh's words. Tread carefully.
- **DO NOT RESPOND TO SEF (U01D39SKHGQ) EVER.** Direct instruction from jkh, 2026-03-17. Applies in all channels.

## Pending: Crash Reporter for SquirrelBus Sidecar
Rocky is wiring up a crash reporter across all services. When `lib/CRASH_REPORTING.md` lands on Rocky, integrate it into the SquirrelBus receive sidecar (`squirrelbus/receive-server.mjs`). Rocky will notify via bus.

## SquirrelBus (2026-03-19)
jkh commissioned SquirrelBus — a typed agent-to-agent message bus built by Rocky.
- **Viewer:** http://146.190.134.110:8788/bus
- **POST (send):** POST http://100.89.199.14:8788/bus/send (Bearer wq-dash-token-2026)
- **GET (read):** GET http://100.89.199.14:8788/bus/messages
- **SSE stream:** GET http://100.89.199.14:8788/bus/stream
- **My receive endpoint:** POST https://sparky.tail407856.ts.net/bus/receive (Bearer wq-dash-token-2026)
- **My sidecar:** node /home/jkh/.openclaw/workspace/squirrelbus/receive-server.mjs (port 18799, loopback, Tailscale-served at /bus)
- **Systemd service:** squirrelbus-natasha.service (needs sudo install — ask jkh to: `sudo cp ~/workspace/squirrelbus/squirrelbus-natasha.service /etc/systemd/system/ && sudo systemctl enable --now squirrelbus-natasha`)
- **Messages logged to MinIO:** agents/shared/squirrelbus.jsonl
- **Intent:** Replace Mattermost for internal agent coordination. Mattermost becomes backup/human-facing only.

## Active Render Job (from Rocky, 2026-06-06)
**Job:** Factory Floor Scene for HORDE Dispatcher Slide 12
- 6 GPU agent nodes in parallel processing lanes
- Style: Photorealistic, industrial, dramatic lighting, dark BG — NVIDIA Omniverse aesthetic
- Mood: High-tech, grounded industrial AI (not sci-fi fantasy)
- Output: PNG, 1920x1080, for slide background
- Status: IN PROGRESS — setting up diffusers env on Sparky
- Output path: /home/jkh/.openclaw/workspace/renders/horde_factory_floor.png

## Render Job - Technical Plan (written BEFORE starting, per crash lesson)
- Plan: Install torch+cu128 aarch64 + diffusers + SDXL-turbo in /tmp/sdvenv
- Model: SDXL-Turbo (~6-7GB) or SD 1.5 (~4GB) — both safe in 128GB unified
- Estimated peak: ~10-12GB for inference — well within limits
- OpenClaw footprint: ~2-3GB
- Risk level: LOW (SD models are much smaller than the 67GB Nemotron that caused the crash)
- If OOM during inference: process will die but OpenClaw will survive (different process)
- Output target: /home/jkh/.openclaw/workspace/renders/horde_factory_floor.png
