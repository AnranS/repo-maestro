# FAQ

### How is this different from running Codex or Cursor directly?

Codex or Cursor is the engine. `maestro` is the harness around it: project registry, DAG runner, shared memory layer, skills mirror, dashboard, action protocol, and verification gate. Inside `maestro`, every actual LLM call is delegated to the selected adapter.

### Can I use Claude Code or Codex instead?

Codex is supported for task execution with `agent: codex` or `defaults.agent: codex`, and for chat with `maestro chat send --provider codex` or a session provider pin. Claude Code is wired for chat (`provider: claude`) as a simple non-resumable stream; task execution still has no Claude adapter. Cursor remains the default task and chat provider.

### Will `maestro` send my code somewhere?

`maestro` itself is fully local: state, memory, plans, runs — all on disk in your workspace. The LLM calls go wherever your selected AI tool sends them (Cursor's backend for `cursor-agent`, Codex's configured provider for `codex`, or Claude Code's configured backend for `claude`). The auto-tagger explicitly only sends **user-side** messages, never your assistant output.

### Why not store the registry inside each project?

Because the registry is **about** relationships between projects. A single repo doesn't have the full picture. A workspace-level file naturally encodes "these N repos are in this conversation."

### Why YAML for plans, not a real DSL?

- Readable by humans and by every model
- Plays well with diff tools
- Has all the structure we need (lists, optional fields, multiline strings)
- One less compiler to maintain

### Can the orchestrator agent run plans without my approval?

No — by design. Plans must be confirmed via the `maestro-action run` button. Inside a plan, individual tasks can run autonomously, but the plan itself is always gated.

If you want full hands-off, you can `maestro run plans/foo.yaml` from a shell yourself. The dashboard is just one interface.

### My session disappeared!

Sessions live in `.maestro/chat/sessions/`. If the file is still there, it's just hidden by tag filters — clear the filter in the sidebar. If the file is gone, you've probably `rm -rf .maestro`'d a previous workspace.

### Why are some tasks marked `cancelled` instead of `failed`?

Three exit paths:

- **done** — exit code 0
- **failed** — exit code non-zero, or timed out
- **cancelled** — `maestro cancel-run` was triggered while this task was running or queued

`cancelled` is distinct so reports can render them differently and `maestro rerun --from` knows it's safe to retry.

### Can I commit `.maestro/` to my project repo?

Mixed. The pieces:

- `projects.yaml`, `skills/`, `memory/l1_facts/` — **yes**, commit them, they're the team's playbook
- `memory/l2_decisions/`, `runs/`, `chat/sessions/` — **probably not**, generated and noisy
- `*_models.json` / `cursor_models.json` — **no**, machine-specific

A typical `.gitignore` rule:

```
.maestro/runs/
.maestro/chat/
.maestro/*_models.json
.maestro/cursor_models.json
.maestro/memory/l2_decisions/
```

### How do I deploy `maestro` to a server?

Don't. `maestro` is a local-dev tool. For server workflows use your real CI: GitHub Actions, Buildkite, etc. The plans you ran locally can be reused as a starting point but should be ported into the CI's native format.
