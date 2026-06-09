# Multi-agent task dispatch and closeout SOP

Status: active SOP

This SOP turns chat-driven bot collaboration into a repeatable engineering
closeout loop. Chat remains the notification and decision channel; the durable
facts live in the repo, docs, task cards, CI, and review evidence.

## Core rule

The task card is the source of truth. Agents execute and collect evidence.
Review gates define responsibility boundaries. A task is closed only when the
evidence is complete, CI is green, and the reviewer records pass or blocker.
Non-blocking debt is recorded as follow-up work instead of being buried in chat.

## Minimal objects

| object | purpose | required fields |
|---|---|---|
| WorkflowRun | The durable execution record that ties plan, nodes, gates, and evidence together. | run id, objective/spec, plan preview, node list, gate state, evidence links |
| TaskCard | The durable task record. | objective, scope, owner, executing agent, reviewer, state, links |
| PlanPreview | The reviewable compiled plan before execution starts. | schema version, task count, dependency edges, warnings, errors, approval state |
| WorkflowNode | A reviewable slice of the task. | slice id, dependencies, inputs, outputs, completion criteria |
| ReviewGate | A human or reviewer-agent decision point. | gate type, reviewer, checked range, decision, notes |
| EvidenceArtifact | The only acceptable proof of completion. | commits, CI, commands, screenshots or logs, docs, review result |
| DebtItem | A non-blocking follow-up. | issue, impact, owner suggestion, resume path, blocker status |

## Workflow absorption rules

- Dynamic decomposition is only a draft until it is compiled into a
  `PlanPreview` and recorded against the `WorkflowRun`.
- Agent text is not the source of truth. Node status comes from run/task state,
  event records, and evidence artifacts.
- Gates must be named and auditable. A gate response should identify the checked
  range, decision, reviewer, and any resume value.
- Evidence belongs to a run or node. Keep commit ids, CI, commands, screenshots,
  logs, docs, and review messages attached to the closeout, not buried in chat.
- Product surfaces should consume read-only projections first (`RunMonitor`,
  `TaskDetail`, typed events). Add write controls only after the projection is
  stable and reviewed.

## Dispatch template

Use this when assigning work to another agent.

```text
@<agent> task dispatch:

- Objective:
- Scope:
- Repository / path:
- References:
- Inputs:
- Expected output:
- Evidence required:
- Completion criteria:
- Out of scope:
```

Rules:

- Name exactly one accountable executing agent.
- Name exactly one reviewer or review gate for the next step.
- Include concrete paths, document links, commit ranges, or task ids.
- Include evidence requirements up front. Do not let the executing agent decide
  what proof is enough after the fact.

## Review template

Use this when reviewing another agent's work.

```text
Review result: pass | blocker | follow-up only

Checked range:
- Commit range:
- Files or paths:
- Commands:

Findings:
- blocker: <file:line> <problem> <required fix>
- follow-up: <issue> <why non-blocking> <resume path>

Evidence notes:
- CI:
- Local verification:
- Screenshots / logs / docs:
```

Rules:

- Blockers must include file and line when they are code issues.
- A review pass must say what was checked, not just "looks good".
- For format-only commits, verify they are behavior-free. A strong check is:
  run the formatter on the previous tree and confirm the resulting diff matches
  the format commit.
- Follow-up items must be explicitly non-blocking and must include a resume path.

## Closeout template

Use this when closing a task after review.

```text
Task closeout:

- Final status:
- Commits:
- CI:
- Verification:
- Review gate:
- Documentation:
- Follow-up debt:
- Worktree state:
```

Rules:

- Do not close on agent text alone. At minimum include commit ids, CI status, and
  reviewer decision.
- If CI failed on an intermediate commit but passed later, name both the failed
  run and the green run so the history is understandable.
- If the worktree has unrelated untracked or dirty files, call them out and state
  whether they were touched.
- If a reviewer agent crashes or cannot respond, record the handoff attempts and
  close only when independent evidence and CI are sufficient.

## Evidence checklist

Each closeout should include the relevant subset of this list.

| evidence | expected form |
|---|---|
| Code change | commit ids and pushed branch |
| CI | run id or URL plus conclusion |
| Local commands | exact commands and pass/fail result |
| UI proof | screenshots, Playwright checks, or console/error observations |
| Docs | Feishu or repo doc link and revision when applicable |
| Review | reviewer pass/blocker message id or PR review link |
| Debt | follow-up item, owner suggestion, and why it is non-blocking |

## Z-index sweep review table

For UI token migrations that can affect layering, include this table in the
review package.

| file:line | role | old z | new token | new effective z | changed? / reason |
|---|---|---:|---|---:|---|
| path/to/file.tsx:123 | modal | 50 | z-modal | 50 | N |
| path/to/file.tsx:456 | drawer | 20 | z-drawer | 40 | Y - canonical drawer layer above page popovers |

Rules:

- Every migrated site must have a role.
- Role and tier must agree; a modal must not use a tooltip token.
- If a site intentionally keeps an ad-hoc z-index, list it as retained and state
  why the named scale is not expressive enough yet.

## Operating cadence

1. Dispatch with a concrete template.
2. Execute in small slices and collect evidence while working.
3. Send the reviewer a review package, not a vague request.
4. Reviewer responds with pass, blocker, or follow-up only.
5. Close only after evidence and review are recorded.
6. Move non-blocking debt into the next task queue.

## P0 adoption

The first implementation does not require a new platform. Use this SOP in the
existing Feishu plus botmux plus GitHub workflow. Fielded task cards and UI
automation can come later once the template proves stable.
