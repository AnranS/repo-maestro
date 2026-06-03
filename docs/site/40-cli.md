# CLI reference

Everything `maestro <subcommand> --help` will tell you, organized by topic.

## Workspace

```bash
maestro setup                         # guided first-run setup
maestro init                          # create .maestro/ in cwd
maestro add <path> [opts]             # register a project
maestro work "<goal>" --root <dir>     # scan/register/plan/validate in one command
maestro ls                            # list registered projects
maestro rm <name>                     # remove a project
maestro validate                      # check projects.yaml consistency
```

`setup` is the recommended first command on a new machine or workspace. It
creates `.maestro/`, installs or refreshes bundled skills without overwriting
local edits, summarizes known agent providers, optionally refreshes models with
`--refresh-models`, and runs `doctor` unless `--skip-doctor` is set.

### `maestro add` flags

| Flag | Meaning |
|---|---|
| `--name <id>` | logical name (defaults to dir basename) |
| `--type <t>` | type tag (backend / frontend / mobile / tool / library / …) |
| `--stack <a,b,c>` | comma-separated stack tags |
| `--agent <a>` | per-project agent (`cursor` / `codex` / `shell` / `mock`) |

To set contracts or `memory_scope`, hand-edit `.maestro/projects.yaml` afterwards.

### `maestro work`

```bash
maestro work "Add JSON output to the CLI" --root ~/work/monorepo --agent codex
maestro work "Add JSON output to the CLI" --project api,web --run
maestro work "Add JSON output to the CLI" --root ~/work/monorepo --dry
```

`work` is the short path for day-to-day use. It initializes `.maestro/` if
needed, optionally scans `--root` and applies discovered projects, synthesizes a
dependency-ordered workflow plan, then validates it. Add `--run` to dispatch the
plan immediately, or `--dry` to render the full prompts without calling agents.

## Plans

```bash
maestro plan validate <path>          # static analysis
maestro plan hash <path>              # hash for guarded maestro-action blocks
maestro run <path> [--model <id>]     # run a PLAN.yaml
maestro rerun <run-id> [--from <task>]  # fork a run
maestro resume [--run <id>] [--force]   # continue an interrupted run (skips done tasks)
maestro compare "<prompt>" --project <p> --agents codex,cursor[,mock]
                                       # run one task on several agents, pick the best (voting)
maestro approve <task-id>             # release a paused task
maestro cancel-run [<run-id>]         # cancel current or named run
maestro status                        # current run status
maestro runs                          # list historical runs
maestro runs evidence [run-id]         # show evidence: task timeline, overlap, worktrees
maestro runs replay [run-id]           # reconstruct timeline from events + evidence
maestro runs pr-body [run-id] --write   # write PR_BODY.md from evidence
maestro pr [--run id] [--push]         # open a draft PR; --push publishes the branch first
maestro runs events [run-id]           # show append-only run events
maestro tui [--run <id>]               # terminal dashboard for run status/evidence
maestro logs <task-id> [-f]           # tail a task log
maestro providers [--adapters-only]   # provider health and adapter coverage
```

## Chat

```bash
maestro chat                          # interactive REPL backed by the selected chat provider
maestro chat ls                       # list sessions
maestro chat new                      # create new session and switch to it
maestro chat rm <id>                  # delete a session
maestro chat tag <id> <tag>...        # add tags to a session
maestro chat use <id>                 # set current session
maestro chat brief [id] [--stdout]    # write a continuation handoff brief
```

## Mailbox

```bash
maestro mailbox send --from architect --to frontend --subject "Contract ready" --body "Use schemas/openapi.yaml"
maestro mailbox ls [--all] [--to frontend] [--project web]
maestro mailbox show <id-prefix>
maestro mailbox resolve <id-prefix> [--note "..."]
```

## Memory

```bash
maestro memory ls
maestro memory get <topic>/<id>
maestro memory put <topic>/<id> < file.md    # or paste then Ctrl-D
maestro memory rm <topic>/<id>
```

## Skills

```bash
maestro skill ls
maestro skill new <scope> <id>        # scaffold
maestro skill show <scope> <id>
maestro skill update [id] [--force]   # install/refresh bundled skills
maestro skill rm <scope> <id>
maestro skill sync                    # mirror to .cursor/rules + .claude/skills
```

`<scope>` is either `_global` or a project name.

## Learn (failure-driven guardrails)

```bash
maestro learn list                                  # pending proposals, ×occurrences
maestro learn show <fingerprint>                    # full body + provenance
maestro learn promote <fingerprint> --trigger "<phrase>" [--scope <scope>]
maestro learn reject <fingerprint>                  # audit record; not re-proposed
maestro learn scan-memory                           # draft dedup proposals for near-duplicate L2
```

Opt-in (`learning.propose_guardrails: true` in `.maestro/settings.yaml`); a
failed run then distils guardrail proposals you review and promote into skills.
See [Failure-driven learning](15-learning.md).

## Models

```bash
maestro models                        # show cached models
maestro models --refresh              # refresh Cursor's live catalog, use fallbacks elsewhere
```

`maestro providers --json` prints a machine-readable provider registry with
installed status, binary path, adapter support, and model override support.

## Dashboard

```bash
maestro ui [--host 127.0.0.1] [--port 7777]   # foreground server
maestro open [--port 7777] [--no-browser]     # background server + open browser
maestro doc                                    # open the docs site
```

## External history (read-only)

```bash
maestro history                       # list sessions from Cursor IDE, Claude Code, Codex
maestro history show <id>             # print one session's transcript
```

## Common patterns

```bash
# Spin up a workspace from scratch
mkdir myproj && cd myproj && maestro setup && maestro open

# Re-run only the failing tail of a plan
maestro rerun 20260516-082011_d4c95577 --from T_failed_task

# One-command path: scan, register, plan, validate
maestro work "Add JSON output to the CLI" --root ~/work/monorepo --agent codex

# Watch a verify task
maestro logs T_verify -f
```
