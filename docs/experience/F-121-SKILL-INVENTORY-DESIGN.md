# F-121 - Skill/profile visibility inventory (design)

Status: design / awaiting review · Owner: dali design / dafu implementation

Parent: [reference local agent-runtime package absorption](REFERENCE-LOCAL-AGENT-RUNTIME-PKG-ABSORPTION.md) -
borrowed primitive #4, "skill / profile visibility inventory".

F-121 is a read-only inventory layer. It explains which skills are visible to a
project or specialist profile without loading or rendering skill bodies. It
does not change dispatch, prompt assembly, skill sync, or the existing skill
editor.

## Problem

Maestro already has two skill-related surfaces:

- runtime resolution in `src/skills/mod.rs`, where task dispatch loads full
  skill bodies for prompt injection;
- the WebUI Context tab, where `/api/skills` lists editable skill files.

Those surfaces are useful but they answer a different question. Operators need a
safe way to inspect:

- what a project can see from `_global` plus its project-local scope;
- which skill references a specialist profile declares;
- whether those references resolve under the same project-first rules used at
  dispatch;
- which references are missing, shadowed, malformed, or skipped by scan budget.

Today, answering that question tends to reuse the body-loading path. That is
unnecessarily broad for a visibility view and makes it too easy for an operator
surface to expose playbook content when all it needs is metadata.

## Goals

- Add a stable `SkillInventory` JSON projection for project/profile visibility.
- Read only skill metadata required for inventory: frontmatter `name`,
  `description`, `trigger`, filename, size, and safe symbolic scope.
- Never read or return skill bodies in the inventory path.
- Reuse F-114 resolution semantics: explicit `scope/name` stays exact;
  unscoped references resolve project scope first, then `_global`.
- Make WebUI able to show "this specialist can see these skills" and "these
  profile references are missing" without opening the skill editor or skill
  body.
- Keep scan bounded: max scopes, max files, max frontmatter bytes, max total
  bytes, no symlink traversal, no deep directory walking.

## Non-goals

- No change to `resolve_for_task`, `trigger_match`, prompt injection, skill body
  rendering, or adapter behavior.
- No raw skill body, prompt, role body, memory body, transcript, or log in any
  F-121 API response.
- No skill editing redesign. Existing `GET/PUT /api/skills/:scope/:name`
  remains the editor path and may still return full content.
- No external IDE skill inventory in v1. `.cursor/` and `.claude/` sync outputs
  are not scanned.
- No semantic search, embeddings, content hash, body excerpt, or generated
  summary.
- No public mirror sync is implied by this design. Sync remains a separate
  explicit release decision after implementation and dogfood.

## Existing surfaces to reuse

| Surface | Reuse |
|---|---|
| `SkillScope`, `SkillFront`, `SkillSummary` | Metadata model and scope naming. |
| `reference_exists(project, name)` / `load_visible` semantics | Resolution rules; F-121 should match them without loading bodies. |
| F-114 `AgentProfile.skills` | Declared profile skill references to resolve and explain. |
| F-116 `skills.section` context layer | Runtime "actually injected" provenance; F-121 is the pre-run visibility counterpart. |
| WebUI `ContextView` | Existing Context tab is the right landing area; no new top-level navigation. |

## Data contract

Types should live in a new module:

```text
src/schema/skill_inventory.rs
```

`schema/mod.rs` exports:

```text
SKILL_INVENTORY_V1 = "maestro.skill_inventory.v1"
```

### `SkillInventory`

| field | type | notes |
|---|---|---|
| `schema_version` | string | `maestro.skill_inventory.v1` |
| `project` | string? | project context used for unscoped resolution |
| `profile` | string? | optional specialist profile name |
| `summary` | `SkillInventorySummary` | counts and warnings |
| `visible_skills` | `SkillDescriptor[]` | metadata-only descriptors visible in this context |
| `profile_resolution` | `ProfileSkillResolution?` | present when `profile` is requested |
| `budget` | `SkillInventoryBudget` | effective scan limits |
| `issues` | `InventoryIssue[]` | budget, parse, path, and missing-ref issues |

The builder must be pure with respect to prompt content: it reads metadata from
the skill tree and config, but it never returns body text.

### `SkillInventorySummary`

| field | type | notes |
|---|---|---|
| `scope_count` | u32 | scopes considered |
| `skill_count` | u32 | descriptors returned |
| `visible_count` | u32 | visible to the selected project/profile context |
| `profile_ref_count` | u32 | declared refs on requested profile |
| `missing_ref_count` | u32 | refs that do not resolve |
| `skipped_count` | u32 | files/scopes skipped by policy or budget |
| `truncated_count` | u32 | metadata reads truncated by frontmatter budget |

### `SkillDescriptor`

| field | type | notes |
|---|---|---|
| `scope` | string | `_global` or project scope |
| `name` | string | resolved skill name, frontmatter name if valid else filename |
| `declared_name` | string? | optional frontmatter `name` when it differs from filename |
| `description` | string? | frontmatter only, single-line and capped |
| `trigger` | string? | frontmatter only, single-line and capped |
| `source_ref` | string | symbolic, e.g. `_global/contract-reviewer`; never absolute |
| `file_bytes` | u64 | from metadata, not a body read |
| `frontmatter_bytes_read` | u64 | capped metadata read size |
| `truncated` | bool | true when frontmatter budget stopped the metadata read |
| `issues` | `InventoryIssue[]` | descriptor-local parse/path warnings |

No body excerpt field exists in v1.

### `ProfileSkillResolution`

| field | type | notes |
|---|---|---|
| `profile` | string | requested profile |
| `enabled` | bool | profile state from F-114 |
| `role` | string? | metadata only |
| `model_profile` | string? | metadata only |
| `declared_skills` | `ProfileSkillRef[]` | each declared ref plus resolution |

### `ProfileSkillRef`

| field | type | notes |
|---|---|---|
| `ref` | string | original declared string, trimmed and capped |
| `resolution` | string | `resolved`, `missing`, `invalid`, or `deferred` |
| `resolved_scope` | string? | `_global` or project scope when resolved |
| `resolved_name` | string? | resolved skill name |
| `reason` | string? | short machine-readable reason |

Resolution rules:

- `_global/foo` resolves exactly in `_global`;
- `project/foo` resolves exactly in `project`;
- `foo` resolves to project scope first when `project` is provided, then
  `_global`;
- `foo` without project context resolves only `_global` in v1 and may be
  `deferred` if the profile is trigger-only and not pinned to a project.

### `InventoryIssue`

Use the F-111 issue-envelope style without importing the full `PlanPreview`
shape:

| field | type | notes |
|---|---|---|
| `code` | string | flat dotted code |
| `severity` | string | `info`, `warning`, or `error` |
| `scope` | string? | symbolic scope |
| `skill` | string? | skill name or ref |
| `message` | string | neutral, single-line, capped |
| `suggestions` | string[] | optional short suggestions |

Initial codes:

| code | severity | trigger |
|---|---|---|
| `skill_inventory.budget_exceeded` | warning | max file/scope/bytes budget hit |
| `skill_inventory.frontmatter_truncated` | warning | frontmatter read stopped at cap |
| `skill_inventory.frontmatter_invalid` | warning | YAML frontmatter failed to parse |
| `skill_inventory.unsafe_scope` | error | scope path component rejected |
| `skill_inventory.unsafe_name` | error | filename/name rejected |
| `skill_inventory.symlink_skipped` | warning | file/dir is a symlink |
| `skill_inventory.missing_profile` | error | requested profile does not exist |
| `skill_inventory.missing_skill_ref` | error | profile declares a skill that cannot resolve |
| `skill_inventory.shadowed_by_project` | info | project-local skill wins over `_global` for an unscoped ref |

## Scan budget and filesystem policy

Add a small budget type with conservative defaults:

| field | default | notes |
|---|---:|---|
| `max_scopes` | 128 | includes `_global` |
| `max_files` | 512 | markdown files considered |
| `max_total_frontmatter_bytes` | 1 MiB | cumulative bytes read |
| `max_frontmatter_bytes_per_file` | 16 KiB | per file cap |
| `max_name_bytes` | 120 | descriptor and ref labels |

Filesystem rules:

- root is `.maestro/skills`;
- only immediate scope directories are scanned, no recursive skill body search;
- only `.md` files are candidates;
- symlinks are skipped;
- invalid scope names or skill file names are skipped with issues;
- no absolute path is returned in the JSON response;
- missing `.maestro/skills` returns an empty inventory, not an error.

Budget overflow returns partial inventory plus `budget_exceeded` issues. It does
not fail the endpoint unless the root itself cannot be read or a path guard
detects an escape.

## Metadata parser

Do not call `parse_skill` from the inventory scanner. `parse_skill` reads the
whole file and returns full `content`, which is correct for dispatch/editor
paths but too broad for inventory.

Add a separate metadata-only helper, for example:

```text
read_skill_descriptor_budgeted(scope, path, budget) -> SkillDescriptor
```

It reads at most `max_frontmatter_bytes_per_file`, extracts only the first YAML
frontmatter block if it is complete within that cap, and never stores body text.
If the frontmatter block does not close before the cap, return filename-based
metadata with `truncated=true` and an issue.

String constraints:

- `description`, `trigger`, `message`, and suggestions must be single-line and
  capped;
- unsafe path-like strings are rejected before they become refs;
- invalid frontmatter should not cause body reads.

## API

Add one read-only endpoint:

| method | path | response |
|---|---|---|
| `GET` | `/api/skills/inventory?project=<name>&profile=<name>` | `SkillInventory` |

Route ordering matters: register `/api/skills/inventory` before the existing
`/api/skills/:scope/:name` editor route.

Error semantics:

- unknown explicit `project` -> `404`;
- unknown explicit `profile` -> `404`;
- invalid query value -> `400`;
- unreadable skills root or corrupt config -> `500`;
- missing skills root -> `200` with empty inventory.

The endpoint returns JSON only. It never returns raw Markdown bodies.

## WebUI scope

F-121 WebUI is intentionally small and read-only.

Add a "Skill inventory" section to the Context tab:

- reuse the existing project filter;
- show summary chips: visible skills, profile refs, missing refs, budget
  warnings;
- show a compact list grouped by `_global` and selected project scope;
- when a profile is selected, show its declared skill refs and resolution
  status;
- distinguish `resolved`, `missing`, `invalid`, `deferred`, and
  `shadowed_by_project`;
- keep the existing skill editor/detail behavior unchanged.

UI/UX borrowed shape:

- dense operator-console layout, not a marketing card;
- counts first, details second;
- warnings are visible but not modal;
- no nested cards;
- no raw body preview.

F-116 task context manifests can later link `skills.section` refs to this
inventory, but F-121 v1 does not need to modify the manifest schema.

## Tests

Step 1 - schema + scanner:

- metadata-only parser does not return or retain body text;
- huge skill body with marker text never appears in `SkillInventory`;
- complete frontmatter returns name/description/trigger;
- incomplete frontmatter at cap returns descriptor with `truncated=true`;
- symlink skill file is skipped;
- invalid scope/name is skipped with issue;
- project-local skill shadows `_global` for unscoped profile refs;
- explicit `_global/foo` ignores project-local shadowing;
- budget overflow returns partial inventory plus warnings.

Step 2 - endpoint:

- `/api/skills/inventory` returns v1 schema and no body content;
- project filter returns `_global` + selected project only;
- unknown project/profile returns 404;
- route does not collide with `/api/skills/:scope/:name`;
- no absolute paths in JSON.

Step 3 - WebUI:

- TypeScript build passes;
- empty inventory renders an empty state;
- missing profile refs render as warnings;
- project filter changes inventory without opening skill bodies.

Dogfood:

- neutral workspace with `_global/contract-reviewer` and
  `billing-service/contract-reviewer`;
- profile declares `contract-reviewer` and `_global/release-check`;
- inventory shows project-local shadow for the unscoped ref and exact global
  resolution for `_global/release-check`;
- screenshot contains only neutral names and metadata.

## Implementation slices

1. **Schema + scanner**: `src/schema/skill_inventory.rs`, budgeted scanner in a
   new `src/skills/inventory.rs` or equivalent submodule; no server/UI.
2. **Endpoint**: `GET /api/skills/inventory`, route-order tests, JSON privacy
   tests.
3. **WebUI read-only panel**: Context tab inventory panel, profile-resolution
   view, no editor changes.
4. **Dogfood + docs**: neutral fixture, screenshot for review, standing
   all-targets gate and explicit private wordlist scan.

## Review decisions locked for v1

- v1 has no body excerpts.
- v1 skips symlinks instead of following them.
- v1 uses partial results for budget overflow.
- v1 keeps `/api/skills/:scope/:name` as the only full-body editor path.
- v1 does not scan IDE-native synced skill directories.
- public mirror sync is not automatic; it is a separate release call.

## Status — shipped

F-121 shipped to private `main` in four reviewed slices (design / implementation
split), each gated with `cargo check` / `clippy -D warnings` / `cargo test`
(`--all-targets`, embeddings + codegraph) plus the explicit private-wordlist
release-scope scan.

- **Step 1 — schema + budgeted scanner** (`856b570`, review `3304cf4`):
  `src/schema/skill_inventory.rs` (the `maestro.skill_inventory.v1` contract and
  a body-free `validate_inventory`) and `src/skills/inventory.rs` (a budgeted,
  metadata-only scanner that never calls `parse_skill` — it reads frontmatter
  incrementally and stops at the closing `\n---`, so a body never lands in
  memory; resolution mirrors `load_visible`; symlinks are skipped via
  `symlink_metadata`; `max_files` / `max_total_frontmatter_bytes` /
  `max_frontmatter_bytes_per_file` / `max_scopes` are hard caps that yield a
  partial result plus `budget_exceeded`). 24 unit tests.
- **Step 2 — endpoint** (`c7a606e`, review `2323647`):
  `GET /api/skills/inventory`, registered before the `:scope/:name` editor route;
  a path-unsafe query value is a neutral `400`; an unknown project/profile is a
  `404`; a missing skills root is a `200` empty inventory; `validate_inventory`
  runs before emit so a malformed projection is a neutral `500`, never
  half-JSON. 7 integration tests.
- **Step 3 — Context-tab panel** (`e00ac04`, review `27d9198`): a read-only
  "skill inventory › visibility" panel — summary chips, scope-grouped visible
  skills (metadata only), and profile-ref resolution (resolved / missing /
  invalid / deferred / shadowed-by-project), reusing the existing project
  filter, without touching the skill editor. The review also made
  `GET /api/skills` return body-free summaries so opening the Context tab no
  longer downloads playbook content to the browser; `/api/skills/:scope/:name`
  stays the only full-body path.
- **Step 4 — dogfood + closure**: a neutral fixture (`_global/contract-reviewer`
  + `billing-service/contract-reviewer` + `_global/release-check`, with a
  `reviewer` profile declaring `contract-reviewer`, `_global/release-check`, and
  `ghost-skill`). The inventory shows the unscoped `contract-reviewer` resolving
  project-first (`billing-service`, shadowed by `_global`), `_global/release-check`
  resolving exactly in `_global`, and `ghost-skill` as missing — with no skill
  body, no `content`, and no absolute path on the wire.

**Privacy invariant held end to end:** no skill body, prompt, role/memory body,
absolute path, or content hash appears in any F-121 response — only provenance,
counts, sizes, and symbolic refs.

**Noted follow-up (non-blocking):** `skills::list_all_summaries` is body-free on
the wire but still reads full files server-side (it reuses `list_all` then
summarizes). N1's goal — keeping bodies out of the browser — is met; a later
change could give the `/api/skills` summary path a metadata-only parser to cut
local I/O.
