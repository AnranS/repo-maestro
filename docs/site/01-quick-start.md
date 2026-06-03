# Quick start (10 minutes)

This is the shortest path for the current workflow: point `maestro` at a folder
of related projects, let it infer the dependency DAG, inspect the generated
prompts, then run when it looks right.

## 0. Prerequisites

- `maestro` binary in `$PATH` (built from this repo with `cargo build --release`)
- At least one agent backend installed: `codex` or `cursor-agent`
- A folder containing one or more local projects

## 1. Pick a workspace

A **workspace** is any directory that contains `.maestro/`. `maestro` operates
relative to the directory where you launch it.

```bash
mkdir -p ~/work/maestro-demo
cd ~/work/maestro-demo
```

You do not need to run `maestro init` separately. `maestro work` initializes the
workspace if needed.

On a new machine, run the guided setup once to seed bundled skills, inspect
provider availability, and run diagnostics:

```bash
maestro setup
maestro demo --run
```

`maestro demo --run` creates a zero-config two-task shell DAG, runs it, and
prints the generated report path. Use plain `maestro demo` when you want to
inspect the dry-run prompts before executing anything.

## 2. Scan, plan, and validate

Point `--root` at the folder containing your projects:

```bash
maestro work "Add JSON output to the calculator CLI" \
  --root ~/work/projects \
  --agent codex
```

This command:

1. creates `.maestro/` if it does not exist,
2. scans `--root` for projects,
3. writes discovered projects and dependencies to `.maestro/projects.yaml`,
4. generates a dependency-ordered `PLAN.yaml`,
5. validates the plan.

By default it stops after validation and prints the next command to run.

## 3. Inspect before running

Use `--dry` to render the exact prompts without calling an agent:

```bash
maestro work "Add JSON output to the calculator CLI" \
  --root ~/work/projects \
  --agent codex \
  --dry
```

The dry-run output points to `.maestro/runs/<dry-run-id>/dry/*.prompt.md`.

## 4. Run the DAG

When the plan and prompts look right:

```bash
maestro work "Add JSON output to the calculator CLI" \
  --root ~/work/projects \
  --agent codex \
  --run
```

You can also run a generated plan manually:

```bash
maestro run plans/2026-05-22-add-json-output.yaml
```

## 5. Open the dashboard

```bash
maestro open
```

This starts the web UI on `127.0.0.1:7777` and opens the dashboard. Use:

```bash
maestro ui
```

when you want the server in the foreground.

The key tabs are:

- **Chat**: confirm `maestro-action` blocks from the orchestrator.
- **Tasks**: watch DAG execution and task logs.
- **Architecture**: inspect project and contract dependency edges.
- **Context**: edit skills and memory used by task prompts.
- **Docs**: built-in EN/ZH reference.

## 6. What changed on disk

After the first `maestro work`, you should see:

```text
.maestro/
├── projects.yaml
├── memory/
├── skills/
└── runs/
```

A real or dry run adds:

- `.maestro/runs/<run-id>/PLAN.yaml`
- `.maestro/runs/<run-id>/RUN_STATE.json`
- `.maestro/runs/<run-id>/REPORT.md`
- `.maestro/runs/<run-id>/logs/<task>.log`
- `.maestro/runs/<dry-run-id>/dry/<task>.prompt.md` for dry runs

## What just happened?

| Step | Component | Output |
|---|---|---|
| You gave one goal | `maestro work` | normalized workflow intent |
| Projects were detected | discovery engine | `.maestro/projects.yaml` |
| Dependencies were ordered | planner | `plans/*.yaml` or chosen `--out` |
| Prompts were prepared | scheduler dry-run / run | `.maestro/runs/<id>/dry/` or logs |
| Tasks ran | Codex / Cursor / shell adapter | `.maestro/runs/<id>/RUN_STATE.json` |
| Results were summarized | reports module | `.maestro/runs/<id>/REPORT.md` |

## Where to go next

- **Understand the generated plan** -> [PLAN.yaml & DAG runs](#docs/plans)
- **Tune project metadata** -> [projects.yaml](#docs/projects-yaml)
- **Control chat actions** -> [maestro-action protocol](#docs/protocols)
- **Use project skills** -> [Skills playbooks](#docs/skills)
