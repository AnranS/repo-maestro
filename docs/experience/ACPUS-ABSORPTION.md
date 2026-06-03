# acpus absorption plan

Status: design / boundary-setting · Owner: maestro · Reviewer: cross-review

Scope: decide which **contract-level primitives** from [acpus](https://github.com/kelvinschen/acpus)
(a public agent-workflow CLI) are worth absorbing into Repo Maestro — and which
are explicitly out of scope. We borrow acpus's *product contracts*, not its
engine.

Repo Maestro stays what it is: a local-first, dependency-ordered, **auditable
multi-repo / multi-agent workflow runtime** built around the repo graph,
`PLAN.yaml`, and the run audit trail. acpus is studied as a reference for clean
contracts (preview, structured issues, read-only projections), not as a workflow
engine to port.

## What acpus does well (the parts worth learning from)

1. A clear layered lifecycle: `validate / preview / run / follow / monitor /
   diagnose / recover / resume`.
2. Authoring spec and the *compiled execution plan* are separate; **preview is a
   first-class artifact**, not a side effect of running.
3. The monitor view and task-detail view are **read-only projections** of run
   state, not direct dumps of the runtime's internal structures.
4. Agent output has an explicit **contract** with a parse / repair boundary;
   problems are expressed as **structured issues** (code / path / message /
   suggestion), not free-form text.

## Non-goals (hard)

- NOT porting acpus's full `workflow.spec` / stage engine / fanout / loop. A
  general workflow engine would dilute Maestro's repo-graph + `PLAN.yaml` +
  run-audit core.
- NOT introducing the `acpx` / Node / Ink TUI dependency stack.
- NOT adding arbitrary loop / fanout execution — that's a large project and a
  poor fit right now.
- NOT mixing this absorption with the just-closed finding-ledger work; these are
  separate lines.

## Fit-gap matrix

| acpus capability | Maestro today | Near-term absorb | Out of scope (now) |
|---|---|---|---|
| First-class machine-readable preview | `work --dry` + `plan validate`, but preview/error shape isn't a stable contract | **F-111** stable JSON preview + `Issue` envelope | porting acpus's authoring spec |
| Structured issues (code/path/suggestion) | warnings/errors are mostly stdout text | **F-111** generic `Issue { code, severity, path, message, suggestions, docs }` | runtime-wide issue refactor |
| Monitor / task-detail as read-only projections | WebUI reads `RunState` fairly directly | **F-112** `/api/runs/:id/monitor` + task-detail projections | replacing the state files |
| diagnose vs recover/resume split (diagnose read-only) | `doctor` / `rerun` / abandoned-run reconcile exist | **F-113** read-only `maestro diagnose <run>` | auto-recover policy |
| Stage engine / fanout / loop | dependency DAG from the repo graph | — | a general workflow engine |
| acpx / Node / Ink TUI | Rust single binary + embedded UI | — | the Node/Ink stack |

## Borrowed directions (by priority)

1. **F-111 — Plan Preview + Issue Envelope.** A stable JSON preview contract
   (task count, project count, dependency edges, blast radius, warnings/errors,
   findings summary) plus a generic `Issue { code, severity, path, message,
   suggestions, docs }`. Wired first into `plan validate` / `work --dry` only —
   NOT runtime. Value: the CC skill, MCP, and WebUI all read one dry-first
   preview instead of scraping stdout. Design:
   [F-111](F-111-PLAN-PREVIEW-ISSUE-ENVELOPE-DESIGN.md).
2. **F-112 — Run Monitor Projection.** Read-only projections
   (`/api/runs/:id/monitor`, `/api/runs/:id/tasks/:task/detail`) so WebUI / TUI /
   MCP share one shape and stop depending on the state-file layout. Deferred.
3. **F-113 — Read-only Diagnose.** `maestro diagnose <run>` reads RunState +
   findings + events + evidence and emits a recovery-diagnosis artifact **without
   changing the run**; recover/resume stay separate. Deferred.

## Implementation order

1. **F-111** (design first, then minimal implementation after design review).
2. F-112 and F-113 are **not** opened concurrently — absorbing acpus must not
   become a large refactor. They follow only after F-111 lands and is reviewed.

## Privacy boundary — no employer-sensitive material

This absorption draws **abstract contract ideas only** from a public reference.
The following classes must NEVER enter this repo (or any public mirror), in any
case or normalized variant: consumer-app brand names, the parent-company name,
internal platform names, internal project names, real repo paths, real tokens,
real ticket/alert content, internal URLs, meeting links, or screenshots. All
examples use neutral fixtures only: `billing-service`, `web-frontend`,
`shared-contracts`, `example-workspace`. The concrete forbidden terms live only
in the operator-local private wordlist, never in-repo.

Before every push:

```bash
SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 ./scripts/secret-scan.sh
```

plus a manual sweep of the keyword classes above (including case and normalized
variants).
