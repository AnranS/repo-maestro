# `maestro-action` protocol

Already overviewed in [`maestro-action` blocks](#docs/actions); this page is the small wire-level reference.

## Block format

````
```maestro-action
verb: <verb>
description: optional human label
<key>: <value>
```
````

Rules:

- The info string must be **exactly** `maestro-action`.
- Body must be parseable as YAML.
- A block with an unknown `verb` is ignored by the executor.
- Multiple blocks per message are allowed and rendered in order.

## Verb catalog

### `work`

Use this first. It can initialize the workspace, scan a project folder, register discovered projects, synthesize a dependency-ordered plan, and validate it.

```yaml
verb: work
description: Scan, plan, and validate JSON output
spec: Add JSON output to the CLI
root: ~/work/monorepo   # optional
agent: codex            # optional
out: plans/json.yaml    # optional
run: true               # optional
dry: true               # optional
```

`spec` is required. `root` is the folder to scan for projects. Use `dry: true` when you want to inspect generated prompts without calling agents.

### `run`

```yaml
verb: run
description: Run the proposed login plan
plan: plans/20260516-login.yaml
plan_hash: fnv1a64:8b7e1d2a4f0c9a31  # optional stale-plan guard
```

`plan` is required, relative to the workspace root. `plan_hash` is optional; generate it with `maestro plan hash <path>`. If present, the executor checks the current file hash before running.

### `rerun`

```yaml
verb: rerun
plan: 20260516-082011_d4c95577
from: T2_api_impl       # optional
```

### `approve`

```yaml
verb: approve
task: T_design_review
```

### `status`

```yaml
verb: status
```

### `plan_validate`

```yaml
verb: plan_validate
plan: plans/20260516-login.yaml
plan_hash: fnv1a64:8b7e1d2a4f0c9a31  # optional
```

## Execution semantics

- An action is only executed after the user clicks **run** in the UI (or `maestro action run <chat-id> <action-idx>` in the terminal).
- Each verb maps onto an existing CLI subcommand; the execution is identical to running that subcommand by hand.
- stdout/stderr are streamed back to the chat UI and shown in the action card's output pane.
- Exit codes propagate: non-zero → red card, zero → green card.
- A failed action never silently advances anything else; downstream agent reasoning sees the failure.

## Why not real function calling?

Cursor's chat completion API exposes function calling on some models but not all. Action blocks work uniformly across every model your account supports, can be paginated/inspected in markdown, and are trivially editable by hand. That's worth the small cost of YAML parsing.
