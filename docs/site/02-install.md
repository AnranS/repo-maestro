# Install & uninstall

## Build from source

`maestro` is a single Rust binary. To build it:

```bash
git clone <this-repo>
cd multi_projects_cooperation

# 1. Build the embedded dashboard first — the Rust build embeds these assets.
cd web && pnpm install && pnpm build && cd ..

# 2. Build the binary.
cargo build --release
```

The binary lives at `./target/release/maestro`. Drop it somewhere on your `$PATH`:

```bash
cp ./target/release/maestro /usr/local/bin/
```

## Requirements

| Tool | Why | How to check |
|---|---|---|
| Rust 1.76+ | build toolchain | `rustc --version` |
| Node.js 18+ + pnpm | build the embedded dashboard | `pnpm --version` |
| `cursor-agent` | default Agent backend | `cursor-agent status` |
| `codex` | optional Codex task backend | `codex --version` |
| git | inspecting repositories and run diffs | `git --version` |

`cargo build` embeds the already-built dashboard assets from `web/dist` (via `rust-embed`) — it does **not** run the web build for you, so build the web bundle first (step 1 above). If you later change anything under `web/`, re-run `pnpm build` in that folder before re-building Rust.

```bash
cd web && pnpm build && cd ..
cargo build --release
```

## Verifying the install

```bash
maestro --version
maestro --help
```

In any directory, `maestro init` will create the `.maestro/` workspace. You can have any number of workspaces on disk — they don't talk to each other.

## Uninstall

`maestro` writes nothing outside the workspace directory and the state folders used by whichever agent adapter you choose (`cursor-agent`, `codex`, etc.). To fully remove:

```bash
rm /usr/local/bin/maestro                # the binary
rm -rf <your-workspace>/.maestro         # per-workspace state
```

External tool mirrors that `maestro` writes when you run `maestro skill sync`:

- `.cursor/rules/*.mdc` inside each project
- `.claude/skills/<name>/SKILL.md` inside each project

These are *additive* — removing them won't affect anything outside `maestro`'s own behavior.
