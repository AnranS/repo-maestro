# Skills playbooks

A **skill** is a reusable agent playbook stored as markdown with YAML frontmatter. Skills capture the *how* of a recurring task: "how we write a new API endpoint," "how we run e2e tests on iOS," "how to clean up a stale branch."

They are deliberately small and human-readable. The orchestrator agent injects a skill into a task's prompt when the task's text *triggers* the skill.

## Anatomy

```markdown
---
description: Writing pytest tests for FastAPI endpoints
trigger: write tests, add coverage, pytest, route test
scope: login-api          # or "_global" for cross-project
---

# pytest playbook

1. Use `TestClient` from `fastapi.testclient`.
2. Mock external services with `respx`; never hit real URLs in tests.
3. Each route gets at minimum a happy-path test and one auth-failure test.

```python
from fastapi.testclient import TestClient

def test_login_happy_path(client: TestClient):
    r = client.post("/login", json={"username": "x", "password": "y"})
    assert r.status_code == 200
    assert "token" in r.json()
```
```

The frontmatter fields:

| Field | Required | Meaning |
|---|---|---|
| `description` | yes | One-line summary shown in skill lists. |
| `trigger` | yes | Comma-separated phrases. Any case-insensitive substring match in the task prompt fires the skill. |
| `scope` | yes | `_global` for everywhere, or a project name for repo-local. |
| `name` | no | Anthropic Agent Skills standard field. Optional; if present it sets the skill's name (otherwise the filename is used). maestro already *writes* `name` + `description` when exporting to `.claude/skills/*/SKILL.md`, so a standard-format `SKILL.md` (or a re-imported export) round-trips with its real name. |

Skills live in `.maestro/skills/<scope>/<id>.md`.

## CLI

```bash
maestro skill ls                              # group by scope
maestro skill new login-api pytest-playbook   # scaffold a new skill
maestro skill show login-api pytest-playbook
maestro skill update [qa-web-flow] [--force]  # install/refresh bundled skills
maestro skill sync                            # mirror to .cursor/rules + .claude/skills
```

`maestro skill update` reads the bundled skill catalog from the current `maestro`
binary. By default it installs missing bundled skills and skips existing local
edits; pass `--force` when you intentionally want the bundled copy to overwrite
the local file.

`maestro skill sync` is the magic command: every maestro skill is copied to:

- `.cursor/rules/<scope>__<id>.mdc` — picked up by Cursor IDE on the next reload
- `.claude/skills/<scope>__<id>/SKILL.md` — picked up by Claude Code

So writing a skill once gives you that playbook in every AI tool you use. The sync also re-runs automatically whenever you save a skill from the dashboard.

## Trigger matching

When the orchestrator dispatches an agent task, it injects two sets of visible skills (global + the task's project scope):

- Explicit dependencies from `tasks[*].skills`. These are required; a missing name fails the task before adapter dispatch.
- Trigger matches whose `trigger` text appears in the task prompt.

Skills are appended to the prompt under a clearly marked Maestro skills section.

Match rules:

- Case-insensitive substring match
- Pipe-separated triggers (`a | b | c`) are OR'd
- An empty `trigger` field disables the skill (use this to keep a skill around as documentation only)

Fresh `maestro init` workspaces include workflow guardrail skills you can pin explicitly:

- `workflow-task-guardrails`
- `contract-first`
- `verify-before-done`
- `qa-web-flow` — requires real headless browser evidence for web-facing QA flows

## Why bother?

Without skills, every Cursor session re-learns your conventions from scratch. Skills:

1. **Codify** a convention once and have it pulled in automatically.
2. **Travel** — sync to `.cursor/rules/` so the IDE picks them up too.
3. **Stay scoped** — global rules don't pollute repos that don't need them.

A typical workspace ends up with ~10 skills covering: lint, test, contract changes, error handling, design tokens, accessibility, ops runbooks.
