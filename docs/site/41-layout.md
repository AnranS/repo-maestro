# On-disk layout (`.maestro/`)

`maestro` keeps every piece of state under `.maestro/` in your workspace. Nothing is written outside the workspace except the optional skill mirrors in `.cursor/rules/` and `.claude/skills/`.

```text
your-workspace/
├── AGENTS.md                     # optional workspace-wide agent instructions
├── apps/
│   └── api/
│       └── AGENTS.md             # optional project/nested instructions
├── .maestro/
│   ├── projects.yaml             # project registry (see [projects.yaml])
│   ├── cursor_models.json        # Cursor model cache
│   ├── <provider>_models.json    # optional provider model caches
│   │
│   ├── memory/
│   │   ├── l1_facts/             # hand-curated topic-scoped notes
│   │   │   └── <topic>/<id>.md
│   │   └── l2_decisions/         # auto-archived run summaries
│   │       └── <project>/<run-id>-<slug>.md
│   │
│   ├── skills/                   # markdown playbooks
│   │   ├── _global/<id>.md
│   │   └── <project>/<id>.md
│   │
│   ├── proposals/                # failure-driven guardrail proposals (NOT gitignored)
│   │   └── <fingerprint>.md      # review with `maestro learn`
│   │
│   ├── chat/
│   │   ├── sessions/             # session metadata + Cursor chat ids
│   │   │   └── <session-id>.json
│   │   └── continuations/         # generated `maestro chat brief` handoffs
│   │
│   ├── mailbox/
│   │   └── messages/             # cross-role / cross-project handoff notes
│   │       └── <message-id>.json
│   │
│   ├── runs/                     # one directory per `maestro run`
│   │   └── <ts>_<short-uuid>/
│   │       ├── PLAN.yaml         # snapshot of what ran
│   │       ├── RUN_STATE.json    # live + final state
│   │       ├── REPORT.md         # human-readable summary
│   │       └── logs/<task-id>.log
│   │
│   └── control/
│       ├── approvals/            # touch a file here to release a paused task
│       └── cancels/              # touch a file here to cancel a run
│
├── plans/                        # PLAN.yaml drafts (versioned with the project)
│   └── 20260516-add-login.yaml
│
└── (your projects)
    ├── login-api/
    ├── login-web/
    └── ...
```

## Why these locations?

| Path | Why it lives there |
|---|---|
| `.maestro/projects.yaml` | Workspace-local registry; should never be checked in to a project repo. Recommended: commit it in a tiny "workspace meta" repo, or `.gitignore` it. |
| `.maestro/runs/` | Generated, big — always `.gitignore`'d. |
| `plans/` | First-class artifacts; commit these alongside the actual code change so future maintainers can see "what plan ran when." |
| `.maestro/memory/l1_facts/` | Hand-curated; either commit (recommended) or sync via your team knowledge base. |
| `.maestro/memory/l2_decisions/` | Auto-generated; commit to capture audit trail, or `.gitignore` if you want a clean history. |
| `.maestro/proposals/` | Failure-driven guardrail proposals (opt-in). The one part of `.maestro/` that is **not** gitignored, so they can be reviewed/committed as a diff. Promote with `maestro learn`. |
| `.maestro/skills/` | Hand-curated; commit and share across the team. |
| `.maestro/mailbox/messages/` | Short-lived local handoffs between roles/projects. Commit only if you want the team to see the coordination trail. |
| `AGENTS.md` | Repo-local operating rules. Agent tasks receive every `AGENTS.md` from the workspace root down to the project directory. |

## Control plane files

The `control/` directory is how the CLI signals the running scheduler without IPC. The scheduler watches it via `notify::recommended_watcher`. Markers are just empty files whose **name** is the signal:

- `control/approvals/<task-id>` — releases that task
- `control/cancels/<run-id>` (or `current`) — cancels that run

This is intentional — it means any other tool (or even `touch` from a shell script) can trigger a release/cancel without speaking HTTP.

## Atomicity

`maestro` uses temp-file + rename for every state write:

- `RUN_STATE.json.tmp` → `mv` → `RUN_STATE.json`
- `projects.yaml.tmp` → `mv` → `projects.yaml`
- Skill / memory files are written via the same pattern

This makes the on-disk state safe against interrupts — at worst you lose the latest write, never end up with a half-written file.

## Cleanup

Old runs grow over time. There's no auto-pruning yet; manually:

```bash
# Keep only the last 20 runs
ls -1t .maestro/runs | tail -n +21 | xargs -I{} rm -rf .maestro/runs/{}
```

Likely will land as `maestro runs --prune --keep 20` in a future version.
