# `projects.yaml`

This file is the **registry** of every project `maestro` knows about. It is the single source of truth for: which directories are reachable, which agent runs in which repo, what contracts they expose, and what memory they should auto-receive.

It lives at `.maestro/projects.yaml` and is created by `maestro init`. Append/remove entries via `maestro add` / `maestro rm`, or hand-edit it.

## Full schema

```yaml
version: 1                        # schema version, currently always 1

defaults:                         # applied to every project unless overridden
  agent: cursor                   # cursor | codex | shell | mock
  agent_model: ""                 # empty = let the selected agent pick
  tagger_model: ""                # cheap model for auto-tagging chat sessions
  model_profile: balanced         # optional fallback chain to use by default
  model_profiles:
    balanced:
      preferred: gpt-5.2
      fallback: [composer-2, composer-2-fast]
  branch_prefix: feat/            # used when maestro creates branches
  max_parallel: 4                 # DAG concurrency cap
  max_total_tasks: 1000           # refuse a run whose plan exceeds this many tasks (0 = unlimited)
  gate_on_high_risk: false        # pause high-risk changes (contract touch / large blast) for approval before integration
  refute_on_high_risk: false      # auto-attach an adversarial `refuter` review to high-risk tasks (no explicit review_by)
  sibling_workspaces:             # also index contract providers from these sibling repos (prefer RELATIVE paths)
    - ../backend-monorepo         # a workspace root with its own .maestro/projects.yaml
  auto_pr: false                  # a verified run auto-pushes its branch + opens a draft PR
  routing:                        # pick agent/model from a task's traits before it runs (first match wins)
    - when: { kind: agent, touches_contract: true }
      model: opus                 # send contract-critical changes to a stronger model
    - when: { kind: verify }
      agent: shell

projects:
  login-api:
    path: ./login-api             # required: relative or ~/absolute
    type: backend                 # backend|frontend|mobile|tool|library|<freeform>
    stack:                        # freeform tags — drives stack templates
      - python
      - fastapi
    agent: cursor                 # override default; "shell" to run raw commands
    agent_model: gpt-5.2          # override per-project model
    model_profile: balanced       # fallback-chain override; wins over agent_model
    memory_scope:                 # which L1 facts to auto-inject
      - api
      - schema
    contracts:                    # cross-project boundaries (see [Contracts])
      provides: schemas/openapi.yaml
      consumes: ""

  login-web:
    path: ./login-web
    type: frontend
    stack: [react, vite, tailwind]
    memory_scope: [ui, design]
    contracts:
      provides: ""
      consumes: schemas/openapi.yaml
```

## Field reference

### `defaults`

| Field | Default | Meaning |
|---|---|---|
| `agent` | `cursor` | Adapter used when a project / task doesn't pick one. |
| `agent_model` | `""` | Adapter-neutral model passed to `cursor` or `codex`. Empty = the selected agent's account/config default. |
| `cursor_model` | unset | Legacy alias for `agent_model`. Still read for old configs, but new writes use `agent_model`. |
| `tagger_model` | `""` | Model used by the chat auto-tagger. Prefer a cheap model. Falls back to `agent_model`. |
| `model_profile` | unset | Named profile from `defaults.model_profiles` to use when no task/run/project profile is set. |
| `model_profiles` | `{}` | Named fallback chains. Each profile has `preferred` plus ordered `fallback` model ids. |
| `branch_prefix` | `feat/` | Used by any agent that creates feature branches. |
| `max_parallel` | `4` | Max simultaneous tasks across the DAG. |
| `max_total_tasks` | `1000` | Hard ceiling on the total tasks a single run may execute (counted after `project_each` expansion). A runaway synthesized plan over this is **refused, not truncated**, with an actionable error. `0` disables the cap. Not applied to `rerun`/`resume`, which recover an already-admitted plan. |
| `gate_on_high_risk` | `false` | When true, a completed task whose change is classified high-risk (touches a contract / large blast radius) pauses for human approval before its patch integrates — even without `requires_approval_after`. See [Approvals]. |
| `refute_on_high_risk` | `false` | When true, a completed high-risk task with no explicit `review_by` gets the builtin `refuter` role auto-attached as an adversarial review — it hunts for missed call sites, un-migrated consumers, broken contracts, and untested edges before the change integrates. A refutation feeds the normal retry/circuit-breaker recovery. Explicit `review_by` always wins. Runs before the `gate_on_high_risk` approval gate. Cost is contained by the default-off + high-risk-only scoping. |
| `sibling_workspaces` | `[]` | Sibling workspace roots (each with its own `.maestro/projects.yaml`) to also index **contract providers** from, so a consumer here can link `consumes` to a producer in another repo. Only sibling projects that already have `contracts.provides` set are indexed (no discovery runs into the sibling). A local provider always wins over a same-named sibling; among siblings the first declared wins. **Prefer relative paths** (resolved against this workspace root) — they stay portable and committable; absolute paths are machine-specific. If no relative path can be computed between consumer and sibling contract, that pair is skipped (never an absolute `consumes`). Empty = single-root behavior. |
| `auto_pr` | `false` | When true, a verified run pushes its integration branch and opens a draft PR (like `maestro pr --push`). Best-effort: a push/PR failure warns, the run still succeeds. |
| `routing` | `[]` | Ordered rules (first match wins) that pick `agent`/`model` from a task's static traits before it runs. `when` matches `kind` (`agent`/`verify`) and/or `touches_contract` (bool). An explicit task agent still wins; routing only overrides the default per-project resolution. |

### `projects.<name>`

| Field | Required | Meaning |
|---|---|---|
| `path` | yes | Directory on disk. Relative paths resolve against the workspace root (the directory holding `.maestro/`). |
| `type` | no | Hint string that drives icons + stack templates. Common: `backend`, `frontend`, `mobile`, `tool`, `library`. |
| `stack` | no | Freeform tag list. Used by skills with `trigger_match` to decide what to inject. |
| `agent` | no | `cursor`, `codex`, `shell`, or `mock`. `shell` runs the task's `command` verbatim, which is useful for verification steps. |
| `agent_model` | no | Per-project model override used after task/run/profile resolution. |
| `cursor_model` | no | Legacy alias for `agent_model`; retained for existing workspaces. |
| `model_profile` | no | Per-project fallback-chain override. Resolved before `agent_model`. |
| `memory_scope` | no | List of subdirectory names under `.maestro/memory/l1_facts/` whose markdown files should be auto-injected into this project's task prompts. |
| `contracts.provides` | no | A single contract file this project authors. Other projects can `consumes` it to declare dependency. |
| `contracts.consumes` | no | A single contract file this project depends on. |

> Today both `provides` and `consumes` accept a single string. Multi-contract support is on the roadmap.

## Resolution order

When a task is dispatched, `maestro` decides which model and agent to use using this hierarchy (highest priority wins):

1. **Task-level** override in `PLAN.yaml` (`task.model`, `task.agent`)
2. **Run-level model** override (`maestro run --model ...`)
3. **Task-level profile** (`task.model_profile`)
4. **Project-level profile** (`projects.<name>.model_profile`)
5. **Role-named profile** (`defaults.model_profiles.<role>`)
6. **Defaults profile** (`defaults.model_profile`)
7. **Project/default model** (`projects.<name>.agent_model`, then `defaults.agent_model`; legacy `cursor_model` is also read)
8. **Adapter default** (whatever the selected agent picks)

Agent selection is simpler: `task.agent`, then `projects.<name>.agent`, then `defaults.agent`.

The same priority applies to memory injection — task-level `memory_inject` always wins over project-level `memory_scope`.

## Repository instructions

For agent tasks, `maestro` also reads `AGENTS.md` files from the workspace root down to the project directory and injects them before the task prompt:

```text
AGENTS.md
apps/AGENTS.md
apps/api/AGENTS.md
```

Use these files for repo-local operating rules: coding style, test commands, review expectations, generated-file warnings, and project conventions. This is intentionally separate from `.maestro/memory/`: memory is topical knowledge, while `AGENTS.md` is the local playbook for how to work in the repo.

## CRUD via CLI

```bash
maestro add ./api --name api --type backend --stack python
maestro ls
maestro rm api
maestro validate
```

`maestro validate` checks paths exist, contracts reference real files (when present), and there are no name collisions.

## Adding from the dashboard

The Architecture tab has **+ add project**. It opens a modal that writes the same `projects.yaml` row you'd get from `maestro add`, plus optional `contracts.provides` / `consumes`. The new project appears in the dependency graph immediately.
