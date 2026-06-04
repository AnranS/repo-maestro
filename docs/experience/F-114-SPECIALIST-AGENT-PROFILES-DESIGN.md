# F-114 — Specialist agent profiles (design)

Status: design (awaiting review before implementation) · Owner: dali design /
dafu implementation

## Problem

Maestro can already route a task to a role, attach skills, choose a model
profile, and record findings. Those knobs are scattered:

- `PlanTask.role` / `Project.role` decide the prompt prelude.
- `PlanTask.skills` inject explicit skill bundles.
- `PlanTask.model_profile` / `Project.model_profile` / defaults choose model
  fallback chains.
- `review_by` and F-106's `refute_on_high_risk` add a read-only specialist in
  one narrow case.
- F-110's finding ledger records risk/refute/doctor/etc. outputs.

The missing concept is a **specialist agent profile**: a named, reusable bundle
that says "when this shape of work appears, use this role + these skills + this
model profile + this output contract." Users should be able to create/train a
specialist once, then let Maestro dispatch to it when the task is a good match,
without making the main controller carry every domain rule in its context.

This is **not** a new agent engine. It is a resolver that composes existing
Maestro primitives and records the result.

## Goals

- Let users define neutral, reusable specialists such as
  `contract-reviewer`, `release-privacy-reviewer`, and `recovery-doctor`.
- Keep the scheduler deterministic: explicit task choices always win, then
  profiles fill in missing fields.
- Reduce main-controller context cost by handing a specialist a small scoped
  context bundle, not the full workspace.
- Reuse existing role / skill / model-profile / finding-ledger machinery.
- Provide a creation + training flow that drafts a profile from prior runs but
  requires human promotion before it affects dispatch.

## Non-goals

- No fine-tuning or model training. "Training" means deterministic profile +
  skill distillation from approved run evidence.
- No new LLM backend or parallel executor.
- No arbitrary trigger language / expression evaluator in v1.
- No marketplace or profile-pack export in v1.
- No automatic mutation of code by the profile itself. Specialists still run
  through normal task/review execution paths.
- No internal, real project, brand, or absolute-path examples.

## Storage

v1 stores profiles in `.maestro/projects.yaml` under
`defaults.agent_profiles`.

Why not a separate `.maestro/agents/*.yaml` file in v1:

- Profiles are dispatch policy, and `projects.yaml` is already the registry for
  dispatch policy (`role`, `skills`, `model_profile`, `routing`,
  `refute_on_high_risk`).
- Atomic config writes and validation already exist for `projects.yaml`.
- Clean export / release scanning stays simpler with one policy file.

Future profile packs can add a separate import/export location, but v1 should
avoid another on-disk namespace.

Example:

```yaml
defaults:
  model_profiles:
    strong-review:
      preferred: gpt-5.2
      fallback: [composer-2]

  agent_profiles:
    contract-reviewer:
      role: refuter
      skills:
        - _global/contract-first
        - _global/verify-before-done
      model_profile: strong-review
      context_budget_bytes: 32000
      triggers:
        - on: contract_changed
        - on: path_changed
          patterns: ["idl/**", "schemas/**", "contracts/**", "openapi/**"]
      outputs:
        - finding_kind: refute
        - review_verdict: true

projects:
  billing-service:
    path: ./billing-service
    role: backend
    agent_profile: contract-reviewer
```

## Schema

### `AgentProfile`

| field | type | req | notes |
|---|---|---|---|
| `role` | string | yes | Existing role name (`src/roles` builtin or custom). |
| `skills` | string[] | no | Existing skill names. Resolved like task skills: project scope, then `_global`, unless explicitly scoped. |
| `model_profile` | string | no | Existing `defaults.model_profiles.<name>`. |
| `context_budget_bytes` | usize | no | Soft maximum for the specialist handoff bundle. The resolver should trim optional context before exceeding it. |
| `priority` | i32 | no | Tie-break among automatically matched profiles. Default `0`; higher wins. |
| `triggers` | `ProfileTrigger[]` | no | Closed v1 trigger list. Empty means "manual / project-bound only." |
| `outputs` | `ProfileOutput[]` | no | Declares what the specialist is expected to emit. Used for validation + UI labelling. |
| `enabled` | bool | no | Default true. Training can create disabled drafts. |

Profiles do **not** add new `allowed_tools` in v1. The role remains the place
that defines tool/mode affordances; the profile only bundles existing parts.
That keeps permission semantics in one place.

### Project / task references

Add optional references:

```yaml
projects:
  web-frontend:
    agent_profile: contract-reviewer
    review_profile: contract-reviewer

tasks:
  - id: T_update_api_client
    project: web-frontend
    agent_profile: contract-reviewer
    review_profile: contract-reviewer
```

`PlanTask.agent_profile` is useful for synthesized or hand-authored plans that
want a named specialist without restating role/skills/model_profile. It is a
profile reference, not a replacement for explicit `role`/`skills` fields.

`PlanTask.review_profile` is the same idea for read-only review specialists.
It is deliberately separate from `review_by`: `review_by` remains the existing
role-only escape hatch, while `review_profile` can also carry skills and a model
profile. Explicit `review_by` still wins.

### `ProfileTrigger`

Closed v1 forms:

| trigger | stage | fields | matches when |
|---|---|---|---|
| `task_kind` | pre-dispatch | `kind: agent|verify` | the task kind matches. |
| `project_type` | pre-dispatch | `types: string[]` | project `type` matches. |
| `project_stack` | pre-dispatch | `stacks: string[]` | any project stack tag matches. |
| `project_has_contract` | pre-dispatch | none | the project declares `contracts.provides` or `contracts.consumes`. |
| `issue_code` | pre-dispatch | `codes: string[]` | a F-111 `Issue.code` is present in dry/validate output. |
| `contract_changed` | post-task | none | risk classification sees a contract path in `files_changed`, or the task project has `contracts.provides` / `consumes` touched. |
| `high_risk` | post-task | none | `compute_and_store_risk` / `task_is_high_risk` classifies the task high. |
| `path_changed` | post-task | `patterns: string[]` | a project-relative changed file matches one of the patterns. |
| `finding_kind` | post-task | `kinds: string[]` | a F-110 finding kind is present for the run/task. |

No boolean expressions in v1. A profile matches if **any** trigger matches.
If more than one profile matches, precedence rules below decide.

Stage matters. Pre-dispatch triggers can choose a writer before an agent runs.
Post-task triggers can only choose review/refute/doctor-style specialists after
there is evidence (`files_changed`, risk level, or findings). Do not use a
post-task trigger to choose a writer — the data does not exist yet.

### `ProfileOutput`

| field | notes |
|---|---|
| `finding_kind` | Optional F-110 finding kind (`risk`, `refute`, `approval`, `learn`, `doctor`, `channel`). |
| `review_verdict` | When true, the profile is allowed to run through the existing `review_by` path and must produce `VERDICT: pass|fail`. |
| `issue_codes` | Optional F-111 issue codes the profile may emit in structured previews/diagnostics. |

The output declaration is a **contract and label**, not a new transport. Runtime
outputs still flow through existing mechanisms:

- review verdicts use `run_review`;
- findings append to `.maestro/runs/<run-id>/findings.ndjson`;
- preview/diagnose issues use the F-111 `Issue` envelope when applicable.

## Resolution and precedence

The resolver should compute a `ResolvedAgentProfile` before dispatch/review and
then lower it into existing fields.

### Writer-task path

Highest priority wins:

1. Explicit task fields in `PLAN.yaml`: `agent`, `model`, `model_profile`,
   `role`, `skills`.
2. Explicit `task.agent_profile`, filling only fields not already explicit.
3. Project `agent_profile`, filling only fields not already explicit.
4. Pre-dispatch trigger-matched profiles from `defaults.agent_profiles`,
   filling only fields not already explicit.
5. Existing project fields: `projects.<name>.role`, `model_profile`, `agent`.
6. Existing defaults and routing.

Important: a profile **never overwrites** an explicit task field. It only fills
gaps. If `task.role` is set and profile `role` is different, the task role wins,
but the profile may still contribute skills/model_profile if those are absent.

### Reviewer/refuter path

Reuse F-106's shape rather than creating `refute_by`.

Priority:

1. Explicit `task.review_by` wins.
2. Explicit `task.review_profile`.
3. Project `review_profile`.
4. Post-task trigger-matched profile whose `outputs.review_verdict=true`.
5. Existing F-106 fallback: `defaults.refute_on_high_risk` -> builtin
   `refuter`.

This means F-114 can eventually replace the hard-coded "high-risk => refuter"
toggle with a profile, but v1 should keep F-106 behavior byte-compatible and
only layer profiles before the fallback.

### Conflict resolution

If multiple trigger profiles match the same missing field:

1. Higher `priority` wins (optional integer, default `0`).
2. More specific trigger wins:
   `path_changed` / `issue_code` / `finding_kind` > `contract_changed` /
   `high_risk` / `project_has_contract` > project type/stack/task kind.
3. Lexicographic profile name as deterministic tie-breaker.

Implementation note: put this in a pure resolver and unit test the matrix.
Do not bury precedence in executor branches.

## Specialist handoff context

The main context-cost reduction comes from a small handoff bundle. A specialist
should receive only:

- task id, project name/type/stack, prompt, command;
- resolved contract paths and changed files for that task;
- relevant dependency/blast-radius entries;
- selected findings/issues by id/code;
- referenced artifacts by run-relative path;
- injected skills and role prelude.

It should not receive full workspace registry, unrelated project transcripts, or
raw historical chat. If `context_budget_bytes` is set, optional context is
trimmed in this order:

1. old findings/issues;
2. downstream project summaries;
3. long artifact excerpts;
4. non-triggering skills.

The hard rule: trimming must not remove the task prompt, role prelude, explicit
skills, or changed-file list.

## Creation and training flow

v1 CLI surface:

```bash
maestro agent-profile new contract-reviewer --template contract-reviewer
maestro agent-profile train contract-reviewer --from-run <run-id>
maestro agent-profile eval contract-reviewer --fixture fixtures/contract-change.yaml
maestro agent-profile promote contract-reviewer
```

Semantics:

- `new` writes a disabled draft profile under `defaults.agent_profiles`.
- `train` is deterministic distillation, not model fine-tuning. It reads a prior
  run's plan, findings, reports, and accepted outcomes; drafts skills, trigger
  suggestions, and output contracts; and leaves the profile disabled.
- `eval` runs trigger matching + handoff shaping against a fixture or existing
  run. It does not execute the agent by default.
- `promote` flips `enabled: true` after validation. Promotion requires at least
  one trigger or an explicit project/task binding; otherwise the profile would
  never be used.

Training output should be reviewable in git. It must not include raw run
transcripts, secrets, absolute paths, or real project names.

## UI / audit surface

Minimal v1:

- Run/task detail shows `resolved_agent_profile` if one was used.
- Findings generated by a specialist keep their normal F-110 `kind`, with
  `source` / `provenance.producer` set to the profile name.
- TUI/WebUI can show "specialist: contract-reviewer" as a compact label.

No profile editor in WebUI v1. CLI + YAML is enough.

## Validation rules

`maestro validate` should fail if:

- a project/task references a missing profile;
- a project/task references a disabled profile (except `agent-profile eval`);
- a profile references a missing role, skill, or model profile;
- trigger fields are malformed or empty where required;
- `priority` is outside the supported integer range;
- `context_budget_bytes` is too small to include required context;
- a profile declares `review_verdict=true` but its role cannot be loaded.

It should warn, not fail, if:

- a profile has no triggers but is referenced by a project/task;
- a profile is enabled but never matches any current project;
- a profile has outputs but no current producer path uses them.

## Tests

Config / validation:

- serde round-trip for `AgentProfile`, triggers, outputs, and enabled drafts.
- missing role/skill/model_profile rejects at validate time.
- disabled profile is never auto-matched but can be explicitly evaluated.

Resolver:

- explicit task `role` / `skills` / `model_profile` win over a profile.
- task `agent_profile` fills missing writer fields before project profile.
- project profile fills missing writer fields before pre-dispatch trigger
  profile.
- explicit `review_by` wins over task/project/trigger review profiles.
- post-task triggers never affect writer dispatch.
- trigger tie-break follows priority -> specificity -> name.
- existing F-106 `refute_on_high_risk` behavior is unchanged when no profile
  matches.

Trigger matcher:

- `contract_changed` matches contract paths and ignores unrelated files.
- `project_has_contract` can match before execution without requiring
  `files_changed`.
- `path_changed` patterns are project-relative and reject absolute/traversal
  patterns.
- `issue_code` and `finding_kind` match only structured F-111/F-110 records.

Training / handoff:

- `train --from-run` creates a disabled draft without raw transcript content.
- handoff context excludes unrelated projects and absolute paths.
- `context_budget_bytes` trimming preserves required fields.

Integration:

- a neutral contract-change fixture auto-selects `contract-reviewer`, injects
  its role/skills/model_profile, records the resolved profile in task state, and
  writes a F-110 finding with profile provenance when the specialist emits one.

## Implementation order

1. **Design fixup pass** — settle any review changes before code.
2. **Config types + docs** — `AgentProfile`, triggers, outputs,
   `Project.agent_profile`, `Project.review_profile`,
   `PlanTask.agent_profile`, `PlanTask.review_profile`; no executor wiring yet.
3. **Pure resolver** — profile matching + precedence + conflict resolution as
   standalone functions with unit tests.
4. **Dispatch integration** — lower `ResolvedAgentProfile` into existing
   role/skills/model_profile before agent launch; record
   `resolved_agent_profile` in task state.
5. **Review integration** — allow review-capable profiles to sit before the
   F-106 `refute_on_high_risk` fallback while preserving explicit `review_by`.
6. **Creation/training CLI** — `agent-profile new/train/eval/promote`, starting
   with deterministic draft generation from run artifacts.
7. **Minimal UI label** — show resolved specialist name in run/task detail.
8. **Dogfood** — neutral fixture first, then real workspace privately; only
   category/count findings may be reported outside the private workspace.

## Privacy boundary

All examples and tests use neutral names:

- `contract-reviewer`
- `release-privacy-reviewer`
- `recovery-doctor`
- `billing-service`
- `web-frontend`
- `shared-contracts`

Do not write real project names, platform names, internal paths, raw transcripts,
or private wordlists into docs, tests, screenshots, commit messages, or public
sync output. Training from real runs must redact before writing any profile or
skill content.

## Resolved decisions for implementation

These are the defaults unless review pushes back:

1. **Storage** — `defaults.agent_profiles` in `.maestro/projects.yaml`; no
   separate profile directory in v1.
2. **Trigger syntax** — closed enum forms only; no arbitrary expression DSL.
3. **Precedence** — explicit task fields > task profile > project profile >
   stage-appropriate trigger profile > existing project/default resolution.
4. **Reviewer relation** — explicit `review_by` wins; task/project review
   profiles then post-task review-capable profiles may auto-attach before
   F-106's `refute_on_high_risk` fallback.
5. **Output contract** — use existing F-110 findings / F-111 issues /
   `review_by` verdicts; no new output transport.
6. **Training** — deterministic draft + human promote; no fine-tuning.
