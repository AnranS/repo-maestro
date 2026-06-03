# `maestro-action` blocks

The action protocol is how the orchestrator agent proposes concrete CLI commands inside a chat message. The dashboard parses these blocks and renders them as confirmable buttons; CLI users see the same blocks as plain YAML they can copy/paste.

This is the human-in-the-loop seam: the agent **suggests**, you **confirm**, and `maestro` actually runs the command. The agent never touches your filesystem without an explicit confirmation.

## Wire format

An action block is a fenced YAML block whose info string is `maestro-action`:

```yaml
verb: work
description: Scan, plan, and validate the login change
spec: Add login JSON output
root: ~/work/apps
agent: codex
```

Anywhere in the assistant message. Multiple blocks per message are allowed.

## Supported verbs

| Verb | Args | Maps to |
|---|---|---|
| `work` | `spec: <goal>`, optional `root`, `agent`, `out`, `run`, `dry` | `maestro work <goal> [--root <dir>]` |
| `run` | `plan: <path>`, optional `plan_hash` | `maestro run <path>` |
| `rerun` | `plan: <run-id>`, optional `from: <task_id>`, `plan_hash` | `maestro rerun <id> [--from <task>]` |
| `approve` | `task: <id>` | `maestro approve <id>` |
| `status` | none | `maestro status` |
| `plan_validate` | `plan: <path>`, optional `plan_hash` | `maestro plan validate <path>` |

The full list lives in `src/chat/actions.rs::ActionVerb`.

## Plan hash guard

For `run`, `rerun`, and `plan_validate`, an action block may include `plan_hash`:

```yaml
verb: run
plan: plans/login.yaml
plan_hash: fnv1a64:8b7e1d2a4f0c9a31
```

Generate it with `maestro plan hash plans/login.yaml`. If the file changes before you click run, the action is rejected instead of dispatching a stale plan.

## UI rendering

Each action block becomes a card with:

- The verb badge (work / run / approve / ...)
- A human-readable label (`description` or the synthesized verb label)
- A **run** button that executes the verb
- An **expand** chevron showing the raw YAML
- An **output pane** that streams the command's stdout/stderr after running, with line-level syntax coloring for diffs, success/error markers, and grep-like patterns

Once executed, the card flips into one of:

- ✓ ok — green ring
- ✗ failed — red ring, output kept for inspection
- ↻ running — animated spinner (rare; most actions are sub-second)

## Why YAML and not function calls?

Three reasons:

1. **Human-editable**. You can hand-author or hand-edit an action block in any markdown file.
2. **Tool-agnostic**. Cursor doesn't (yet) have a stable tool-calling protocol across all its models, but every model can emit a YAML block.
3. **Re-playable**. The action blocks in your chat history form an audit log of "what did the agent ask me to do, and when did I say yes."

## Writing your own

You can paste an action block directly into the chat input — `maestro` will render it the same way. Useful for testing or for scripting:

````markdown
```maestro-action
verb: work
spec: Add JSON output to the CLI
root: ~/work/monorepo
agent: codex
```
````

Skills can suggest the same block when they want the user to confirm a workflow.
