# Agent model selection

`maestro` delegates LLM calls to an adapter (`cursor` by default, `codex` when selected) and gives you a hierarchical override mechanism for picking which model runs which task.

## Resolution order

Highest priority wins:

1. **Task-level** — `task.model` in `PLAN.yaml`
2. **Run-level override** — `--model` flag on `maestro run`
3. **Task profile** — `task.model_profile` in `PLAN.yaml`
4. **Project profile** — `projects.<name>.model_profile` in `projects.yaml`
5. **Role profile** — a profile named like the resolved task role
6. **Workspace profile** — `defaults.model_profile` in `projects.yaml`
7. **Project-level** — `projects.<name>.agent_model` in `projects.yaml`
8. **Workspace default** — `defaults.agent_model` in `projects.yaml`
9. **Account default** — whatever the selected agent picks when no flag is supplied

If the resolved model is empty at every level, `maestro` calls the selected adapter without `--model`, letting that tool's account/config default kick in. `agent_model` is the canonical adapter-neutral field. `cursor_model` is still accepted as a legacy alias for existing `projects.yaml` files.

For the **chat tab**, there are additional layers:

- **Session provider pin** — set via the provider picker or `--provider`; overrides `MAESTRO_CHAT_PROVIDER` / `.maestro/settings.yaml`
- **Session model pin** — set via the model picker; overrides workspace default for that session

## Model list

`maestro` keeps local model caches under `.maestro/`: `cursor_models.json` for Cursor and `<provider>_models.json` for other providers. `/api/models` returns models grouped by provider; `/api/models?provider=codex` narrows the list. The Cursor cache is populated automatically on first dashboard load when possible, while Codex/Claude use curated fallbacks until a provider-specific live catalog exists.

```bash
maestro models                # show cached list
maestro models --refresh      # refresh Cursor's live catalog, use fallbacks elsewhere
```

The dashboard has a **refresh model list** button in model pickers and Settings. With a provider selected, refresh targets that provider; without one, it returns the combined provider list with fallbacks for providers that cannot be queried live.

## Model profiles

Profiles let you name a fallback chain once and reuse it from tasks, projects, or roles:

```yaml
defaults:
  model_profile: balanced
  model_profiles:
    balanced:
      preferred: gpt-5.2
      fallback: [composer-2, composer-2-fast]
    reviewer:
      preferred: gpt-5.3-codex-high
      fallback: [gpt-5.2, composer-2]

projects:
  api:
    path: ./api
    model_profile: reviewer
```

When the model cache is available, `maestro` picks the first candidate in the profile that appears in the cache (including aliases). If the cache is empty or missing, it passes the first configured candidate through to the adapter. This keeps plans stable across accounts that expose slightly different model ids.

## Validation

When you supply a model that isn't in the cache, both CLI and UI warn (without blocking) and suggest the closest match by Levenshtein distance:

```bash
$ maestro run plans/foo.yaml --model gpt5
  ⚠ unknown model `gpt5` (not in cached list). Did you mean `gpt-5.2`?
  Run `maestro models --refresh` if you've added it recently.
```

The threshold is tuned to avoid false suggestions for very short typos (`gpt5 → auto` would be silly).

## Tagger model

The auto-tagger that names chat sessions uses a **separate** model:

```yaml
defaults:
  agent_model: gpt-5.2-codex      # the brain for actual work
  tagger_model: gpt-5-mini        # cheap; just generates 1-3 tags
```

If `tagger_model` is unset, the tagger falls back to `agent_model` (or legacy `cursor_model`). If neither is set, it falls back to the account default.

## Practical recommendations

| Workload | Suggested model |
|---|---|
| Complex multi-file refactor | `gpt-5.3-codex-high` / `claude-4.6-opus-max-thinking` |
| Routine endpoint impl | `composer-2-fast` / `gpt-5.2` |
| Quick chat / planning | `gpt-5-mini` / `composer-2-fast` |
| Auto-tagging | `gpt-5-mini` / `claude-4-sonnet` |
| Debug a hard error | `gpt-5.3-codex-xhigh` |

These are starting points — your account may have different access levels and your tasks have different shapes. The point of the override hierarchy is that you can tune per task without touching everything else.

## On token cost

`maestro` will gladly burn through Cursor tokens — there's no budget-tracking layer. If that matters to you:

- Lean on cheap models for the chat orchestrator (it does a lot of planning work).
- Reserve the big models for `task.model` overrides on specific complex tasks.
- The auto-tagger is the easiest big win: set it to the cheapest model your account exposes.
