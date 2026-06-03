# `PLAN.yaml` & DAG runs

A **plan** is a declarative DAG of tasks. It's what gets versioned, reviewed, and executed. Plans live under `plans/` in your workspace; the orchestrator agent emits one, you can hand-edit it, and `maestro run` executes it.

## Anatomy

```yaml
spec: Add username/password login across api + web

tasks:
  - id: T1_api_schema
    project: login-api
    agent: cursor            # optional; falls back to project's agent
    model: composer-2-fast   # optional; per-task model override
    prompt: |
      Define POST /login with username/password.
      It must return { token: string } where token is a 1h-TTL JWT.
      Write the schema to schemas/openapi.yaml.

  - id: T2_api_impl
    project: login-api
    depends_on: [T1_api_schema]
    prompt: |
      Implement the route handler. Use bcrypt for password verification.
      Add a happy-path unit test.

  - id: T3_web_form
    project: login-web
    depends_on: [T1_api_schema]   # parallel with T2_api_impl
    prompt: |
      Add /login route with a form. On success store token in localStorage
      under key "auth_token" and redirect to /.

  - id: T4_verify
    project: _global
    agent: shell
    kind: verify
    depends_on: [T2_api_impl, T3_web_form]
    command: |
      cd login-api && pytest -q
      cd ../login-web && pnpm test --run
```

## Task fields

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable, kebab-case identifier. Used in dependency edges, logs, and reports. |
| `project` | yes* | Name from `projects.yaml`. The special value `_global` means "no specific repo." |
| `project_each` | no* | List of project names — task gets fanned out into one concrete task per project (`<id>__<name>`). Mutually exclusive with `project`. |
| `kind` | no | `agent` (default) or `verify`. Verify tasks run a shell `command`, not a prompt. |
| `prompt` | sometimes | Required for `kind: agent`. The instruction sent to the agent. |
| `command` | sometimes | Required for `kind: verify`. Multi-line shell script run via `bash -c`. |
| `agent` | no | `cursor` / `codex` / `shell` / `mock`. Overrides project/default. |
| `model` | no | Agent model override for this task only. Passed through to adapters that support it. |
| `depends_on` | no | List of task ids. Task fires only after all complete with `done`. |
| `outputs` | no | Named workflow outputs captured after success. Each output may snapshot a file with `path`. |
| `inputs` | no | Named workflow inputs from upstream outputs (`from: <task>.<output>`). These automatically add `depends_on` edges. |
| `parallel_group` | no | Free string. Tasks sharing a group will rate-limit themselves via `max_parallel`. |
| `requires_approval_after` | no | When `true`, task pauses after completion until `maestro approve <id>`. |
| `timeout_minutes` | no | Wall-clock timeout. Defaults are sensible per `kind`. |
| `memory_inject` | no | Extra memory entries to inject in addition to the project's `memory_scope`. |
| `skills` | no | Explicit Maestro skills to inject into this agent task. Names resolve project scope first, then `_global`; use `_global/<name>` to disambiguate. Missing explicit skills fail the task before adapter dispatch. |

\* Either `project` or `project_each` must be present.

## Tool permissions

Roles can attach `allowed_tools` (`shell`, `git_write`, `network`, and `allowed_commands`) to a task through the mode system. Enforcement is deliberately explicit:

- `agent: shell` is hard-enforced. If `allowed_commands` is set, Maestro only accepts a single simple argv-style command matching the allowlist and rejects shell composition such as `&&`, `;`, `|`, redirection, and command substitution.
- `agent: codex` always runs in Codex's workspace sandbox. When `network: false`, Maestro also passes Codex's workspace network-off sandbox config. Fine-grained `shell` and `git_write` limits are still prompt policy because Codex CLI does not expose stable per-tool deny flags for them.
- `agent: cursor` omits `--force` and enables Cursor sandbox mode whenever a role is restricted. Fine-grained `shell`, `git_write`, and `network` limits remain prompt policy unless Cursor exposes a stable hard flag for that permission.

The task log records which parts were hard-enforced and which parts were soft provider policy.

## Fan-out with `project_each`

This is the cheapest way to "do the same thing across every repo":

```yaml
tasks:
  - id: T_lint
    project_each: [login-api, login-web, mobile-app]
    agent: shell
    command: |
      pre-commit run --all-files

  - id: T_summary
    project: _global
    depends_on: [T_lint]
    prompt: Summarize the lint results across all repos.
```

At load time `maestro` expands `T_lint` into `T_lint__login-api`, `T_lint__login-web`, `T_lint__mobile-app`. Downstream `depends_on: [T_lint]` is automatically rewritten to depend on **all** three.

## Data dependencies with `inputs` / `outputs`

`depends_on` controls ordering. `inputs` / `outputs` make the data edge explicit too:

```yaml
tasks:
  - id: T_schema
    project: login-api
    skills: [workflow-task-guardrails, contract-first, verify-before-done]
    prompt: Update schemas/openapi.yaml for login.
    outputs:
      openapi:
        path: schemas/openapi.yaml

  - id: T_web
    project: login-web
    prompt: Implement the login form against the provided contract.
    inputs:
      api_contract:
        from: T_schema.openapi
```

The `inputs` reference automatically adds `T_schema` to `T_web.depends_on`. After `T_schema` succeeds, maestro snapshots `schemas/openapi.yaml` under the run directory and injects it into `T_web` as workflow context. This avoids passing critical contracts through a natural-language summary.

## How runs work

1. `maestro run plans/foo.yaml` validates the plan, then creates a fresh directory under `.maestro/runs/<timestamp>_<short-uuid>/`.
2. The scheduler builds a `petgraph` DAG and computes a topological order.
3. Tasks ready to fire are dispatched up to `max_parallel` at a time.
4. Each task gets a log file (`logs/<id>.log`), live-streamed to the dashboard via SSE.
5. As tasks finish, downstream tasks become ready; the loop continues until the DAG is drained.
6. A final `REPORT.md` is written summarizing what was done, with L2 decisions archived per project.

## Live status

While a run is in flight, `RUN_STATE.json` at the top of the run dir is the canonical state. It's overwritten on every transition; the dashboard watches it via `notify` for instant updates. From a terminal:

```bash
maestro status
maestro logs T1_api_schema -f
maestro cancel-run
maestro approve T_design_review
```

## Re-running

```bash
maestro rerun <run-id> --from T2_api_impl
```

This forks the chosen run and re-executes the DAG starting from `T2_api_impl`. Earlier tasks are inherited as already-done. Useful when one task fails and you want to retry just it.

## Static analysis

```bash
maestro plan validate plans/foo.yaml
```

Reports:

- Unknown project names
- Cycles
- Shell syntax errors in `verify` tasks (via `bash -n -c`)
- Unsafe contract races (two tasks writing the same `provides` file in parallel)
- Plan size warnings (> 50 tasks)

`maestro plan validate` is also called automatically before every `maestro run`.
