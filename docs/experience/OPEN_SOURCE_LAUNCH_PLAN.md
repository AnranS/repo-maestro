# Open-source launch plan

> **Scope.** Rules + checklist + copy templates. This file ships with the
> public repo. It contains **no** internal codenames, employee handles,
> dogfood project names, or commit hashes — same boundary as
> [`OPEN_SOURCE_READINESS_CHECKLIST.md`](OPEN_SOURCE_READINESS_CHECKLIST.md).
>
> The companion file is the readiness checklist. Where the readiness
> checklist asks *"is the repo safe to make public?"*, this file asks
> *"is the launch likely to actually land?"*

---

## 1. Naming decision

Locked after a three-way discussion (PM + reviewer + implementer):

| Asset | Name | Rationale |
|---|---|---|
| Public repo | `repo-maestro` | Bare `maestro` is too crowded (multiple high-star OSS projects + the `maestro` crate name is taken). `repo-maestro` is short, search-distinct, and covers monorepo / polyrepo / mixed-workspace audiences without narrowing the targeting. |
| Repo title (README h1) | `Repo Maestro` | Title case for prose; the lowercase package name stays in URLs and code. |
| CLI binary | `maestro` (+ `mst` alias) | Existing hand-feel is good; renaming the binary breaks user muscle memory and existing dot-files / aliases. |
| crate (Cargo `package.name`) | `maestro-cli` (subject to availability check) | Resolves the `maestro` crate collision; `bin = maestro` preserves the CLI invocation. |
| Subtitle (one-line scope) | `for monorepos, polyrepos, and mixed workspaces` | Pulls monorepo users back in — the repo prefix could otherwise read as polyrepo-only. |
| Hero tagline | `One AI prompt across many repos, in dependency order.` | Replaces the prior "dependency-aware orchestrator" phrasing — concrete pain hook, no jargon. |

Rejected candidates (kept here so we don't relitigate):

| Candidate | Why not |
|---|---|
| `maestro` (no prefix) | Repo / crate collisions; SEO drowned. |
| `polyrepo-maestro` | Accurate for the target persona but reads as "not for monorepos" — would silently shrink the audience. |
| `maestro-ai` | "AI" suffix is buzzword-saturated in 2026; no positioning value. |
| `dag-maestro` | Reads as a primitive, not a product. |
| `code-maestro` | Too generic; many products fit. |
| `maestro-runner` | Sounds like a task runner; loses the orchestration framing. |

---

## 2. Launch-readiness gate

A public release is only allowed once **every** item below is checked.
These layer on top of [`OPEN_SOURCE_READINESS_CHECKLIST.md`](OPEN_SOURCE_READINESS_CHECKLIST.md)
§2 — readiness checks the floor, this file checks the launch surface.

### 2.1 README and prose

- [x] Hero tagline rewritten (concrete pain hook, no jargon)
- [x] Subtitle covers monorepo / polyrepo / mixed
- [x] Three differentiator bullets between hero and the long prose
- [ ] zh-CN README mirrors the new hero + bullets
- [ ] Competitive table ("Why Repo Maestro") reachable from the first screen
- [ ] All in-README claims have backing code or docs (no over-marketing —
      reviewer pass)

### 2.2 Visual assets

- [x] Hero GIF (`assets/demo.gif`) re-promoted to the first screen
- [ ] Architecture diagram renders cleanly on github.com (Mermaid)
- [ ] Dashboard 3-up screenshot caption matches current UI labels
- [ ] Trust / cost / recovery GIF still accurate after the latest scheduler
      changes

### 2.3 Install paths

- [ ] `curl … install.sh | sh` install path verified on a fresh macOS
      machine
- [ ] `curl … install.sh | sh` install path verified on a fresh stock
      Linux machine
- [ ] `cargo install --path . --features codegraph` works from a fresh
      clone with only `rustup` + `node`/`pnpm` installed
- [ ] `maestro demo --run` completes on a fresh clone with **no LLM
      credentials** — output and timing match the README claim
- [ ] `maestro --help` / `maestro work --help` / `maestro chat --help` /
      `maestro tui --help` print accurate, non-internal text

### 2.4 Naming pass (step 3 of Cycle 8)

Do **not** flip URLs to a placeholder string until the public org and
repo actually exist — a broken `curl … | sh` URL or a 404 badge in the
README is worse than the unchanged private URL it replaces. The flip
happens as part of the clean-export commit, after the public destination
is reserved.

The replacement matrix the clean-export script must apply:

| Surface | Current (private) | Target (public) | Owner of the flip |
|---|---|---|---|
| README h1 / title prose | `maestro` | `Repo Maestro` | done in commit 44856ea (h1/anchors) + the brand-prose fixup |
| README CI badge URL | `<private-org>/maestro/actions/workflows/ci.yml` | `<public-org>/repo-maestro/actions/workflows/ci.yml` | clean-export commit |
| README install one-liner | `<private-org>/maestro-dist/.../install.sh` | **unchanged** — `maestro-dist` is already a public binary distribution repo | n/a |
| `Cargo.toml` `package.name` | `maestro` | `maestro-cli` (verified free on crates.io 2026-05-29) | clean-export commit |
| `Cargo.toml` `[lib].name` (new) | implicit `maestro` | explicit `maestro` so `use maestro::...` keeps resolving after package rename | clean-export commit |
| `Cargo.toml` `[[bin]]` name | `maestro` | `maestro` (unchanged) | n/a |
| `Cargo.toml` `repository` / `homepage` / `documentation` | `<private-org>/maestro` | `<public-org>/repo-maestro` | clean-export commit |
| `CONTRIBUTING.md` / `SECURITY.md` / `CODE_OF_CONDUCT.md` repo links | private repo URL | public repo URL | clean-export commit |
| `CHANGELOG.md` `[Unreleased]` URL | private repo URL | public repo URL | clean-export commit |
| In-product `--help` text mentioning the repo | private repo URL | public repo URL | clean-export commit |

Pre-flight before the flip:

- [x] `<public-org>` (= `AnranS`) reserved and writable.
- [x] `repo-maestro` empty / does not exist before the flip.
- [x] `maestro-cli` available on crates.io (verified 2026-05-29 via
      `cargo search maestro-cli`).
- [x] **install.sh path stays on `AnranS/maestro-dist`** — already
      public, so this row is a non-flip. The matrix row above is
      annotated.

First clean-export executed 2026-05-29 against `AnranS/repo-maestro`
(private). Tree was a 412-file `git ls-files` snapshot + the §2.4 URL
replacements + the new `[lib] name = "maestro"` block in `Cargo.toml`
that's needed once `package.name` flips to `maestro-cli` so internal
`use maestro::...` keeps resolving. `cargo test --lib --features
codegraph` 512/512 passes on the export tree. Initial public-style
commit message: `Initial public release v0.1.0`.

Visibility flipped private → public on 2026-05-29 with operator
confirmation. The public repo is live at
`https://github.com/AnranS/repo-maestro` (anonymous fetch returns
200).

**Crate publish to crates.io intentionally deferred** (decided
2026-05-29). Two blockers were surfaced during the publish dry-run:

1. `cargo` was not logged in (`~/.cargo/credentials.toml` absent),
   so the publish needs a token round-trip with the operator.
2. The web UI is loaded via `RustEmbed!("web/dist/")`, but
   `web/dist/` is `.gitignore`d, so the clean export does not carry
   the built dashboard. A naive `cargo publish` from the export
   tree would ship a `.crate` that **fails to compile downstream**
   (`cargo install maestro-cli` would error on the missing
   `web/dist/index.html`) and a busted crate can be yanked but not
   deleted from the index.

Fixing #2 cleanly requires adding `[package].include = ["web/dist/
**", …]` to the export Cargo.toml *and* running `pnpm -C web build`
immediately before `cargo publish` every release. That's a real
workflow change, not a one-line edit, so it's been deferred until
the GitHub side has settled.

Until then, users who want a from-source install hit:

```
cargo install --git https://github.com/AnranS/repo-maestro --features codegraph
```

This path embeds the dashboard via the local `pnpm` step the README
already documents, sidestepping the `web/dist/` packaging gap.
The `maestro-cli` crate name is verified free on crates.io as of
2026-05-29 and there's no realistic squat risk during the deferral
window.

When the crate publish is picked up again, the §2.4 matrix gets a
new row for `[package].include` and the launch playbook adds a
"`pnpm build` → `cargo publish`" sequence to step 4.

### 2.5 Clean export gate

- [ ] `OPEN_SOURCE_READINESS_CHECKLIST.md` §3 clean-export run, scanner
      green with `SECRET_SCAN_NO_EXCLUSIONS=1`
- [ ] No ignored local directory bundled into the export tree
- [ ] First public commit message reads `Initial public release vX.Y`

---

## 3. Launch playbook

The launch is one calendar day, but the surface area is multi-channel.
Pre-stage everything; do nothing live that wasn't drafted at least 24h
earlier.

### 3.1 Timing

- **Day −7 to −3** — read the launch-readiness gate above; fix each
  unchecked item.
- **Day −2** — README and visuals frozen; clean export tarball produced
  but not yet pushed.
- **Day −1** — fresh-clone smoke from 3 reviewers; `maestro demo --run`
  must succeed for all 3.
- **Day 0** — push the public repo. Show HN at the Tuesday 6:00–8:00 AM
  Pacific window (highest historical front-page conversion). Avoid US
  holidays and Apple / Google event days.
- **Day 0 + 1h** — once the Show HN post has a stable URL, repost to the
  subreddits below.
- **Day 0 + 4h** — first triage pass on issues + HN comments.
- **Day +2** — if the post crossed the HN front page, publish a
  follow-up "what we learned launching" blog post linking back.

### 3.2 Channels

| Channel | Format | Posting window |
|---|---|---|
| Show HN | Plain text. Title + 2–3 paragraph body, no images. | Tuesday 6:00–8:00 AM PT |
| Twitter / X | 5-tweet thread, one screenshot per tweet | Day 0, after the HN post is live |
| r/rust | Self-post, link to the HN thread for discussion | Day 0 + 1h |
| r/programming | Link to the public repo | Day 0 + 1h |
| r/devops | Self-post, focus on the contract-pause / rerun angle | Day 0 + 2h |
| Personal blog | Long-form: motivation + how it works + lessons | Day +2 |

### 3.3 Copy templates

> Replace `<repo-url>`, `<docs-url>`, `<install-cmd>` at posting time —
> these placeholders are intentional.

**Show HN title** (≤ 80 chars):

```
Show HN: Repo Maestro – One AI prompt across many repos, in dependency order
```

**Show HN body**:

```
Hi HN — Repo Maestro is a local-first orchestrator that turns one
prompt into a dependency-ordered DAG of tasks across many repos.

We built it because single-agent coding tools (Cursor, Cline, Aider)
don't help when a change has to land in 3–5 repos in the right order,
each respecting a shared contract. Maestro plans the topology, runs
each step in its own worktree, pauses consumer tasks until their
producers integrate, and feeds the producer's actual updated contract
content into the next prompt (not a description).

What's there today:
- Auto-discovery of project dependencies from manifests + source imports
- Topology-aware planner with parallelism + contract-aware pauses
- Per-task git worktree isolation, audit trail, one-click rerun
- Multiple agent backends (Codex, Cursor, shell, mock for tests)
- Embedded web dashboard + a chat-TUI
- Local-first; no hosted backend

Install + demo:
  <install-cmd>
  maestro demo --run

Repo: <repo-url>
Docs: <docs-url>

Happy to answer questions about the planner, contract injection, or
the rerun / audit model.
```

**Twitter / X thread skeleton** (5 tweets):

```
1/ Repo Maestro: one AI prompt across many repos, in dependency
   order. Local-first. Open source today. <hero-gif>

2/ The pain: a change spans backend + frontend + CLI + shared
   contract. Single-agent coding tools work great in one repo, but
   no one's running the orchestra across the workspace. <dashboard>

3/ How it works: discover the dependency graph → plan a DAG →
   pause consumers until their producers integrate → inject the
   real updated contract into the consumer's next prompt. <gif>

4/ Auditable runs by construction. Every task lands a PLAN.yaml,
   RUN_STATE.json, REPORT.md. One-click rerun reuses what passed
   and only re-runs what blocked. <screenshot>

5/ Built on Rust, embedded web dashboard, multiple agent backends
   (Codex, Cursor, shell). MIT.  ↓ <repo-url>
```

**r/rust body** (lead with the Rust-specific angle):

```
Local-first Rust CLI that turns one prompt into a dependency-ordered
DAG of coding tasks across many repos. Petgraph-based scheduler;
embedded WebUI via a Vite bundle; per-task git worktree isolation.

Open source today: <repo-url>
Discussion: <hn-url>
```

### 3.4 Pre-staged answers (FAQ)

These come up in every launch — draft answers before posting so the
reply window stays tight.

| Likely question | Pre-staged answer (sketch) |
|---|---|
| "How is this different from Aider / Cline / Cursor?" | Those are single-IDE single-agent. Maestro is the layer above — it coordinates one or more of them across repos with contract awareness. |
| "Why not just use Nx / Lerna / Rush?" | Those are monorepo task runners. Maestro is AI-agent-aware and supports polyrepo + mixed workspaces. |
| "Why local-first / why not hosted?" | Local-first keeps state and source on the developer's machine; the rationale is auditability + zero-credential-leak rather than feature minimalism. Hosted execution is a deliberate non-goal today. |
| "Is the planner reliable on real workspaces?" | Benchmarks against real OSS multi-package PRs are in `bench/scenarios/` (badges are placeholders until the first scheduled run; this is called out in README). |
| "What models are supported?" | The backend list lives in CLI `--help`. Codex, Cursor, shell, mock today; the agent profile mechanism keeps adding more cheap. |
| "Will you accept PRs?" | Yes — see `CONTRIBUTING.md` for the verify gate + branching rules. |

---

## 4. Post-launch

Things to keep an eye on for the first two weeks.

- **Issue triage rotation** — set an explicit on-call schedule. First
  48 hours is the window where contributor-first-impression hardens;
  late replies cost stars.
- **Roadmap visibility** — a public roadmap entry beats answering "what's
  coming?" 30 times. Pin a roadmap issue.
- **Followups picked up** — F-104 (chat-repl TTY coverage) and F-105
  (concurrent_registry flake) are deferred but tracked in the experience
  log. Pick them up within the first sprint after launch so the public
  CI never flakes during the high-attention window.
- **Cross-workspace contract provider index** — open question from the
  Cycle 7 dogfood. If launch traffic includes anyone with the same
  layout, prioritize the design note.

---

Owner: implementer (drafting, README) · reviewer (release gate,
naming pass review) · PM (final naming + launch GO).
Cycle: 8 — launch polish.
Status: README hero done; remaining items per §2 still open.
