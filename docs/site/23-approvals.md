# Approval gates & cancellation

The DAG runner is **autonomous by default** — once you confirm a plan, it drives every ready task to completion. Two mechanisms exist to stop or pause that loop when you want a human in the seat.

## `requires_approval_after`

A task with `requires_approval_after: true` finishes its work normally, writes its log, and then **pauses the DAG** until a human releases it.

```yaml
tasks:
  - id: T_design_review
    project: _global
    prompt: Review the proposed API in schemas/openapi.yaml and confirm it's ready.
    requires_approval_after: true

  - id: T_impl
    project: login-api
    depends_on: [T_design_review]
    prompt: Implement the route.
```

After `T_design_review` completes:

- Its node turns amber in the dashboard
- A small "pause" badge appears
- Downstream tasks (`T_impl`) stay pending — the scheduler does not advance

To unblock:

```bash
maestro approve T_design_review
```

or click the **approve** button on the task card in the UI. The scheduler picks up the approval marker on its next tick (under 1 second) and fires the downstream tasks.

### When to use it

- After contract changes that other agents will consume
- Before any task that does something destructive (deploy, push, drop a table)
- As a "design review" gate between planning and implementation

If a plan never has any `requires_approval_after`, the whole run is fire-and-forget — useful when you trust the pipeline and just want fast iteration.

## Risk-driven gating (`gate_on_high_risk`)

Where `requires_approval_after` is a per-task switch you set up front, risk-driven gating decides *dynamically* from what the agent actually changed. Set `defaults.gate_on_high_risk: true` in `projects.yaml` and, when a completed task's diff is classified **high-risk** (touches a declared contract, deletes files, or is a large blast radius), maestro pauses it for approval **before its patch integrates** — even if the task never declared `requires_approval_after`. Low-risk changes flow through untouched.

The classified `risk_level` is recorded on the task; the dashboard's task list shows a red **high-risk** chip next to the inline approve/reject controls, so you can scan which changes tripped the gate and release them in place. Release the same way as any gate: `maestro approve <task-id>` or the dashboard button.

This is "oversight scaled to risk" rather than all-or-nothing — pair it with `defaults.routing` (e.g. send contract-critical changes to a stronger model) for risk-aware execution.

## Cancellation

Two ways:

```bash
maestro cancel-run                  # cancels the current run
maestro cancel-run 20260516-082011_d4c95577   # cancels a specific run
```

or click the **cancel** button on the run header in the Tasks tab.

Cancellation works by dropping a marker file under `.maestro/control/cancels/`. The scheduler watches that directory and:

1. Stops dispatching new tasks
2. Sends SIGTERM to any in-flight subprocesses (shell adapter)
3. Marks all unstarted tasks as `cancelled`
4. Writes a final `REPORT.md` with whatever finished

Cancelled runs can be re-run via `maestro rerun --from <id>` to pick up where they left off.

## Time-outs

Every task has a `timeout_minutes` field (defaults differ by `kind`: 15 for agent tasks, 5 for verify). When the timer expires, the task is marked `failed` and downstream tasks skip. There is intentionally no "soft" timeout — if you need polling-style work, model it as `kind: verify` with an explicit retry loop in the shell command.
