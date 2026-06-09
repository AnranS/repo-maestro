# F-116 — Prompt context layer debug view (design)

Status: design · Owner: dali design / dafu implementation

Parent: [reference agent-runtime absorption](REFERENCE-AGENT-RUNTIME-ABSORPTION.md) —
borrowed direction #2

## Problem

Maestro already assembles a task prompt from several independent sources:

- task prompt / command text from `PLAN.yaml`;
- memory slices from scoped L1 facts, contract-aware L2 fan-in, dependency L2
  fan-in, and prompt-similarity retrieval;
- project instructions;
- previous-attempt diagnosis;
- consumed contract content;
- codegraph context;
- mailbox messages;
- mode constraints;
- role prelude;
- specialist-profile skills.

The executor then hands the assembled prompt to an adapter. Today a user can see
some coarse breadcrumbs on `TaskState` (`memory_used`, `context_bytes`,
`skills_triggered`, `role`, F-114 `resolved_*_profile`), but there is no stable,
machine-readable view of **which context layers contributed, in what order, and
how large each layer was**. Debugging "why did the agent miss this?" still means
opening logs or reconstructing the dispatch path by hand.

F-116 adds a read-only context-layer manifest for each dispatched agent task.
The manifest is an inspectability layer, not a prompt store. It records
provenance and size metadata for every prompt layer while deliberately omitting
raw prompt bodies and raw layer content.

## Goals

- Make prompt composition inspectable without exposing raw prompt/layer bodies.
- Reuse the existing executor assembly path; do not introduce a second prompt
  renderer or a second memory selector.
- Let WebUI/TUI/MCP answer: which role/profile/skills/memory/code/contract
  layers were included, in what order, and roughly how much context they used.
- Provide a stable JSON contract that external tools can consume.
- Keep v1 small: per-task manifest + read endpoint + minimal UI label/list.

## Non-goals

- No raw prompt viewer.
- No raw memory, skill, role, mailbox, codegraph, contract, log, transcript, or
  previous-attempt body in any F-116 artifact or API response.
- No exact tokenizer integration in v1. Use deterministic byte counts and a
  documented rough token estimate.
- No prompt diffing, prompt replay, prompt rebuild, or adapter-specific trace
  parsing.
- No change to model dispatch, specialist-profile resolution, memory retrieval,
  or prompt rendering behavior.
- No internal project names, real paths, private brands, environment keys, or
  transcript excerpts in docs/tests.

## Existing surfaces to reuse

F-116 should reuse these already-landed surfaces:

| Existing surface | Reuse |
|---|---|
| `assemble_memory_context` | Source of truth for memory slice selection. It should return summaries alongside slices, not be duplicated. |
| `assemble_prompt_prelude` | Source of truth for non-memory prelude layers and order. It should feed a recorder while building sections. |
| `adapter::join_prompt_prelude` / `render_prompt_full` | Existing renderer remains the only prompt renderer. F-116 records metadata before/around it. |
| `TaskState.context_bytes` / `memory_used` / `skills_triggered` | Keep as coarse legacy breadcrumbs; F-116 manifest is the structured detail. |
| F-114 `resolved_agent_profile` / `resolved_review_profile` | Copy into the manifest header for specialist provenance. |
| F-112 `TaskDetail` | Can link to the context manifest in WebUI later, but F-116 v1 does not need to modify F-112 schema. |
| F-115 `events.ndjson` | Optional producer event after manifest write. It is not the manifest store. |

## Storage and API

### Artifact location

Write one manifest per dispatched **agent** task:

```text
.maestro/runs/<run-id>/context/<task-id>.json
```

Use the same task-id component validation discipline as log / artifact handlers:
if a task id is not a safe path component, skip manifest artifact generation
and warn; do not sanitize it into a different filename.

Verify / shell tasks do not get a context manifest in v1. They can still expose
their command/log through existing surfaces. A future v2 may add a small command
manifest if users ask for it.

### Endpoint

Add a read-only endpoint:

| method | path | response |
|---|---|---|
| `GET` | `/api/runs/:id/tasks/:task/context` | `TaskContextManifest` |

Error semantics:

- invalid run id or task id -> `400`;
- unknown run -> `404`;
- known run but unknown task -> `404`;
- known task with no manifest -> `404`;
- corrupt manifest -> `500`.

The endpoint reads only the manifest artifact. It must not rebuild the prompt or
read raw memory/role/skill files on demand, because that would drift from what
the agent actually saw.

## Schema

Types should live in a new module:

```text
src/schema/context.rs
```

`schema/mod.rs` exports:

```text
TASK_CONTEXT_MANIFEST_V1 = "maestro.task_context_manifest.v1"
```

### `TaskContextManifest`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.task_context_manifest.v1` |
| `run_id` | string | parent run id |
| `task_id` | string | task id |
| `project` | string | project id |
| `kind` | string | task kind, expected `agent` in v1 |
| `agent` | string | selected adapter/backend |
| `model` | string? | resolved model label, if Maestro chose one |
| `role` | string? | resolved role name |
| `resolved_agent_profile` | string? | F-114 writer provenance |
| `resolved_review_profile` | string? | F-114 reviewer provenance, if already known |
| `total_context_bytes` | u64 | sum of included layer byte counts |
| `estimated_input_tokens` | u64 | deterministic rough estimate, see below |
| `layers` | `ContextLayer[]` | ordered prompt-context layers |
| `created_at` | string | RFC3339, supplied by caller for deterministic tests |

`estimated_input_tokens` is **not** adapter-reported usage. It is a rough,
deterministic estimate:

```text
ceil(total_context_bytes / 4)
```

Adapter-reported real usage remains `TaskState.usage` / F-112 monitor usage.
F-116 should label estimates as estimates everywhere.

### `ContextLayer`

| field | type | notes |
|---|---|---|
| `order` | u32 | layer order before the task body reaches the adapter |
| `id` | string | stable slug, e.g. `memory.topic_scope`, `role.prelude` |
| `kind` | string | closed enum, see below |
| `label` | string | short display label, single-line, <= 120 bytes |
| `source` | string | symbolic source, never an absolute path |
| `item_count` | u32 | number of slices/messages/skills/files represented |
| `content_bytes` | u64 | bytes that were included in the actual prompt layer |
| `estimated_tokens` | u64 | `ceil(content_bytes / 4)` |
| `truncated` | bool | true if this layer was capped before injection |
| `omitted` | bool | true if the layer was considered but empty/skipped |
| `omitted_reason` | string? | `not_applicable`, `empty`, `load_failed`, etc. |
| `refs` | `ContextLayerRef[]` | symbolic/run-relative refs only |

### `ContextLayerKind`

Closed v1 kind set:

| kind | source |
|---|---|
| `task.prompt` | user-authored task body, count only; no body |
| `project.instructions` | rendered project instruction section |
| `attempt.diagnosis` | previous-attempt diagnosis section |
| `contract.consumed` | consumed-contract section |
| `code.context` | codegraph relevant-code section |
| `mailbox.inbox` | delivered mailbox messages |
| `mode.constraints` | role/mode permission constraints |
| `role.prelude` | resolved role prelude |
| `skills.section` | injected skill playbooks |
| `memory.topic_scope` | explicit topic / project memory scope |
| `memory.contract_fan_in` | contract-aware L2 fan-in |
| `memory.dependency_fan_in` | topology-aware L2 fan-in |
| `memory.prompt_similarity` | TF-IDF prompt-similarity retrieval |
| `workflow.inputs` | workflow outputs injected as task context |

### `ContextLayerRef`

| field | type | notes |
|---|---|---|
| `kind` | string | `memory_topic`, `skill`, `role`, `profile`, `artifact`, `count`, etc. |
| `ref` | string | symbolic or run-relative; no absolute path and no `..` |
| `count` | u32? | optional aggregate count |

Examples:

```json
{"kind":"skill","ref":"_global/verify-before-done"}
{"kind":"role","ref":"backend_rust"}
{"kind":"profile","ref":"backend-specialist"}
{"kind":"memory_topic","ref":"billing-service/decisions"}
{"kind":"count","ref":"mailbox.messages","count":2}
```

Do not emit raw file paths for codegraph hits in v1. Use counts only. A future
v2 may add project-relative refs after a separate privacy review.

## Production rules

### Recorder, not renderer

Add a small `ContextLayerRecorder` used by executor assembly. It records metadata
for exactly the layers the existing assembly path already builds. It must not
decide which content to inject.

The only allowed behavior change is adding metadata and writing a manifest. The
rendered prompt sent to adapters must be byte-equivalent except for unrelated
bug fixes explicitly reviewed outside F-116.

### Memory provenance split

`assemble_memory_context` currently returns a flat `Vec<MemorySlice>`. F-116
should refactor it to return:

```text
MemoryContextAssembly {
  slices: Vec<MemorySlice>,
  layers: Vec<ContextLayer>,
}
```

Each existing load path contributes a separate layer summary:

- topic scope;
- contract fan-in;
- dependency fan-in;
- prompt-similarity retrieval.

The returned `slices` must be identical to today's context vector. Tests should
lock this by comparing topics/contents before and after the refactor.

### Prelude provenance

`assemble_prompt_prelude` should keep returning `Option<String>`, but internally
or via a sibling builder it should emit one layer summary per section:

- project instructions;
- previous-attempt diagnosis;
- consumed contracts;
- code context;
- mailbox inbox;
- mode constraints;
- role prelude;
- skills section.

Empty sections can either be omitted from `layers` or included with
`omitted=true`. v1 chooses **include omitted layers only when useful for user
debugging**:

- include omitted `memory.*` layers with `omitted_reason` when a load failed;
- otherwise omit empty layers to keep the UI small.

### Manifest write

Write the manifest after all context layers are frozen and before the adapter is
invoked. This makes the manifest represent what the agent is about to receive.

Failure policy:

- write failure must warn and continue the task;
- corrupt manifest later returns `500` from the endpoint;
- do not fail the run solely because debug metadata could not be written.

Rationale: F-116 is observability, not workflow semantics. It must not become a
new runtime gate.

### Optional F-115 event

After a manifest is successfully written, emit a small F-115 event:

```text
kind: context.manifest_recorded
status: running
task_id: <task>
refs: [{kind: "artifact", path: "context/<task-id>.json"}]
payload: {layer_count, total_context_bytes, estimated_input_tokens}
```

If F-115 closed kind taxonomy does not yet include this kind, defer the event to
F-116 Step 4 docs/follow-up rather than writing `Other` from a producer. F-115
writer-side `Other` is forbidden.

## UI surface

### WebUI v1

Add a compact read-only panel in task detail / expanded task row:

- header: `Context layers · N · ~X tokens · Y KB`;
- ordered list rows:
  - layer label;
  - item count;
  - byte/token estimate;
  - refs as chips (`role: backend_rust`, `skill: _global/verify-before-done`);
  - omitted/truncated badge when relevant.

No "open raw" button in v1.

### TUI v1

Minimal task detail lines:

```text
context: 9 layers · ~4.2k tokens · 16.8 KB
context refs: role/backend_rust, profile/backend-specialist, skills 2, memory 3
```

If this is too much for the first UI patch, TUI can be Step 4 after WebUI. The
schema/endpoint should not depend on either surface.

## Privacy and validation

F-116 has a stricter privacy boundary than ordinary local logs because it is
meant to be a dashboard/API surface.

Hard rules:

- no raw prompt body;
- no raw layer body;
- no raw mailbox message;
- no raw transcript/log/adapter output;
- no absolute path;
- no `..` traversal in refs;
- no `file:` URI;
- no content digest/hash of raw layer body in v1;
- labels and refs are single-line and byte-capped;
- public docs/tests use only neutral fixtures.

Validation helpers should mirror F-110/F-115 style:

- reject invalid refs before writing;
- reject multi-line labels;
- cap label length;
- serialize empty ref arrays where schema requires them;
- corrupt manifest read is an error, not silently empty.

## Implementation slices

### Step 1 — schema + writer/reader helpers

Add `src/schema/context.rs` plus a scheduler helper module if needed.

Tests:

- serde round-trip for full and minimal manifests;
- rough token estimate is deterministic;
- label validation rejects newline / oversize;
- refs reject absolute path, `..`, Windows absolute/UNC, and `file:` URI;
- corrupt manifest read errors;
- manifest path rejects unsafe task id.

No executor wiring in Step 1.

### Step 2 — executor recording

Refactor context assembly to record layer summaries while preserving prompt
behavior.

Tests:

- memory assembly returns the same `MemorySlice` topics/content as before;
- each memory source produces the expected layer kind/count/bytes;
- skill/role/profile/model metadata appears when F-114 profile supplies them;
- no manifest is written for verify task v1;
- manifest write failure warns but does not fail the task;
- generated JSON contains no raw prompt/layer bodies and no absolute paths.

Gate must be CI-aligned:

```text
cargo check --features embeddings,codegraph --all-targets
cargo test --all-targets --features codegraph
cargo clippy --all-targets --features codegraph -- -D warnings
SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 ./scripts/secret-scan.sh
```

### Step 3 — endpoint + minimal UI

Add:

- `GET /api/runs/:id/tasks/:task/context`;
- WebUI read-only Context panel;
- optional TUI summary lines if the diff stays small.

Tests:

- known run/task returns manifest;
- unknown run/task and unsafe ids return expected status;
- missing manifest returns 404;
- corrupt manifest returns 500;
- WebUI build passes.

### Step 4 — neutral dogfood + public sync

Dogfood only with neutral fixture names:

- one agent task with role + profile + skills + memory + code context;
- verify manifest layer order and counts;
- verify UI shows layer count/token estimate but no raw prompt/body;
- verify explicit scan is clean.

Then clean-export additive public sync if no privacy purge is needed.

## Resolved decisions

1. **No raw content in v1.** This is a manifest/provenance feature, not a prompt
   viewer.
2. **No content hashes in v1.** Hashes of raw prompt bodies can become
   dictionary probes; omit them until a concrete use case exists.
3. **Only agent tasks get manifests in v1.** Verify/shell tasks remain visible
   through command/log surfaces.
4. **Endpoint reads stored manifest, not live rebuild.** Debug must show what the
   task actually saw, not what current files would assemble now.
5. **Manifest write is best-effort.** Observability failure should not fail a
   workflow task.
6. **Use byte counts + rough token estimate.** Exact tokenizer support is a
   future enhancement and must not block v1.

