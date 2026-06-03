# Concepts

Maestro uses 8 core terms across configs, code, and docs. Use this page as the
single source of truth; if `docs/design/schema-v1-rfc.md` ever disagrees with
this file, fix the disagreement here first and propagate.

## At A Glance

| Term | One-line | Where it lives in code | How you reference it |
| --- | --- | --- | --- |
| `provider` | An execution capability source. | `src/providers.rs` | `agent: codex` in `PLAN.yaml` or `projects.yaml` defaults |
| `adapter` | Maestro code that invokes a provider. | `src/adapter/*.rs` | Not user-facing; selected by `agent` |
| `agent` | The PLAN task's execution choice. | `task.agent` field | `agent: codex` on a task or as default |
| `role` | Task persona and professional prelude. | `src/roles/*` + `.maestro/roles/*.md` | `role: backend_rust` |
| `skill` | A scoped markdown playbook injected into a task prompt. | `src/skills/*` + `.maestro/skills/**/SKILL.md` | `skills: [qa-web-flow, verify-before-done]` |
| `mode` | Resolved execution constraints from role + skills + allowed tools. | `src/modes/*` | Computed; surfaces in run evidence |
| `model_profile` | Model selection and fallback policy. First concrete profile kind. | `src/config/projects.rs` (`ModelProfile`) + `src/models/*` (model cache) | `model_profile: implementation` |
| `agent_profile` | **Reserved, not yet implemented.** Future optional reference bundle only. | — | — |

## Definitions

### provider

An **execution capability source**: a binary or built-in backend that can carry
out task work. Built-in providers today: `codex`, `cursor`, `shell`, `mock`.
Known but unwired providers (status only): `gemini`, `claude`, `aider`, `goose`,
`opencode`, `qwen`, `cline`, `crush`, `continue`, `openhands`, `mini-swe-agent`.

A provider's capability surface is described by the `ProviderCapability v1`
schema (see [`schema-v1-rfc.md`](./design/schema-v1-rfc.md)): `installed`,
`authenticated`, `adapter_available`, `execution`, `permissions`, `models`,
`tool_trace`.

### adapter

The Maestro Rust code that drives a provider. One adapter per supported
provider; each lives under `src/adapter/`. Users do not reference adapters
directly — `agent` selects a provider, and Maestro picks the adapter.

### agent

The PLAN task field that picks **which provider executes this task**. In most
of the codebase `agent` and `provider id` are the same string; future work may
introduce richer routing.

```yaml
tasks:
  - id: T_backend
    project: api
    agent: codex   # ← this is the agent (= provider id today)
```

### role

A reusable **task persona** with a professional prelude. Roles live in
`src/roles/builtin/` and user-defined ones in `.maestro/roles/`. Each role has
optional `allowed_tools` and `skills` fields in its YAML frontmatter.

```yaml
tasks:
  - id: T_review
    role: qa          # ← prelude + permission profile
```

### skill

A **scoped markdown playbook** that gets injected into a task's prompt before
dispatch. Skills live in `.maestro/skills/_global/` or `.maestro/skills/<project>/`.

```yaml
tasks:
  - id: T_implement
    skills: [verify-before-done, qa-web-flow]
```

### mode

The **resolved execution constraints** computed at dispatch time from a role's
`allowed_tools`, the chosen skills, and any task-level overrides. Mode is not
something you write by hand — it is derived and recorded in run evidence
(`permission.resolved.*` per the `Permission v1` schema).

### model_profile

A **model selection and fallback policy** declared in
`projects.yaml.defaults.model_profiles`. The first concrete `profile` kind in
Maestro; future `agent_profile` (see below) would be another. Profiles only
aggregate; they do not hide the underlying provider, role, mode, or skills.

The struct is defined in `src/config/projects.rs` (`ModelProfile`):
`preferred` is the first model to try, `fallback` is an ordered list. Maestro
picks the first candidate present in the refreshed model cache (`src/models/`)
when one exists; otherwise it passes `preferred` straight through to the
adapter.

```yaml
defaults:
  model_profiles:
    implementation:
      preferred: gpt-5
      fallback: [claude-sonnet-4.5, gemini-2.5-pro]
```

### agent_profile

**Reserved, not yet implemented.** If added in the future, an `agent_profile`
would aggregate a `role` + `mode` + `model_profile` + permission policy under
one reference name. It would **not** replace any of those fields — it would
only act as a named bundle that downstream tooling can inline back to the flat
fields. This constraint is on purpose: profile types must remain explainable
after resolution, otherwise PR reviewers cannot trace why a task got the
permissions it got.

## Term Relationships

```
projects.yaml ──┬──> agent (provider id)  ──> adapter ──> provider binary
                ├──> role         ──┐
                └──> model_profile  ├──> mode (resolved at dispatch)
                                    │
PLAN.yaml task ──> skills ─────────┘

(future) agent_profile ──> { role, mode, model_profile, permissions }
                            (aggregator only, never replaces fields)
```

## See Also

- [`docs/design/schema-v1-rfc.md`](./design/schema-v1-rfc.md) — authoritative
  schema field definitions for `ProviderCapability v1`, `Permission v1`,
  `RunEvent v1`, etc.
- [`README.md`](../README.md) — quick-start and operational overview.
- [`docs/site/`](./site/) — built-in docs surfaced by `maestro doc`.
