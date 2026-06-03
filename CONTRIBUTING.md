# Contributing to maestro

Thanks for considering a contribution. Maestro is intentionally narrow in
scope — please align on scope before writing code for anything substantial.

## Ground rules

- **Open an issue first** for any change that touches scheduling, the channel
  adapter, plan synthesis, or the public CLI surface. Tiny doc / typo fixes
  can skip straight to a PR.
- **Small, focused PRs.** A reviewable PR is < ~500 lines of diff. Split work
  along natural seams (schema → parsing → routing → persistence → CLI).
- **TDD where it matters.** Scheduler, channel adapter, and plan synth land
  red-test → minimum-impl → next-task. Pure helpers and CLI plumbing can
  skip strict TDD.
- **One concern per commit.** Conventional Commits style preferred:
  `feat(scope): …`, `fix(scope): …`, `docs: …`, `refactor: …`, `test: …`,
  `chore: …`.

## Development setup

Requires:
- Rust toolchain (`rustup`), stable channel. MSRV is **1.82**
  (pinned in `Cargo.toml` and `rust-toolchain.toml`).
- `pnpm` (only if you touch the dashboard under `web/`).
- At least one task backend if you want to run end-to-end: `codex`,
  `cursor-agent`, `shell`, or `mock`.

Clone, build, and run the test suite:

```bash
git clone https://github.com/AnranS/repo-maestro.git
cd maestro
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Build the embedded dashboard:

```bash
cd web
pnpm install
pnpm build
```

Useful local smoke test against the bundled lab projects:

```bash
cargo run --quiet -- work \
  "Add smarter JSON output to the calculator CLI" \
  --root .maestro/codex-workflow-lab \
  --agent codex \
  --out /private/tmp/maestro-smoke-plan.yaml \
  --dry
```

## Branching and PR flow

1. Branch from `main`. Use a descriptive name:
   `feat/channel-poll-since`, `fix/cursor-atomic`, `docs/readme-quickstart`.
2. Keep the branch up to date by rebasing on `main`, not merging it.
3. Push and open a PR. Fill in the PR template. Link the related issue.
4. CI must be green before merge:
   - `rust stable` job (fmt · clippy · test)
   - `web` job (pnpm build, if `web/` changed)
5. Squash-merge is the default. The squashed message should match
   Conventional Commits, e.g. `feat(channel): drain outbound replies (#42)`.

### Local verify gate before pushing main

For any code change, run the bundled verify script **before** `git push`.
It mirrors the CI rust+web jobs, so a green run here means a green CI:

```bash
./scripts/verify-before-push.sh             # full gate (default)
QUICK=1 ./scripts/verify-before-push.sh     # lib tests only, no web build
SKIP_WEB=1 ./scripts/verify-before-push.sh  # full Rust gate, skip web
SCAN_PRIVATE=1 ./scripts/verify-before-push.sh   # also run the secret / internal-string scanner (see below)
```

Docs-only commits (only `*.md`, `docs/**`, `LICENSE`) can skip the gate —
they don't touch CI's rust job, and the docs sit outside the build pipeline.

### Secret / internal-string scan

`scripts/secret-scan.sh` runs a small set of generic regex categories
(absolute local paths, corporate-style email patterns, bearer / AWS /
SSH key shapes) against the tracked working tree. It is OFF by default
in the verify gate so day-to-day pushes aren't blocked by something
that mostly matters near an open-source export, but **release-readiness
checks must run it on**:

```bash
SCAN_PRIVATE=1 ./scripts/verify-before-push.sh
# or just the scanner on its own:
./scripts/secret-scan.sh
```

The scanner emits `path:line  category` only — never the matched text,
so a stray run-log of the output doesn't itself become a leak. Contributors
can extend it with environment-specific patterns via a private wordlist:

```bash
MAESTRO_PRIVATE_WORDLIST=~/.maestro-private-wordlist ./scripts/secret-scan.sh
```

The wordlist path is read at runtime; it's never persisted in the repo
and its contents never echoed.

## Release notes

The project keeps two complementary surfaces — both are mandatory for a
release; neither replaces the other.

**`CHANGELOG.md`** is the canonical, source-of-truth log following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and Semantic
Versioning.

- Every PR that ships user-visible behaviour adds a bullet under
  `## [Unreleased]`, grouped by `### Added` / `### Changed` /
  `### Deprecated` / `### Removed` / `### Fixed` / `### Security`.
- Pure refactors, hygiene fixes, and internal docs do **not** need a
  CHANGELOG entry — they shouldn't be visible to users.
- When cutting a release, the maintainer renames `## [Unreleased]` to
  `## [vX.Y.Z] - YYYY-MM-DD`, then opens a fresh empty `[Unreleased]`
  on top. A diff-link line goes under the new version (compare the
  previous tag to the new one on GitHub).

**GitHub Releases** is the announcement surface that users actually
read. After tagging:

- Cut a release from the new tag and paste the corresponding
  CHANGELOG section into the body. Avoid restating the same content in
  the release prose — link back to the CHANGELOG for the canonical
  list.
- Attach any prebuilt binaries to that GitHub Release; the
  README's curl-installer points at `releases/latest/download/...`.

**Initial public release** (clean-export flow per
[OPEN_SOURCE_READINESS_CHECKLIST §3](docs/experience/OPEN_SOURCE_READINESS_CHECKLIST.md))
is a special case:

- The first published commit on the public repository carries the
  entire current CHANGELOG as the initial state — it's the project's
  history at snapshot time.
- The first GitHub Release on the public repo (e.g. `v0.1.0`)
  references that initial CHANGELOG and explicitly states that the
  internal history pre-dates the public repo and lives separately.

## Code style

### Rust

- `cargo fmt --all` is the format of record.
- `cargo clippy --all-targets -- -D warnings` must pass.
- Prefer expressive names (`is_ready`, `has_pending_replies`) over
  abbreviations.
- Use `thiserror` for typed errors, `anyhow` only at outermost CLI
  boundaries.
- For async code, use `tokio` and `tokio::sync` primitives. Never block
  inside `async fn` — offload to `tokio::task::spawn_blocking`.
- For cursor / log files written by long-running loops: write to a `.tmp`
  sibling and `fs::rename` for atomicity.

### TypeScript / React (web dashboard)

- Follow the rules in `.cursor/rules/front-end-cursor-rules.mdc` if you have
  the editor rule loaded.
- TailwindCSS for styling; avoid raw CSS files.
- Event handlers use the `handle*` prefix.
- No semicolons.

## What to put in a PR description

The PR template asks for:

- **Goal** — one sentence on why this change exists.
- **Approach** — bullet list of what changed, ordered by impact.
- **Verification** — raw output of `cargo fmt --check`, `cargo clippy`,
  `cargo test`, plus any boundary greps you ran.
- **Risks / N-2 follow-ups** — anything you punted to a later PR.

## Reporting bugs / requesting features

Use the GitHub issue templates:

- **Bug report** — what you ran, what happened, what you expected, the
  smallest reproducer.
- **Feature request** — the trigger (the situation that made you wish for
  this), the proposed shape, and what you'd ship without it.

Untriaged issues get the `triage` label. We respond on a best-effort basis;
this is a small project.

## Security

If you find a security issue, **do not open a public issue**. Follow the
process in [SECURITY.md](SECURITY.md).

## Code of conduct

By participating you agree to abide by the
[Code of Conduct](CODE_OF_CONDUCT.md).
