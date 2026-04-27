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

---

## Follow-On Track: Hermes, Single DAG, and Outcome-Centric Scheduling

See `docs/specs/hermes-dag-outcome-scheduling.md` and the follow-on section in `tasks/plan.md`.

## Epic E8 — Hermes DAG Unification
- [ ] T22: move Hermes durable work onto `/api/tasks`
- [ ] T23: reduce `/api/queue` to compatibility ingress only
- [ ] T24: reframe runtime modules as executors and interfaces, not schedulers

## Epic E9 — Outcome Model
- [ ] T25: add `outcome_id` and `workflow_role` to the task model
- [ ] T26: add explicit join and commit-gate tasks to the workflow model
- [ ] T27: add structured outcome result reporting

## Epic E10 — Cooperative Scheduling
- [ ] T28: select and persist one finisher agent per outcome
- [ ] T29: make review and gap closure explicit workflow gates
- [ ] T32: add project/session affinity routing for cooperative outcomes
- [ ] T33: make Hermes a first-class executor class under the same routing model

## Epic E11 — Commit Finalization
- [ ] T30: file commit tasks from reviewed outcome readiness, not only dirty workspace state
- [ ] T31: enforce a single-finisher claim policy for commit tasks

## Epic E12 — Workflow Observability
- [ ] T34: add workflow and outcome visibility to dashboards and APIs
- [ ] T35: write the Hermes-to-DAG cutover runbook

## Recommended First Slice
- [ ] T22: move Hermes durable work onto `/api/tasks`
- [ ] T23: reduce `/api/queue` to compatibility ingress only
- [ ] T25: add `outcome_id` and `workflow_role`
- [ ] T26: add explicit join and commit-gate tasks
- [ ] T28: select and persist one finisher agent per outcome
- [ ] T31: enforce a single-finisher claim policy
- [ ] T30: file commit tasks from reviewed outcome readiness
