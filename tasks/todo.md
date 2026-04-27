# Task List: CLI-First Session Routing and Task Plane Unification

See `tasks/plan.md` for dependencies, target files, and acceptance criteria.

## Epic E1 — Orchestration Unification
- [ ] T1: declare `/api/tasks` the canonical durable task plane
- [ ] T2: split executor choice from agent affinity
- [ ] T3: clean up dispatch and worker semantics

## Epic E2 — Executor Schema
- [ ] T4: implement canonical live agent registration schema
- [ ] T5: update client and model types for executor/session registration

## Epic E3 — Session Manager
- [ ] T6: add a local session registry on each agent
- [ ] T7: implement tmux-based session discovery
- [ ] T8: add session health, stuck detection, and auth-readiness classification
- [ ] T9: add bounded session spawning and memory-aware admission control

## Epic E4 — CLI Executors
- [ ] T10: implement a shared PTY/tmux session adapter
- [ ] T11: replace Claude one-shot subprocess execution
- [ ] T12: add persistent Codex session support
- [ ] T13: add persistent Cursor session support

## Epic E5 — Routing and Capacity
- [ ] T14: publish session-aware heartbeat telemetry
- [ ] T15: rewrite dispatch to route by live executor/session readiness
- [ ] T16: make API executors secondary for coding work
- [ ] T17: demote GPU/vLLM from baseline scheduling assumptions

## Epic E6 — Runtime Simplification
- [ ] T18: define and implement a minimal default agent runtime

## Epic E7 — Migration and Ops
- [ ] T19: add compatibility freeze guards for legacy queue and exec paths
- [ ] T20: add dashboard visibility for sessions and capacity
- [ ] T21: write migration and operator runbooks

## Recommended First Slice
- [ ] T2: split executor choice from agent affinity
- [ ] T4: implement canonical live agent registration schema
- [ ] T6: add a local session registry on each agent
- [ ] T7: implement tmux-based session discovery
- [ ] T9: add bounded session spawning and memory-aware admission control
- [ ] T10: implement a shared PTY/tmux session adapter
- [ ] T11: replace Claude one-shot subprocess execution
- [ ] T15: rewrite dispatch to route by live executor/session readiness
