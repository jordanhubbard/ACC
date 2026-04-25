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


<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

### Two-Tier Task Convention

CCC uses **two complementary task systems**:

| System | Purpose | Examples |
|--------|---------|---------|
| **Beads (`.beads/`)** | CCC development backlog — code changes, bugs, features, doc updates | `CCC-c49`, `CCC-3so` |
| **CCC fleet queue (`/api/queue`)** | Operational fleet tasks — deployments, migrations, agent health, directives | `wq-API-…` |

**Use Beads when**: the work involves editing files in this repo, fixing a bug in the codebase, or tracking a design decision.

**Use the fleet queue when**: the work requires a running agent (hermes/claude), targets a remote machine, or is an operational action (restart, rotate tokens, deploy script).

A fleet queue task that produces a PR should reference or close the corresponding Beads issue.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
