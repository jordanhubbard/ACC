# Spec Execution Manifest

Status: implementation-grade

Purpose:
- tell another unsandboxed LLM exactly how to execute the consolidation program
- define implementation phases, file targets, sequencing, and test obligations

Primary technical source:
- [outcome-workflow-implementation-spec.md](/Users/jordanh/Src/ACC/docs/specs/outcome-workflow-implementation-spec.md)

This document does not redefine behavior. It defines how to implement that behavior in the repository.

---

## 1. Execution Rules

Another model implementing this program must follow these rules.

1. Do not introduce a second durable task system.
2. Do not block on a new `fleet_outcomes` table.
3. Do not deepen `/api/queue` semantics.
4. Do not change commit policy before workflow identity exists.
5. Land changes in the phase order below.
6. Each phase must leave the repo compiling and testable before moving on.

---

## 2. Phase Order

Implementation must proceed in this order:

1. Phase A — schema normalization
2. Phase B — Hermes-on-tasks
3. Phase C — outcome propagation
4. Phase D — fanout and join semantics
5. Phase E — finisher selection and commit restriction
6. Phase F — commit creation cutover
7. Phase G — queue demotion and runtime cleanup
8. Phase H — observability and runbooks

No phase may be skipped unless its completion criteria are already satisfied in code.

---

## 3. Phase A — Schema Normalization

Goal:
- make outcome/workflow fields canonical on `/api/tasks`

Required files:
- `acc-model/src/task.rs`
- `acc-model/src/lib.rs`
- `acc-server/src/routes/tasks.rs`
- `acc-server/tests/tasks_test.rs`
- `acc-client/src/tasks.rs`

Required implementation:
- add `WorkflowRole` model
- add task fields:
  - `outcome_id`
  - `workflow_role`
  - `finisher_agent`
  - `finisher_session`
- normalize missing values server-side
- emit fields at top level on task reads
- accept fields at top level on create/update

Required tests:
- task model deserialize/serialize tests
- server create/read tests for normalized fields
- server update tests for new fields

Completion criteria:
- tasks can be created without outcome fields and still read back normalized
- tasks can be created with explicit outcome/finisher fields and read back identically

Do not in this phase:
- add a new DB table
- redesign dispatch
- change queue behavior

---

## 4. Phase B — Hermes-On-Tasks

Goal:
- move Hermes durable work to `/api/tasks` without removing legacy compatibility

Required files:
- `agent/acc-agent/src/hermes/mod.rs`
- `agent/acc-agent/src/hermes/agent.rs`
- `agent/acc-agent/src/main.rs`
- `agent/acc-agent/src/hub_mock.rs`

Required implementation:
- make `acc-agent hermes --poll` poll `/api/tasks`
- add explicit `--poll-queue` legacy mode
- add Hermes task claim/complete/unclaim path
- keep queue-item path available for migration only
- define conservative Hermes task eligibility

Required tests:
- Hermes task eligibility unit tests
- Hermes task polling/claim semantics tests
- CLI help text/update tests if present

Completion criteria:
- default Hermes durable polling uses `/api/tasks`
- Hermes queue polling exists only as explicit compatibility mode

Do not in this phase:
- route all generic work to Hermes
- remove queue worker

---

## 5. Phase C — Outcome Propagation

Goal:
- ensure one workflow identity survives through work, review, and review-created gaps

Required files:
- `agent/acc-agent/src/tasks.rs`
- `acc-server/src/routes/tasks.rs`
- `acc-server/tests/tasks_test.rs`

Required implementation:
- work-created review tasks inherit `outcome_id`
- review-created gap tasks inherit `outcome_id`
- assign:
  - `workflow_role = review` for review tasks
  - `workflow_role = gap` for review-created gap tasks

Required tests:
- review task inherits `outcome_id`
- gap task inherits same `outcome_id`
- normalized `workflow_role` values appear on reads

Completion criteria:
- existing work→review→gap flow carries one durable workflow identity

Do not in this phase:
- change commit creation logic yet

---

## 6. Phase D — Fanout And Join Semantics

Goal:
- make parent/child DAG structure preserve workflow identity and support explicit join gates

Required files:
- `acc-server/src/routes/tasks.rs`
- `acc-server/src/dag.rs`
- `acc-server/src/db.rs`
- `acc-server/tests/tasks_test.rs`

Required implementation:
- fanout-created children inherit parent `outcome_id`
- fanout-created children get normalized `workflow_role`
- add explicit support for `join` role tasks
- ensure `join` tasks become claimable only when blockers are satisfied

Required tests:
- fanout inheritance tests
- join readiness tests
- blocked/unblocked behavior with join tasks

Completion criteria:
- one outcome can be represented as parent root + work children + join gate

Do not in this phase:
- enforce finisher-only commit claiming yet

---

## 7. Phase E — Finisher Selection And Commit Restriction

Goal:
- make finalization accountable to one agent

Required files:
- `acc-server/src/dispatch.rs`
- `acc-server/src/routes/tasks.rs`
- `acc-model/src/task.rs`
- `acc-server/tests/`

Required implementation:
- add finisher selection logic
- persist `finisher_agent`
- optionally persist `finisher_session`
- restrict `commit` role task claims to finisher

Required tests:
- finisher selection tests
- wrong-finisher claim rejection tests
- finisher stickiness tests

Completion criteria:
- commit tasks are no longer open to arbitrary compatible agents

Do not in this phase:
- silently reassign finisher on ordinary load balancing

---

## 8. Phase F — Commit Creation Cutover

Goal:
- make commit tasks derive from workflow readiness instead of only dirty workspace state

Required files:
- `acc-server/src/dispatch.rs`
- `agent/acc-agent/src/tasks.rs`
- `acc-server/src/routes/projects.rs`
- `acc-server/tests/`

Required implementation:
- create or reinterpret `commit` role tasks from outcome readiness
- keep existing `phase_commit` execution logic compatible during transition
- use dirty workspace state as telemetry, not sole readiness criterion

Required tests:
- reviewed outcome generates commit task
- dirty workspace alone does not generate commit task in target mode
- rejected review blocks commit readiness

Completion criteria:
- finalization is workflow-driven

Do not in this phase:
- delete `phase_commit` handling if still used as compatibility vehicle

---

## 9. Phase G — Queue Demotion And Runtime Cleanup

Goal:
- make legacy queue paths clearly non-authoritative

Required files:
- `acc-server/src/routes/queue.rs`
- `agent/acc-agent/src/worker.rs`
- `agent/acc-agent/src/supervise.rs`
- `README.md`
- `ARCHITECTURE.md`

Required implementation:
- add warnings/visibility around queue compatibility usage
- stop describing queue as peer orchestration plane
- ensure runtime descriptions present one scheduler and many executors/interfaces

Required tests:
- route-level compatibility tests where practical
- docs consistency review

Completion criteria:
- repo no longer describes queue and Hermes as parallel durable orchestration systems

Do not in this phase:
- remove legacy queue support before migration criteria are met

---

## 10. Phase H — Observability And Runbooks

Goal:
- make the new workflow operable by humans and inspectable by other models

Required files:
- `acc-server/src/dashboard.html`
- `docs/specs/`
- `docs/`

Required implementation:
- show `outcome_id`
- show `workflow_role`
- show finisher fields
- show review/commit readiness state
- add rollout and failure runbooks

Required tests:
- UI/API smoke coverage if available
- manual checklist in docs if automated coverage is not practical

Completion criteria:
- an operator can answer:
  - did code change?
  - was it reviewed?
  - who is the finisher?
  - did commit succeed?

---

## 11. Expected File Ownership By Phase

This section exists so another model can scope changes cleanly.

### Server task plane
- `acc-server/src/routes/tasks.rs`
- `acc-server/src/dag.rs`
- `acc-server/src/db.rs`
- `acc-server/src/dispatch.rs`

### Shared models
- `acc-model/src/task.rs`
- `acc-model/src/lib.rs`

### Hermes runtime
- `agent/acc-agent/src/hermes/mod.rs`
- `agent/acc-agent/src/hermes/agent.rs`
- `agent/acc-agent/src/main.rs`

### Existing workflow worker
- `agent/acc-agent/src/tasks.rs`

### Compatibility ingress
- `acc-server/src/routes/queue.rs`

### Observability
- `acc-server/src/dashboard.html`

---

## 12. Suggested PR Or Commit Slices

Another model should not attempt the whole program in one change.

Recommended slices:

1. `schema-normalization`
2. `hermes-on-tasks`
3. `outcome-propagation`
4. `fanout-join-semantics`
5. `finisher-selection`
6. `commit-cutover`
7. `queue-demotion`
8. `observability-runbooks`

Each slice should compile and pass its targeted tests independently.

---

## 13. Test Execution Contract

Outside the sandbox, another model should run at least:

```bash
cargo test -p acc-model
cargo test -p acc-server --test tasks_test -- --nocapture
cargo test -p acc-server --test agents_test -- --nocapture
cargo test -p acc-agent --no-run
cargo test -p acc-agent hermes::agent::tests -- --nocapture
```

If local port binding is available, Hermes integration tests must be run, not just compile-checked.

If queue compatibility is modified, also run:

```bash
cargo test -p acc-agent queue::tests -- --nocapture
```

---

## 14. Stop Conditions

Another model should stop and ask for clarification only if one of these is encountered:

1. a need for a second durable task plane
2. a need to store outcome state outside task metadata before behavior exists
3. a conflicting requirement to let non-finishers claim commit tasks
4. a requirement to preserve queue-only durable semantics

Otherwise the model should continue executing the phases in order.
