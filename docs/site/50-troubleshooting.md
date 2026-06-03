# Troubleshooting

## "Operation not permitted (os error 1)" on macOS

`maestro` can't call `getcwd()` in some Desktop subfolders because of macOS TCC (Transparency, Consent, Control). You'll see this when running from `~/Desktop/...` for the first time.

Fix:

1. System Settings → Privacy & Security → Files and Folders → Terminal (or iTerm / your shell host) → enable "Desktop Folder" + "Downloads Folder."
2. Or move your workspace out of `~/Desktop` to `~/work/` or `~/code/`.

## Empty model dropdown

`/api/models` returns provider-tagged models from local caches (`cursor_models.json`, `<provider>_models.json`) plus curated fallbacks. On first launch the Cursor cache may not exist; the server tries `cursor-agent --list-models` in the background and falls back if it cannot.

If that fails (offline, not signed in, binary missing), you'll see the 8-entry hardcoded fallback. Fix:

```bash
cursor-agent status        # are you signed in?
cursor-agent --list-models | head     # does it return data?
maestro models --refresh       # force Cursor refresh from CLI
```

## Dashboard is blank

Open the browser devtools network tab — the most common culprits:

1. **No browser cache**, served new JS, old chunk references — hard reload (Cmd-Shift-R).
2. **/api/state returned 500** — the workspace permissions failed; see TCC fix above.
3. **Console error `Cannot convert undefined or null to object`** — you're on an older binary; rebuild.

## Run hangs at "running" forever

Two possibilities:

1. The agent itself is blocked. Check `maestro logs <task-id> -f` — if it's silent, the model may have stalled. `maestro cancel-run` and retry.
2. A `verify` task is genuinely slow. Default timeout is 5 min; bump with `timeout_minutes: 30`.

## "missing field `spec`"

Your `PLAN.yaml` needs a top-level `spec: <one-line description>`. The agent should always emit this; if you hand-write a plan, don't forget it.

## "agent task X has empty prompt"

`kind: agent` tasks require a non-empty `prompt`. If the task is supposed to run a shell command, set `kind: verify` and use `command` instead.

## Skill / memory edits not visible to the agent

Skills and memory are read at task **dispatch** time, not chat reply time. To see a change take effect, dispatch a new task (or start a new chat message). The dashboard always reads the latest, so the UI is never stale.

## "unknown model" warning with no plausible suggestion

Run `maestro models --refresh` — your Cursor account may have just gained access to a model that isn't in the local cache yet.

## CARGO_TARGET_DIR clobbers builds

Inside Cursor's shell sandbox, `CARGO_TARGET_DIR` is sometimes set to a tmp path. If you see stale builds, run:

```bash
unset CARGO_TARGET_DIR
cargo build --release
```

This is local to your shell; the repo's `target/` will then be correctly populated.

## Where do logs go?

| Source | File |
|---|---|
| `maestro ui` foreground | stdout/stderr of your shell |
| `maestro open` background | `/tmp/maestro-ui.log` |
| Task execution | `.maestro/runs/<id>/logs/<task>.log` |
| Chat sessions | `.maestro/chat/sessions/<id>.json` (full transcript inside) |

When opening a bug report, the run dir + the `maestro ui` log are almost always sufficient.
