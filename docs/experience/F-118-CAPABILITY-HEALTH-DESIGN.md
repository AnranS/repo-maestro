# F-118 — Local runtime capability + health (design)

Status: design / awaiting review · Owner: dali design / dafu implementation

Parent: [reference local agent-runtime package absorption](REFERENCE-LOCAL-AGENT-RUNTIME-PKG-ABSORPTION.md) —
borrowed direction #1. It is also the first runtime data source for the
operator-console Dashboard established by
[F-UI-001](F-UI-001-OPERATOR-CONSOLE-REDESIGN-DESIGN.md).

This is a **design document only**. It does not add feature code. It locks the
local capability/health contract, the timed-probe boundary, the Dashboard
readiness mapping, the privacy rules, and the implementation slices.

## Problem

The new Dashboard intentionally shows six readiness items as `not wired yet`:

- workspace detected
- provider available
- profiles valid
- skills visible
- plan preview valid
- resume guard ready

Those items cannot be inferred from run history without lying. `maestro doctor`
has useful checks, and `maestro providers --json` exposes a static provider
registry, but the UI/MCP/CLI do not yet share one compact, machine-readable
runtime health projection.

F-118 adds that projection. It keeps the tool local-first, runs only small timed
probes, and feeds `doctor runtime`, WebUI Dashboard readiness, and future MCP
consumers with the same schema.

## Goals

- Produce a stable `maestro.runtime_health.v1` report for the six Dashboard
  readiness items.
- Reuse existing source-of-truth checks: workspace paths, `projects.yaml`
  validation, F-111 plan preview, F-114 profile/skill validation, provider
  registry, and F-117 resume validation.
- Run a minimal timed provider probe so health is not just a config read.
- Keep `maestro doctor` behavior intact while adding a focused `doctor runtime`
  view and JSON output.
- Keep all output privacy-safe: no absolute binary path, raw prompt, raw skill
  body, raw log, token, secret, or environment value.

## Non-goals

- No cloud device registration, remote daemon, websocket control plane, SSO/RBAC,
  PAT, member invite, multi-tenant surface, or telemetry.
- No bot or group-chat entry. Chat remains the local Conversation surface; bot
  work follows the dedicated collaboration reference, not this slice.
- No executor behavior changes and no provider-session ownership rules. F-119
  owns session reuse/busy control.
- No full model enumeration as a required health gate. Model list refresh may be
  referenced, but F-118 does not make network/model fetching a blocking probe.
- No raw provider command output in the health report.

## Existing surfaces to reuse

| Surface | Reuse in F-118 |
|---|---|
| `src/providers.rs` | Static provider/capability registry; extend with safe health projection rather than exposing raw paths. |
| `maestro providers --json` | Remains static capability/install view; F-118 does not break it. |
| `maestro doctor` | Existing full report stays; `doctor runtime` is an additive focused view. |
| F-111 `PlanPreview` | Plan preview validity and issue codes feed the plan readiness row. |
| F-114 profiles/skills | `ProjectsConfig::agent_profile_issues`, `roles::exists`, and `skills::reference_exists` feed profiles/skills rows. |
| F-117 resume guard | `validate_resume_target` feeds resume-guard readiness for current/abandoned runs. |
| F-UI-001 Dashboard | Replaces the six `not wired yet` chips with real pass/warn/fail/skip rows. |

## Source of truth

F-118 is a projection, not a new authority:

- `.maestro/` and `projects.yaml` remain the workspace/config authorities.
- Provider descriptors remain the static capability source.
- Existing validators remain authoritative for profile/skill/plan/resume checks.
- The runtime health report is computed on demand and is not stored as a durable
  ledger.

The report answers: "Can the local runtime start useful work right now, and what
single action should the operator take if not?"

## Schema

Types should live in:

```text
src/schema/runtime_health.rs
```

`schema/mod.rs` exports:

```text
RUNTIME_HEALTH_V1 = "maestro.runtime_health.v1"
runtime_health_version()
```

### `RuntimeHealthReport`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.runtime_health.v1` |
| `generated_at` | string | RFC3339, set by handler/CLI wrapper, not by pure projector |
| `summary` | `RuntimeHealthSummary` | total/pass/warn/fail/skip and top issue |
| `checks` | `RuntimeHealthCheck[]` | exactly the six v1 readiness checks, stable order |
| `providers` | `ProviderHealth[]` | focused provider health, safe for UI/MCP |

### `RuntimeHealthSummary`

| field | type | notes |
|---|---|---|
| `total` | u64 | number of checks |
| `passed` | u64 | pass count |
| `warnings` | u64 | warn count |
| `failed` | u64 | fail count |
| `skipped` | u64 | skip count |
| `overall` | string | `pass` if no warn/fail, `warn` if any warn and no fail, `fail` if any fail, else `skip` |
| `top_issue` | string? | first fail/warn message in issue-first order |

### `RuntimeHealthCheck`

| field | type | notes |
|---|---|---|
| `id` | string | closed v1 ids listed below |
| `label` | string | Dashboard label |
| `status` | string | `pass/warn/fail/skip` |
| `severity` | string | `info/low/medium/high`; fail generally high, warn medium |
| `message` | string | short, neutral, no raw path/body/env value |
| `fix` | string? | one copyable CLI action or UI jump |
| `duration_ms` | u64? | present for timed probes |
| `refs` | `HealthRef[]` | symbolic refs only |

V1 check ids and Dashboard labels:

| id | Dashboard label | Source |
|---|---|---|
| `workspace.detected` | Workspace detected | `paths::workspace_root`, `.maestro`, `projects.yaml` |
| `provider.available` | Provider available | provider registry + timed provider probe |
| `profiles.valid` | Profiles valid | F-114 config + role/profile validation |
| `skills.visible` | Skills visible | F-114 skill visibility resolution |
| `plan.preview_valid` | Plan preview valid | F-111 `PlanPreview` over `PLAN.yaml` when present |
| `resume.guard_ready` | Resume guard ready | F-117 current/abandoned run validation |

### `ProviderHealth`

| field | type | notes |
|---|---|---|
| `id` | string | provider id |
| `display` | string | display name |
| `kind` | string | `task_adapter/known_cli` |
| `adapter_available` | bool | from provider descriptor |
| `installed` | bool | binary/built-in presence |
| `probe` | `ProbeResult` | timed local health probe |
| `capabilities` | `ProviderCapabilitySummary` | compact support booleans |
| `message` | string | one-line reason |

Do **not** include `execution.path` or env override values. If a provider is
found by path, the health report says `installed=true`; it does not print where.

### `ProbeResult`

| field | type | notes |
|---|---|---|
| `status` | string | `pass/warn/fail/skip` |
| `duration_ms` | u64? | timed probe duration |
| `timed_out` | bool | true when timeout elapsed |
| `message` | string | neutral one-line result |

## Probe policy

The provider probe is intentionally small:

- deadline: 2 seconds per provider, run concurrently with a small cap;
- command shape: a no-network, no-prompt local liveness command (`--version`,
  `version`, or built-in check), never a model call;
- no stdout/stderr body in the report; at most pass/fail/timeout and duration;
- `mock` and `shell` are built-in/local checks;
- missing binary is `fail` for providers selected by the workspace/defaults, and
  `skip` for known external providers that are not active in the workspace.

Active provider set in v1:

- defaults/project/task adapter values present in `projects.yaml` / `PLAN.yaml`;
- current chat provider may be reported as a `known_cli` provider but does not
  block `provider.available` unless it is the only configured provider.

## Readiness semantics

Each Dashboard row is issue-first:

- `pass`: ready, no action.
- `warn`: usable but degraded; show one fix action.
- `fail`: cannot safely start the intended work; show one fix action.
- `skip`: no data source or not applicable; not an error.

Suggested mapping to existing WebUI status tokens:

| health status | status token |
|---|---|
| `pass` | `done` |
| `warn` | `blocked` |
| `fail` | `failed` |
| `skip` | `pending` |

`provider.available` is `pass` when at least one configured task adapter is
installed and probe-passes. It is `warn` when a provider is installed but probe
times out, and `fail` when all configured task providers are missing or fail the
local probe.

`plan.preview_valid` is `skip` when no `PLAN.yaml` exists. It is `pass/warn/fail`
when a plan exists, using F-111 parse/validate/analyze issue severity. It must
never synthesize or mutate a plan.

`resume.guard_ready` is `skip` when there is no current/abandoned run. It is
`pass` for a current abandoned run that can resume, `warn` for a resumable but
force-gated liveness uncertainty, and `fail` for an abandoned run with blocking
resume issues. Terminal failed/cancelled guidance is `maestro rerun`.

## CLI

Make `doctor runtime` additive:

```text
maestro doctor runtime
maestro doctor runtime --json
```

The existing `maestro doctor` and `maestro doctor --json` stay compatible. The
focused runtime command prints only the six readiness rows + provider probe
summary. Its exit code follows the report: any fail -> non-zero; warn-only ->
zero unless a future `--strict` is added.

If clap nesting makes `doctor runtime` too invasive for v1, the fallback spelling
is `maestro runtime-health [--json]`; however the preferred UX is
`doctor runtime` because it keeps health under the existing diagnostic verb.

## WebUI

Add:

```text
GET /api/runtime/health
```

It returns `RuntimeHealthReport` and does not accept parameters in v1. Errors:

- workspace resolution/config read errors become a valid report with failing
  checks where possible;
- malformed internal artifacts that prevent a specific check are surfaced as
  that check's `fail`, not as a whole-dashboard crash;
- only unrecoverable serialization/runtime errors become 500.

Dashboard changes:

- replace the six hard-coded `not wired yet` rows with `report.checks`;
- keep the same first-viewport structure and shared status chips;
- sort issue-first (fail, warn, pass, skip) while preserving the stable v1 order
  inside each status group;
- show one short `fix` action where present; no dead-clicks for unavailable
  actions.

## MCP / external consumers

No MCP tool is required in v1. The schema is designed so a future MCP tool can
serve the same `RuntimeHealthReport` without inventing another shape.

## Privacy boundary

The report must not contain:

- absolute binary paths, workspace absolute paths, or env values;
- raw prompt / role / skill / memory / transcript / log bodies;
- provider stdout/stderr;
- tokens, cookies, keys, secrets, or credential-like substrings.

Allowed refs are symbolic and short:

- `workspace`, `projects.yaml`, `PLAN.yaml`, `provider:<id>`,
  `profile:<name>`, `skill:<scope>/<name>`, `run:<id>`.

If a lower-level helper already returns a path or env value, the F-118 projector
must redact or replace it before returning the health report.

## Implementation slices

### Step 1 — schema + pure health builder

Files:

- add `src/schema/runtime_health.rs`
- add `src/runtime_health.rs` or `src/health/runtime.rs`
- update `src/schema/mod.rs`

Scope:

- define `RuntimeHealthReport`, checks, provider probe result, summary;
- implement pure summarizers and status mapping;
- implement provider safe projection from existing `ProviderStatus` without
  absolute path/env values;
- add unit tests for summary ordering, status mapping, path/env redaction, and
  stable six-check contract.

No CLI, server, WebUI, or timed process spawning in this step.

### Step 2 — timed provider probe + CLI

Files:

- extend `src/runtime_health.rs`
- modify `src/cli/mod.rs`
- modify `src/cli/commands/doctor.rs` or add a small command module

Scope:

- add timed local probe helper with a 2 second deadline;
- build full `RuntimeHealthReport`;
- add `maestro doctor runtime [--json]`;
- keep existing `doctor` output unchanged;
- tests with fake binaries/scripts for pass/fail/timeout and selected-provider
  behavior.

### Step 3 — WebUI endpoint + Dashboard readiness

Files:

- add server route `GET /api/runtime/health`
- add TS type/API helper
- modify `Dashboard`

Scope:

- Dashboard fetches the health report and renders readiness rows from it;
- loading shows neutral `checking`;
- fetch error shows one neutral unavailable row, not fake pass;
- no dead-clicks; only fix actions with real targets render interactive.

Tests:

- handler returns valid schema;
- handler output contains no absolute paths/env values from provider status;
- web `tsc` + build;
- screenshot neutral fixture.

### Step 4 — dogfood + private收口

Scope:

- neutral workspace dogfood for pass/warn/fail/skip rows;
- verify Dashboard replaces all six `not wired yet` readiness rows with real
  health;
- full standing gate:

```text
cargo check --features embeddings,codegraph --all-targets
cargo test --all-targets --features codegraph
cargo clippy --all-targets --features codegraph -- -D warnings
MAESTRO_PRIVATE_WORDLIST=~/.maestro-private-wordlist.txt SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 ./scripts/secret-scan.sh
web tsc -b
web vite build
```

Report private HEAD + gate + neutral screenshot only. Do not push the public
mirror.

## Open questions for review

1. **CLI spelling:** preferred `maestro doctor runtime`; fallback
   `maestro runtime-health` only if clap nesting is unexpectedly invasive.
2. **Provider probe timeout:** v1 proposes 2 seconds per provider. If you want
   stricter UI responsiveness, use 1 second; if provider startup is slow, use 3
   seconds to match existing doctor command timeout.
3. **Active provider selection:** v1 treats configured task providers as
   readiness-blocking and known-but-unconfigured CLIs as skip. This avoids making
   every benchmarked external CLI a Dashboard failure.

## Acceptance criteria

- `maestro doctor runtime --json` returns `maestro.runtime_health.v1`.
- Existing `maestro doctor` and `maestro providers` behavior remains compatible.
- Dashboard no longer shows `not wired yet` for the six readiness items once the
  endpoint is available.
- Report contains no absolute path/env/raw output even when providers were found
  through PATH/env override.
- Provider probe is timed and never performs a model/prompt call.
- Plan/profile/skill/resume rows reuse the existing validators and do not mutate
  workspace state.
- All implementation reports stay private-only; public mirror is not pushed.

## Status — shipped

All four slices landed (private-only; public mirror not pushed):

- **Step 1** — `schema/runtime_health.rs` + `runtime_health.rs`: types, closed-enum
  validation, pure summarizer, safe provider projection (drops the whole
  `execution` sub-struct).
- **Step 2** — timed 2s local probe (no model/prompt/network, stdio discarded,
  `kill_on_drop`) + the six read-only gatherers reusing the existing validators +
  `maestro doctor runtime [--json]` (additive; validate-before-emit).
- **Step 3** — `GET /api/runtime/health` (validated; neutral 500 body) + the
  Dashboard's six readiness rows from real health (issue-first; neutral
  checking/unavailable; no fake health). The two pure helpers (`plan_preview`,
  `profile_existence_report`) were sunk out of `cli::commands` so the server does
  not depend on a CLI command module.
- **Step 4** — neutral-workspace dogfood exercises pass/warn/fail/skip in one view
  (provider.available forced to fail via a restricted PATH); the Dashboard shows
  all six rows from real health, and a forced fetch failure renders one neutral
  "unavailable" row + retry rather than fake health.

Every acceptance criterion above is met. Standing gate (cargo check/clippy/test
all-targets + secret-scan + web tsc/vite build) green at closure.
