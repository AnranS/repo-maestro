# Shared memory: L1 facts + L2 decisions

`maestro`'s biggest difference from "just call an agent CLI in N folders" is that it maintains a **shared knowledge layer** between agents. Every task gets a prelude built from the right slice of this knowledge before its prompt is sent.

There are two tiers:

| Tier | Lives in | Lifetime | Who writes it |
|---|---|---|---|
| **L1 facts** | `.maestro/memory/l1_facts/<topic>/*.md` | Forever, hand-curated | You (or an agent via `maestro-action`) |
| **L2 decisions** | `.maestro/memory/l2_decisions/<project>/*.md` | Forever, append-only | The reports module after every run |

## L1 facts

Topic-scoped, durable knowledge. Think:

- `api/auth-scheme.md` — "we use HS256 with rotating keys per environment"
- `design/typography.md` — "Inter for UI, JetBrains Mono for code"
- `schema/openapi-conventions.md` — "every endpoint must define 400 and 401 explicitly"

```text
.maestro/memory/
└── l1_facts/
    ├── api/
    │   └── auth-scheme.md
    ├── design/
    │   └── typography.md
    └── schema/
        └── openapi-conventions.md
```

A project picks topics via `memory_scope`:

```yaml
projects:
  login-api:
    memory_scope: [api, schema]
  login-web:
    memory_scope: [design]
```

When `login-api`'s task fires, `maestro` reads every `.md` file under `l1_facts/api/` and `l1_facts/schema/` and prepends them to the prompt as a system-style preamble.

### CLI

```bash
maestro memory ls
maestro memory put api/auth-scheme < notes.md
maestro memory get api/auth-scheme
maestro memory rm api/auth-scheme
```

### From the UI

The **context** tab has an editable list of facts grouped by topic. Saving in the in-browser CodeMirror editor writes straight to disk.

## L2 decisions

After every `maestro run`, the reports module appends a markdown decision entry to each project's L2 store summarizing what changed.

```text
.maestro/memory/
└── l2_decisions/
    ├── login-api/
    │   └── 20260516-101205-add-jwt-login.md
    └── login-web/
        └── 20260516-101205-add-jwt-login.md
```

A decision entry includes:

- The run id and timestamp
- The task ids that touched this project
- A short summary (auto-extracted from task output / your chat conversation)
- Any explicit "decided to X because Y" lines you put into chat

L2 entries are auto-injected too — most recent ones first, capped at 5 per project to keep prompts bounded.

## Why two tiers?

| | L1 | L2 |
|---|---|---|
| Use case | Reference material that doesn't change with each feature | A trail of *what* you decided and *when* |
| Editability | Hand-edited; you own the structure | Append-only; structure is enforced |
| Injection | Filtered by `memory_scope` | Always per-project, recency-capped |

If you find yourself constantly editing an L2 file, it's probably an L1 fact in disguise — promote it.

## Privacy

Both tiers are local files. Nothing leaves your machine unless you forward them to an Agent (which by definition does send the prompt content to the model API). The auto-tagger that names chat sessions uses **only user-side messages** for tag generation — assistant output is never sent.
