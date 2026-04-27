# Spec: Outcome Workflow Implementation Contract

Status: implementation-grade

Audience:
- coding agents implementing ACC
- reviewers validating behavior against a precise contract

Scope:
- durable workflow execution on `/api/tasks`
- Hermes integration onto the task plane
- outcome grouping and workflow roles
- single-finisher finalization
- migration compatibility rules

This spec is intentionally narrow and concrete. It is written so another model can implement against it without needing to infer missing behavior from prose elsewhere.

Companion documents:
- spec index: `docs/specs/README.md`
- execution order: `docs/specs/spec-execution-manifest.md`

---

## 1. Canonical Decisions

These are mandatory and override older architectural ambiguity.

1. `/api/tasks` is the only durable orchestration plane.
2. `/api/queue` is compatibility ingress only.
3. `/api/exec` is operator-only and never part of normal durable workflow execution.
4. Every durable workflow is grouped by `outcome_id`.
5. Every durable task has a `workflow_role`.
6. Work and review may be cooperative across agents.
7. Final commit is executed by exactly one finisher agent.

---

## 2. Source Of Truth

### 2.1 Durable state

The durable source of truth is the `fleet_tasks` table exposed through `/api/tasks`.

No implementation may introduce a second durable task store for:
- Hermes work
- review work
- gap work
- join/commit gates

### 2.2 Outcome storage

For the current implementation contract, outcome fields are stored in task metadata and surfaced as top-level API fields.

This means:
- **required now**: metadata-backed outcome fields on tasks
- **optional later**: dedicated `fleet_outcomes` table or materialized view

Another model implementing this spec must not block on a new outcome table.

### 2.3 Queue compatibility

`/api/queue` may continue to exist, but only as a compatibility ingress.

Required behavior:
- queue-originated durable work must be representable as root task creation on `/api/tasks`
- no new durable workflow semantics may be added only to `/api/queue`

---

## 3. Canonical Wire Schema

## 3.1 Task create/update/read fields

All durable tasks must support these top-level fields on create and read:

```json
{
  "id": "task-123",
  "project_id": "proj-123",
  "title": "Implement X",
  "description": "Detailed work",
  "status": "open",
  "priority": 2,
  "task_type": "work",
  "phase": "build",
  "review_of": null,
  "blocked_by": [],

  "preferred_executor": "claude_cli",
  "required_executors": ["claude_cli"],
  "preferred_agent": "natasha",
  "assigned_agent": null,
  "assigned_session": null,

  "outcome_id": "outcome-123",
  "workflow_role": "work",
  "finisher_agent": null,
  "finisher_session": null,

  "review_result": null,
  "metadata": {}
}
```

### 3.1.1 Enum values

`task_type` currently allowed:
- `work`
- `review`
- `idea`
- `discovery`
- `phase_commit`
- `feature`
- `bug`
- `epic`
- `task`

`workflow_role` allowed:
- `root`
- `work`
- `review`
- `gap`
- `join`
- `commit`

Unknown `workflow_role` values must deserialize as `unknown` internally but must not be emitted by server code.

### 3.1.2 Required fields

Required on task create:
- `project_id`
- `title`

Required by workflow contract after normalization:
- `outcome_id`
- `workflow_role`

If absent on create, server normalization must fill them.

### 3.1.3 Normalization rules

When a task is created:

1. If `outcome_id` is absent and the task is not a child of another task, set `outcome_id = task.id`.
2. If `outcome_id` is absent and the task is fanout-created from a parent task, inherit the parent `outcome_id`.
3. If `workflow_role` is absent:
   - `review` task type -> `workflow_role = "review"`
   - `phase_commit` task type -> `workflow_role = "commit"`
   - everything else -> `workflow_role = "work"`

These normalized values must be persisted in task metadata and emitted as top-level fields on read.

### 3.1.4 Metadata compatibility

During migration, these fields are canonical even if physically stored in `metadata`:
- `preferred_executor`
- `required_executors`
- `preferred_agent`
- `assigned_agent`
- `assigned_session`
- `outcome_id`
- `workflow_role`
- `finisher_agent`
- `finisher_session`

Server behavior must always:
- read them from metadata
- emit them as top-level fields
- accept them as top-level write fields

---

## 4. Workflow Shape

Every durable workflow must be expressible as this DAG shape:

```text
root
  -> work*
      -> review*
          -> gap*
  -> join
      -> commit
```

Interpretation:
- `root` represents the durable requested result
- `work` tasks perform code changes
- `review` tasks validate completed work
- `gap` tasks represent blocking fixes discovered during review
- `join` becomes eligible only when required work/review/gap blockers are satisfied
- `commit` finalizes the outcome

### 4.1 Required roles

For a workflow to be considered complete:
- it must have at least one `work` task
- it must have at least one `review` path for each required work output
- it must have exactly one `commit` task

### 4.2 Optional roles

`gap` tasks are optional and appear only when review creates them.

`join` tasks are optional only during migration. Final target behavior requires them.

---

## 5. Role Semantics

## 5.1 `root`

Meaning:
- durable requested result

Rules:
- may be fanout-transformed into child work tasks
- must carry the canonical `outcome_id`
- should be the natural place to persist finisher fields once chosen

## 5.2 `work`

Meaning:
- code-changing implementation work

Rules:
- may modify project workspace
- may be routed to CLI executors, Hermes, or API executors according to executor policy
- completion alone does not make the outcome ready to commit

## 5.3 `review`

Meaning:
- independent validation of completed work

Rules:
- should be assigned to a different agent when peers exist
- may approve or reject
- may create blocking `gap` tasks
- writes `review_result` on the reviewed work task

## 5.4 `gap`

Meaning:
- blocking follow-up work created by review

Rules:
- belongs to the same `outcome_id`
- blocks finalization until completed and approved if marked blocking

Required current simplification:
- all review-created gap tasks are blocking unless explicitly extended later

## 5.5 `join`

Meaning:
- workflow readiness gate

Rules:
- does no code-changing work
- becomes claimable only when all blockers are satisfied
- should typically be handled by the server or a lightweight agent path

## 5.6 `commit`

Meaning:
- single-finisher finalization task

Rules:
- exactly one per outcome
- claimable only by finisher
- performs git commit/push/final merge policy
- writes final structured result

---

## 6. Finisher Contract

## 6.1 Finisher fields

Canonical fields:

```json
{
  "finisher_agent": "bullwinkle",
  "finisher_session": "claude-proj-foo"
}
```

`finisher_agent` is required once commit finalization is planned.

`finisher_session` is optional and used when session affinity exists.

## 6.2 Finisher selection algorithm

When finisher is first selected, the dispatcher must apply this order:

1. existing healthy project-affine session already bound to the project
2. agent that has completed the most `work` tasks in the same `outcome_id`
3. agent with healthy preferred executor and free session capacity
4. least-loaded compatible online agent

Tie-break:
- alphabetical agent name

## 6.3 Finisher persistence

Once set, `finisher_agent` must remain unchanged unless one of these is true:
- agent is offline for more than 300 seconds
- assigned session is `dead`
- assigned session is `unauthenticated`
- manual reassignment is requested

No implementation may opportunistically change finisher just because a lower-load agent appears.

## 6.4 Commit claim restriction

A `commit` role task must only be claimable by:
- `finisher_agent`

If `finisher_session` is set, dispatch should prefer that session, but claim ownership still belongs to the finisher agent.

If a non-finisher attempts to claim a `commit` task:
- server must reject with conflict-like semantics

Recommended HTTP behavior:
- `409 Conflict` with `{"error":"wrong_finisher"}`

---

## 7. Review Contract

## 7.1 Required review behavior

For each `work` task that contributes to a commit-bound outcome:
- at least one `review` task must exist

Current minimum rule:
- one review task per completed work task

## 7.2 Reviewer selection rule

When peers are available:
- reviewer must not equal the work task completer

When no peer is available:
- self-review is allowed only in degraded mode
- degraded mode must be visible in metadata

Recommended metadata:

```json
{
  "review_mode": "self_review_fallback"
}
```

## 7.3 Review results

Allowed review verdicts:
- `approved`
- `rejected`

Effects:
- `approved` may unblock downstream tasks
- `rejected` prevents finalization until blocking gaps are closed and reviewed

## 7.4 Gap creation contract

A review may create zero or more `gap` tasks.

Each gap task must:
- inherit the same `outcome_id`
- use `workflow_role = "gap"`
- persist the parent review task id in metadata

Recommended metadata:

```json
{
  "spawned_by_review": "task-review-123"
}
```

---

## 8. Commit Contract

## 8.1 Commit creation rule

Final target behavior:
- create `commit` tasks from outcome readiness, not only from dirty workspace detection

Migration behavior:
- current dirty-bit `phase_commit` auto-file behavior may continue
- but all new code must move toward join-gated commit filing

## 8.2 Commit readiness

A `commit` task is ready only when:
- all required `work` tasks in the outcome are completed
- all required `review` tasks are approved
- all blocking `gap` tasks are completed
- all blocking gap reviews are approved if gap review is enabled
- the `join` task for the outcome is satisfied

## 8.3 Commit execution policy

Current implementation-compatible policy:
1. checkout/create `phase/<phase>` branch
2. `git add -A`
3. `git commit -m <message>`
4. `git pull --rebase origin <branch>` best effort
5. `git push origin <branch>`
6. attempt fast-forward of `main`

Required behavior:
- non-fast-forward merge to `main` is not automatic
- push failure completes commit task with failure summary rather than inventing more durable work

## 8.4 Commit result fields

When commit finishes, the outcome must be derivable from structured task output and metadata fields:

```json
{
  "code_changed_successfully": true,
  "reviewed_successfully": true,
  "committed_successfully": true,
  "commit_sha": "abc123",
  "branch": "phase/milestone"
}
```

If exact commit SHA is unavailable in the first cut, set:
- `commit_sha = null`

Do not fake a SHA.

---

## 9. Hermes Contract

## 9.1 Durable Hermes work

Hermes durable work must be fetched from `/api/tasks`.

Default Hermes polling mode:
- `--poll` means `/api/tasks`

Legacy mode:
- `--poll-queue` means `/api/queue`

## 9.2 Hermes task eligibility

Hermes must consider a durable task eligible only if all of these are true:

1. `status == open`
2. `task_type` is work-like (`work`, `feature`, `bug`, `task`)
3. `review_of == null`
4. `assigned_agent` is absent or equals current agent
5. `preferred_agent` is absent or equals current agent
6. `preferred_executor` is absent or not one of:
   - `claude_cli`
   - `codex_cli`
   - `cursor_cli`
   - `opencode`
7. if `required_executors` is non-empty, it must contain `hermes` or `llm`

This rule is intentionally conservative so Hermes does not race generic CLI coding tasks.

## 9.3 Hermes executor identity

Hermes must register as an executor capability using at least:
- `hermes`
- optionally `llm`

No implementation should require a separate queue-specific contract for Hermes after migration.

---

## 10. Routing Contract

## 10.1 Work routing

For `work` role tasks:
- honor `required_executors` as hard filter
- honor `preferred_executor` as soft preference
- honor `preferred_agent` as soft node affinity
- honor `assigned_agent` as hard assignment if set

## 10.2 Review routing

For `review` role tasks:
- prefer non-author peer
- if peer unavailable, allow degraded self-review
- do not route to offline finisher merely because finisher worked on the outcome

## 10.3 Commit routing

For `commit` role tasks:
- route only to `finisher_agent`
- prefer `finisher_session` if healthy
- if finisher unavailable, do not silently hand commit to another agent
- instead either:
  - leave task open and unclaimable
  - or trigger explicit finisher reassignment logic

## 10.4 Session affinity

If `assigned_session` or `finisher_session` is set and healthy:
- dispatch should prefer that session over launching a new one

If session is unhealthy:
- dispatch may fall back to same-agent different session
- dispatch may not change finisher unless finisher reassignment conditions are met

---

## 11. State Machine

This is the required conceptual state machine for one outcome.

```text
root_created
  -> work_open
  -> work_claimed
  -> work_completed
  -> review_open
  -> review_claimed
  -> review_approved | review_rejected

review_rejected
  -> gap_open
  -> gap_claimed
  -> gap_completed
  -> review_open

review_approved
  -> join_ready
  -> commit_open
  -> commit_claimed
  -> committed_successfully | commit_failed
```

Notes:
- `commit_failed` is terminal for the task, not necessarily for the outcome
- recovery from `commit_failed` is operational, not automatic durable fanout

---

## 12. Migration Contract

## 12.1 Mixed-version compatibility

During migration, old and new agents may coexist.

Server requirements:
- always accept old clients that do not send outcome fields
- normalize missing outcome fields on create/read
- never require a new outcome table for the API to function

Agent requirements:
- tolerate tasks with missing top-level outcome fields if metadata contains them
- tolerate tasks with neither field by assuming:
  - `outcome_id = task.id`
  - `workflow_role = default from task_type`

## 12.2 Queue compatibility

Allowed during migration:
- Hermes may still support `--poll-queue`
- queue items may still be processed by legacy worker loops

Not allowed:
- introducing new durable workflow-only semantics that exist only on queue items

## 12.3 Phase commit compatibility

Allowed during migration:
- existing `phase_commit` task type remains valid
- existing dirty-bit auto-filing may remain active

Required direction:
- new join-gated commit behavior must map onto `phase_commit` or future `commit` role tasks
- no new logic should deepen dirty-bit-only coupling

---

## 13. API Error Contract

Recommended server errors for workflow-specific cases:

### 13.1 Wrong finisher

```json
{
  "error": "wrong_finisher",
  "message": "task may only be claimed by the selected finisher"
}
```

Suggested status:
- `409`

### 13.2 Outcome not ready

```json
{
  "error": "outcome_not_ready",
  "message": "workflow blockers are not satisfied"
}
```

Suggested status:
- `423`

### 13.3 Reassignment required

```json
{
  "error": "finisher_unavailable",
  "message": "selected finisher is unavailable and reassignment is required"
}
```

Suggested status:
- `409`

---

## 14. Acceptance Test Matrix

Another model implementing this spec should add tests covering all of these.

### 14.1 Schema tests

- create task without `outcome_id` -> server emits `outcome_id == task.id`
- create `review` task without `workflow_role` -> emits `workflow_role == review`
- create `phase_commit` task without `workflow_role` -> emits `workflow_role == commit`
- update task with finisher fields -> emitted on subsequent read

### 14.2 Fanout tests

- parent with `outcome_id` fanout-creates children -> all children inherit same `outcome_id`
- child-specific metadata does not erase inherited `outcome_id`

### 14.3 Hermes tests

- Hermes polling ignores CLI-pinned tasks
- Hermes polling accepts Hermes-eligible tasks
- `--poll` uses `/api/tasks`
- `--poll-queue` uses `/api/queue`

### 14.4 Review tests

- work completion creates review task with same `outcome_id`
- review-created gap task has same `outcome_id`
- rejected review blocks commit readiness

### 14.5 Commit tests

- commit task with finisher set cannot be claimed by other agent
- commit task can be claimed by finisher
- commit task is not created solely from dirty bit in final target mode

### 14.6 Migration tests

- old create-task clients still work
- tasks missing top-level outcome fields still deserialize and normalize
- mixed old/new agents do not break claim flow

---

## 15. Implementation Order

If another model is executing this spec, implement in this order:

1. schema normalization on `/api/tasks`
2. Hermes `/api/tasks` polling
3. outcome propagation through work/review/gap
4. fanout inheritance
5. finisher fields and commit claim restriction
6. join-gated commit creation
7. queue ingress demotion

Do not start with a new outcomes table.

---

## 16. Non-Goals

This spec does not require:
- deleting `/api/queue` immediately
- introducing a new database table before workflow behavior exists
- redesigning chat gateway behavior
- making commit retries fully automatic

It does require that all new durable orchestration behavior be implemented on `/api/tasks` and grouped by `outcome_id`.
