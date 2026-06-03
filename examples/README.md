# `examples/`

Ready-to-run artifacts for kicking the tires on `maestro` without wiring up
real repos. Each file is referenced by one or more docs pages — run
`maestro doc show <id>` (or open the docs tab in `maestro ui`) for the matching
narrative.

| File | What it is | Run it with | Docs page |
|---|---|---|---|
| `projects.yaml` | A 3-project registry: `api`, `web`, `tool`. Use as a starting `.maestro/projects.yaml`. | `cp examples/projects.yaml .maestro/projects.yaml` | `maestro doc show projects-yaml` |
| `plan-demo.yaml` | Minimal happy-path plan: 1 backend + 1 frontend task, no contracts, no approvals. | `maestro run examples/plan-demo.yaml` | `maestro doc show plans` |
| `plan-approval.yaml` | Shows `requires_approval_after` and how `maestro approve` releases the gate. | `maestro run examples/plan-approval.yaml`, then in another shell: `maestro approve T_design_review` | `maestro doc show approvals` |
| `plan-playground.yaml` | Multi-stage fan-out via `project_each`; useful for testing parallelism + DAG visualization. | `maestro run examples/plan-playground.yaml` | `maestro doc show plans` |

## Quick demo

```bash
# In an empty workspace:
maestro init
cp examples/projects.yaml .maestro/projects.yaml
mkdir -p api web tool
maestro run examples/plan-demo.yaml
maestro open                  # open the dashboard to see the DAG
```

All four files are valid YAML and parse with `maestro plan validate`. They are
covered by the smoke tests under `tests/smoke.rs` — if you change one in
a breaking way, CI will catch it.
