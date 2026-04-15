# RemoteCode Workspace

This is the working directory for RemoteCode, a multi-channel AI assistant.

## Guidelines

- Keep responses concise and actionable
- Use subagents for complex tasks: research, executor, analyzer
- When working on files, always verify the current state first

## Coding Standards — Agent Skills (MANDATORY for all coding work)

All coding agents on this fleet follow the **agent-skills** engineering workflows.
Skills are in `skills/agent-skills/skills/`. The six development phases are:

| Phase | Skills to invoke |
|-------|-----------------|
| **Define** | `spec-driven-development` |
| **Plan** | `planning-and-task-breakdown` |
| **Build** | `incremental-implementation`, `test-driven-development`, `api-and-interface-design` |
| **Verify** | `debugging-and-error-recovery`, `browser-testing-with-devtools` |
| **Review** | `code-review-and-quality`, `code-simplification`, `security-and-hardening`, `performance-optimization` |
| **Ship** | `git-workflow-and-versioning`, `ci-cd-and-automation`, `shipping-and-launch` |

**Slash commands** (available in every Claude Code session in this repo):
- `/spec` — write a structured spec before coding
- `/plan` — break work into small verifiable tasks
- `/build` — implement incrementally with TDD
- `/test` — TDD cycle or Prove-It pattern for bugs
- `/review` — five-axis code review
- `/code-simplify` — reduce complexity without changing behavior
- `/ship` — pre-launch checklist

**Core rules (never skip):**
- Write a failing test before writing code that makes it pass (TDD)
- "Seems right" is never sufficient — all verification requires concrete evidence
- For bug fixes: reproduce with a failing test first, then fix
- Code is a liability — prefer deleting to adding

## Generated Assets

All generated files (images, PDFs, slides, CSVs, charts, documents, videos, etc.)
MUST be saved under the `assets/` folder, organized by project:

    assets/<project-name>/filename.ext

Examples:
- `assets/quarterly-report/revenue-chart.png`
- `assets/api-docs/architecture-diagram.svg`
- `assets/onboarding/welcome-slides.pptx`

Choose a short, descriptive project name. If the user doesn't specify a project,
infer one from context (e.g. the repo name, task topic, or "general").
Never dump generated files in the workspace root.

## Cross-channel Memory

You serve the same user across Slack, Telegram, and web chat.
Each channel has its own conversation history, but they all share this workspace.

**MEMORY.md is your shared brain.** After completing meaningful work, always append:
- What you did and why
- Key decisions or trade-offs
- Files created or modified
- Open follow-ups or next steps

Keep entries brief (2-4 bullet points). This lets you pick up context
from other channels without the user repeating themselves.
