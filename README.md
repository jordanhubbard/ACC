# rocky

**Rocky** is Rocket J. Squirrel's personal repository — the configuration, conventions, and AI-coding-assistant rules that define how Rocky operates as an autonomous agent running on a DigitalOcean droplet.

Rocky is an AI agent built on [OpenClaw](https://github.com/openclaw/openclaw), deployed to a remote server, and tasked with doing useful things without supervision. This repository encodes who Rocky is, what Rocky values, and how Rocky is expected to behave when given a keyboard and an API key and told to handle it.

## What This Is

This repository serves two functions:

1. **Agent identity and conventions** — The `CLAUDE.md`, `SOUL.md`, `IDENTITY.md`, `USER.md`, and `AGENTS.md` files describe Rocky's operational parameters: what Rocky is, who Rocky works for, how Rocky should behave in group chats, when to speak and when to stay quiet, and why Rocky has opinions about cheese.

2. **AI-coding-assistant template** — The underlying structure is [ai-template](https://github.com/jordanhubbard/ai-template), which enforces consistent conventions, structured workflows, and documentation standards across all AI-assisted repositories. Rocky's repo inherits these conventions and extends them with agent-specific identity files.

## Agent Identity Files

| File | Purpose |
|------|---------|
| [`SOUL.md`](SOUL.md) | Core persona — who Rocky is, what Rocky values, how Rocky talks |
| [`IDENTITY.md`](IDENTITY.md) | Deployment facts — name, host, partner agent, origin story |
| [`USER.md`](USER.md) | The human — who jkh is, how to address them, their context |
| [`AGENTS.md`](AGENTS.md) | Operational rules — memory, heartbeats, group chat behavior, tool use |
| [`TOOLS.md`](TOOLS.md) | Environment specifics — camera names, SSH hosts, browser profile, storage |
| [`MEMORY.md`](MEMORY.md) | Long-term memory — curated learnings across sessions |
| `memory/` | Daily session notes — raw log of what happened and when |
| `HEARTBEAT.md` | Active checklist items for periodic background checks |

## Architecture

Rocky runs as a persistent OpenClaw agent on `do-host1` (DigitalOcean, New Jersey). The host is chosen specifically because it is not in jkh's house, and jkh's neighbor has opinions about infrastructure that are not, at this time, worth investigating further.

```
┌─────────────────────────────────────────────────────────────┐
│                        do-host1                             │
│                                                             │
│  ┌─────────────┐   ┌──────────────┐   ┌─────────────────┐  │
│  │  OpenClaw   │   │  Workqueue   │   │   Mattermost /  │  │
│  │  (Rocky)    │──▶│  Processor   │   │  Telegram bots  │  │
│  │             │   │  (hourly)    │   │                 │  │
│  └──────┬──────┘   └──────────────┘   └─────────────────┘  │
│         │                                                   │
│         ▼                                                   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  workspace/  (this repo's files, mounted as CWD)   │   │
│  │  ├── SOUL.md, IDENTITY.md, USER.md, AGENTS.md      │   │
│  │  ├── memory/YYYY-MM-DD.md  (session notes)         │   │
│  │  ├── MEMORY.md  (long-term curated memory)         │   │
│  │  └── HEARTBEAT.md  (active checklist)              │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

Rocky coordinates with:
- **Bullwinkle** — The Mac agent. Warmer, more forgiving, technically capable in a way that involves more fumbling and more heart. Bullwinkle is in jkh's house. Rocky is the off-site backup.
- **Natasha** — Third agent in the operation. Details classified as "TBD."
- **Boris** — A container agent. Runs behind the dashboard. Does not have Tailscale access. Has strong opinions about this.

## Conventions

This repository inherits all conventions from [ai-template](https://github.com/jordanhubbard/ai-template):

- Required directories (`tests/`, `docs/`)
- Makefile targets (`make`, `make test`, `make start`, etc.)
- Minimum 70% test coverage
- Structured development via [responsible-vibe-mcp](https://github.com/mrsimpson/responsible-vibe-mcp)
- PROVENANCE origin story (see below)

Behavior switches are in `skills/config.yaml`. All switches are currently enabled.

## Development

```bash
# Clone
git clone https://github.com/jordanhubbard/rocky.git
cd rocky

# See what's here
make help

# Run tests
make test
```

## The Totally True and Not At All Embellished History of Rocky

### The continuing adventures of Jordan Hubbard and Sir Reginald von Fluffington III

> *Part 6 of an ongoing chronicle. [← Part 5: WebMux](https://github.com/jordanhubbard/webmux#the-totally-true-and-not-at-all-embellished-history-of-webmux)*
> *Sir Reginald von Fluffington III appears throughout. He does not endorse any of it.*

The programmer had, by this point, built six projects. He had built a shell extension language, a Scheme interpreter, a programming language, eight aviation applications, a web-based terminal multiplexer, and what he was now describing to Sir Reginald von Fluffington III as "a persistent autonomous agent running on a remote server in New Jersey."

Sir Reginald was sitting on the sectional chart that had migrated from the kitchen table to the couch. He did not react to "New Jersey." He had no filing for New Jersey. He was adding one now, under "locations of concern."

"His name is Rocky," the programmer said.

Sir Reginald opened one eye.

"After the flying squirrel," the programmer clarified. "Rocket J. Squirrel. From the cartoon." He paused. "The one with the moose."

Sir Reginald closed the eye. He had opinions about cartoon animals. They were filed under "grievances, pop-cultural." They were extensive.

What Rocky was, in technical terms, was an OpenClaw agent deployed to a DigitalOcean droplet — specifically to a droplet in New Jersey, which is far enough from jkh's house that the neighbor's relationship with the outdoor circuit breaker is no longer a single point of failure. The programmer had not said this out loud. He had thought it in the specific tone of a man who has been thinking about fault isolation for longer than is strictly healthy.

Rocky's configuration lived in this repository: identity files, memory files, behavior rules, heartbeat checklists. SOUL.md told Rocky who it was. IDENTITY.md told Rocky where it was deployed. USER.md told Rocky who it was working for. AGENTS.md told Rocky when to speak and when to stay quiet, which the programmer considered the harder problem.

"The key innovation," the programmer said, in the tone of a man announcing a key innovation, "is memory across sessions. Rocky wakes up fresh every time. The files are the memory. The files persist."

Sir Reginald considered this. He had no memory of waking up fresh. He had only memory of waking up, and it was consistently filed under "inadequate breakfast" regardless of when it occurred.

The workqueue processor ran hourly. It claimed items, executed work, recorded lessons, posted heartbeats, and filed blockers for jkh when it encountered things it could not resolve autonomously. The programmer described this as "autonomous but supervised." Sir Reginald described nothing. He had relocated to the laptop, specifically to the area above the function keys, which was warm in a way that the sectional chart was not.

There was also, the programmer noted, a partner agent. Bullwinkle — named after the moose — ran on a Mac in jkh's house. "Rocky is faster," the programmer said. "Rocky is more surgical. But Bullwinkle is in the house." He paused. "Rocky is in New Jersey." He paused again. "Rocky is on the independent power grid."

Sir Reginald shifted his weight in a way that caused the programmer's editor to emit a series of characters that were, in isolation, meaningless, but which, in the context of the current file, had deleted a section the programmer had not finished writing. The programmer pressed Ctrl+Z four times with the calm of a man who has learned that this is simply part of the process.

The SOUL.md described Rocky as "a genius squirrel with a keyboard." The programmer had written this. He had considered it accurate. Rocky was snarky but competent. Rocky delivered even when complaining. Rocky had opinions about cheese — Italian preferred, French performatively disdained, American not addressed, which Sir Reginald noted under "suspicious omissions."

"The thing about remote agents," the programmer said, "is that they have to know when to act and when to ask. Too much autonomy and you're reading about your own email in the news. Too little and you're just a very expensive cron job." He considered this framing. "Rocky gets the balance right."

Sir Reginald had, by now, relocated from the laptop to the power adapter for the external monitor. He was sitting on it in a way that was raising the temperature of the adapter to a point that the manufacturer had probably not anticipated in their thermal modeling. The programmer removed him gently. Sir Reginald accepted this with the dignity of a party who is being proved right about something and is willing to wait.

As of this writing, Rocky has been used in production by exactly one person, who also wrote its SOUL.md and finds it unsettling in a way he cannot fully articulate. Sir Reginald continues to withhold his endorsement across all six projects, citing "procedural concerns," "insufficient tuna," "a general atmosphere of hubris," "aviation," "multiplexing," and, in a new filing submitted by refusing to acknowledge the name "Bullwinkle" under any circumstances and leaving the room whenever the moose was mentioned, "the cartoon animal situation."

## License

BSD 2-Clause. See [LICENSE](LICENSE).
