# Spec: Hermes, Single DAG, and Outcome-Centric Cooperative Scheduling

Spec index:
- `docs/specs/README.md`

Implementation contract:
- `docs/specs/outcome-workflow-implementation-spec.md`
- `docs/specs/spec-execution-manifest.md`

## Objective

ACC now has three strong first-class mechanisms:
- a durable task DAG in `/api/tasks`
- Hermes as a real agent runtime
- generalized agent registration with executor/session metadata

The next consolidation step is to make those mechanisms describe one coherent system instead of several adjacent ones.

This spec defines four decisions:

1. **Hermes moves onto `/api/tasks`**
2. **The task DAG becomes the only orchestration model**
3. **Cooperative work is scheduled around outcomes, not just task types**
4. **Each outcome is finalized by one explicit finisher agent**

The target behavior is straightforward:
- code is changed successfully
- code is reviewed successfully
- code is ultimately committed successfully by one accountable agent

---

## Problem

The codebase now has the right primitives, but they are still split across multiple control paths.

The current task subsystem already supports:
- durable tasks
- dependency edges
- cycle detection
- atomic claiming
- fanout
- unblocking on completion/review approval

The current agent runtime also already supports:
- work execution
- review execution
- phase commits
- Hermes as a separate runtime

What remains incoherent is that these are not yet one model.

Current contradictions:
- Hermes still polls the legacy queue instead of `/api/tasks`
- `/api/queue` and `/api/exec` still coexist as peer orchestration planes
- work, review, and `phase_commit` are implemented as separate polling lanes instead of an explicit workflow graph
- `phase_commit` is auto-filed from a dirty workspace bit, not from a reviewed outcome join gate
- final commit ownership is implicit rather than assigned

---

## Core Decisions

### D1 — Hermes runs on the task DAG

Hermes must stop being a separate queue-driven orchestration path.

Hermes becomes:
- an executor class
- an agent implementation
- an optional gateway/chat surface

Hermes must not remain a separate durable work plane.

Rules:
- Hermes durable work is fetched from `/api/tasks`
- Hermes may still handle chat/gateway traffic outside the task DAG
- Hermes queue polling is legacy-only until removed

### D2 — The DAG is the single orchestration model

All durable multi-agent work must be represented as tasks and dependency edges.

That includes:
- coding work
- review work
- review-filed gap work
- join gates
- final commit work

`/api/queue` may remain only as an ingress shim that creates task DAG nodes.

`/api/exec` remains operator-only and is never part of normal work scheduling.

### D3 — Outcomes become first-class

The system currently tracks tasks well, but its real unit of success is an outcome:
- a requested code change
- the review and rework needed to make it acceptable
- the final commit that lands it

Introduce an explicit grouping key:

```json
{
  "outcome_id": "outcome-abc123",
  "workflow_role": "work"
}
```

`outcome_id` groups all tasks that contribute to the same durable result.

Suggested `workflow_role` values:
- `root`
- `work`
- `review`
- `gap`
- `join`
- `commit`

### D4 — Cooperative scheduling is multi-agent, finalization is single-agent

Many agents may cooperate on an outcome:
- work may fan out
- review may be assigned to peers
- review may create new gap tasks

But only one finisher agent owns final commit execution for a given outcome.

That finisher should be explicit:

```json
{
  "outcome_id": "outcome-abc123",
  "finisher_agent": "bullwinkle",
  "finisher_session": "claude-proj-foo"
}
```

This creates one accountable owner for:
- final git commit
- final push
- final merge attempt
- final workflow result publication

### D5 — Success is expressed as structured results

The system should stop inferring end-state primarily from logs and task types.

Each outcome should have explicit result fields such as:

```json
{
  "status": "committed_successfully",
  "code_changed_successfully": true,
  "reviewed_successfully": true,
  "committed_successfully": true,
  "commit_sha": "abc123...",
  "branch": "phase/milestone",
  "review_summary": {
    "approved": 3,
    "rejected": 0,
    "gaps_opened": 1,
    "gaps_closed": 1
  }
}
```

---

## Canonical Workflow Shape

### Root outcome

A user request or imported queue item becomes a root task plus an `outcome_id`.

The root task represents the requested durable result, not the first worker step.

### Work fanout

The root may fan out into one or more `work` tasks.

These tasks may:
- run on different agents
- prefer different executors
- share one project workspace

### Review stage

Each completed work task must be reviewed.

Reviews may:
- approve
- reject
- file one or more gap tasks

Rejected work or open required gaps keep the outcome from reaching the join gate.

### Join gate

A join task becomes claimable only when:
- all required work tasks completed
- all required review tasks are approved
- all required gap tasks are completed and approved

The join task has no executor responsibility beyond declaring the outcome ready for finalization.

### Commit task

Exactly one `commit` task is blocked by the join gate.

The commit task:
- is assigned to one finisher agent or session
- commits reviewed project state
- pushes the result
- optionally fast-forwards or merges according to policy
- records the final structured outcome result

---

## Data Model

### Task fields

Normalize the following task-level fields:

```json
{
  "outcome_id": "outcome-abc123",
  "workflow_role": "work",
  "finisher_agent": null,
  "finisher_session": null,
  "required_reviews": 1,
  "requires_review_approval": true
}
```

Rules:
- every durable task belongs to an `outcome_id`
- `workflow_role` replaces implicit behavior encoded only in `task_type`
- `task_type` may remain for compatibility, but role should drive workflow semantics
- `finisher_agent` and `finisher_session` are meaningful only for root/join/commit coordination

### Outcome record

Add a first-class outcome record or materialized view:

```json
{
  "id": "outcome-abc123",
  "project_id": "proj-123",
  "root_task_id": "task-root",
  "status": "in_review",
  "finisher_agent": "bullwinkle",
  "finisher_session": "claude-proj-foo",
  "code_changed_successfully": true,
  "reviewed_successfully": false,
  "committed_successfully": false,
  "commit_task_id": "task-commit",
  "commit_sha": null
}
```

This may be stored in a dedicated table or derived from task metadata during migration.

---

## Scheduling Rules

### Cooperative work

Dispatch may spread `work` and `review` tasks across multiple agents.

Primary routing signals:
- executor readiness
- free session slots
- project/session affinity
- current task load
- explicit `preferred_agent`

### Finisher selection

The finisher should be chosen once per outcome, ideally when the first work task is claimed or when the root task is expanded.

Selection priorities:
1. existing project-affine session
2. agent already executing most work in the outcome
3. agent with ready coding executor and free session capacity
4. least-loaded compatible agent

Once chosen, the finisher should be sticky unless:
- the agent goes offline
- the session dies and cannot be recovered
- the outcome is manually reassigned

### Review independence

The finisher must not review its own final commit path by default.

At least one reviewer should be a different agent when peers are available.

If no peer exists, self-review may be allowed only as a degraded mode and should be marked explicitly in the outcome record.

### Commit gating

The commit task is not filed merely because a workspace is dirty.

It is filed because an outcome is ready:
- reviewed
- approved
- gap-closed
- joined

Workspace dirtiness remains useful telemetry, but not the primary readiness signal for finalization.

---

## Hermes Integration

### Hermes as executor

Hermes must be routable via the same executor model used for:
- `claude_cli`
- `codex_cli`
- `cursor_cli`
- API executors

That means Hermes work should be a task assignment choice, not a separate queue subsystem.

### Hermes as gateway

Gateway/chat usage may still sit outside durable tasks.

When a gateway interaction becomes durable work, it must create:
- a root task
- an `outcome_id`
- downstream DAG nodes as needed

### Hermes migration rule

During migration:
- Hermes may continue to read `/api/queue`
- but queue items must be considered compatibility-only
- new durable workflow features must not be added to the queue path

---

## Runtime Simplification

The default agent runtime should converge toward:
- task poller
- bus listener
- session manager
- executor adapters, including Hermes

Optional surfaces:
- Slack/Telegram gateway
- operator proxy
- legacy queue bridge during migration

The important shift is conceptual:
- workers stop being orchestration systems
- workers become executor or interface modules under one scheduler

---

## Migration Plan

### Phase 1 — Schema and compatibility

- add `outcome_id`, `workflow_role`, `finisher_agent`, `finisher_session`
- allow these to live in metadata first if needed
- expose them on `/api/tasks`

### Phase 2 — Hermes-on-tasks

- add Hermes task polling against `/api/tasks`
- keep queue polling as compatibility mode only
- stop adding new queue-only semantics

### Phase 3 — Explicit outcome DAGs

- create root/fanout/review/join/commit workflow shapes
- file commit tasks from join gates, not only dirty-bit scanning

### Phase 4 — Single-finisher routing

- choose and persist finisher affinity
- route commit tasks only to the selected finisher

### Phase 5 — Retire queue as an orchestration plane

- keep `/api/queue` only as import shim or remove it entirely
- remove Hermes queue polling once task parity is complete

---

## Acceptance Criteria

A coherent implementation of this spec means:

- Hermes durable work runs on `/api/tasks`, not only `/api/queue`
- every durable multi-agent workflow can be expressed as one DAG
- work, review, and commit tasks share an `outcome_id`
- final commit is executed by one explicit finisher agent
- commit readiness is based on reviewed workflow state, not only dirty workspace state
- dashboards and APIs can answer:
  - did code change successfully?
  - did review complete successfully?
  - did one agent commit the result successfully?

---

## Non-Goals

This spec does not require:
- removal of all chat/gateway flows
- immediate deletion of `/api/queue`
- a full replacement of current `task_type` semantics on day one

It does require that all new durable orchestration move toward one model rather than adding another.
