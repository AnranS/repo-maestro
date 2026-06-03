# Backlog — open follow-ups

Tracked, intentionally-deferred work. Each entry says **why** it's deferred and
**how to resume**, with file anchors. Items are removed from here when landed.

_Last updated: 2026-06-02 (round-2 dogfood)._

## Channel

### #8 — `BotmuxTransport.poll` doesn't paginate with `--since-message-id`

- **Where:** `src/channel/transport.rs::poll` (invokes `botmux history --limit N`).
- **Problem:** the fetch is bounded by a fixed recent-N window + a local filter.
  If more than N messages arrive between two polls, the ones below the window
  are never returned, and the cursor advances past them (unrecoverable).
- **Why deferred:** the only correct fix is a **server-side** cursor
  (`--since-message-id`), but whether the installed `botmux` supports that flag
  is unverified — shipping it blind (even behind a flag) could break the channel.
- **How to resume:** confirm `botmux history --since-message-id <id>` exists;
  thread the newest cursor id into the invocation and paginate until botmux
  reports nothing newer than the cursor. Keep the local boundary seen-set filter
  (already landed, see #9) as the safety net. If support is version-dependent,
  gate it behind a channel-config opt-in (default off).

## Server

### #30 — `run_outcome` runs git twice per task

- **Where:** `src/server/handlers/runs.rs::run_outcome`.
- **Problem:** `changed_files_with_status` is invoked per task in the main loop
  and again in the contract-drift block (~`changed_projects`).
- **Why deferred:** cosmetic — this is a cold, human-triggered endpoint, not a
  hot poll. The naive dedup is **behavior-changing**: a safe version must reuse
  the main-loop `files` set AND still iterate tasks that have `workspace_path`
  but no `worktree_path` (the in-place worktree-create-failure fallback), or it
  silently regresses contract-drift detection for in-place runs.
- **How to resume:** collect changed projects into a `HashSet` during the main
  loop when `!files.is_empty()`, then run `changed_files_with_status` only for
  the remaining `workspace_path`-but-no-`worktree_path` tasks — do not drop the
  second pass wholesale.

## Scheduler

### #20 (remainder) — decompose `dispatch_one_task`'s worktree/spawn phase

- **Where:** `src/scheduler/executor.rs::dispatch_one_task`.
- **Done so far:** the memory fan-in (`assemble_memory_context`) and the prompt
  prelude (`assemble_prompt_prelude`) are extracted as clean synchronous phases.
- **Remaining:** the worktree decision/policy/integration/guard-creation and the
  spawned-run closure. These are **entangled** with run-state locks, the
  `fail_task_inline` early-returns, and async spawning.
- **Why deferred:** unlike the extracted phases, this part interleaves
  `state.lock().await`, `.await`, and `LoopFlow::Break/Proceed` early returns, so
  a careless extraction can change lock scope or control flow in ways tests may
  not catch. Extract only with explicit attention to those.

## Discovery / contracts

### #41 — cross-workspace contract consumers never link (0-consumer on real monorepos)

- **Where:** `src/config/discovery/contracts.rs` (provider/consumer matching) +
  `src/config/analyze.rs::producer_projects_for`.
- **Problem:** contract matching is scoped to a single workspace root. On a real
  multi-monorepo dogfood, discovery promoted the in-tree contract **providers**
  but found **0 consumers** — the services consuming those contracts live in a
  **separate sibling repo** outside the scanned root, so `provides`/`consumes`
  never link across the boundary. Surfaced repeatedly via the
  `RUST_LOG=debug` "saw generated-client subdirs but no registered producer
  matched" near-miss logs.
- **Why deferred:** this is **structural**, not a bug — a correct fix needs a
  **cross-workspace provider index** (let maestro recognize a producer that
  lives under a different root), which is a real feature with its own design
  questions (how multiple roots are declared/registered, how stale a remote
  contract snapshot may be, security of reading a sibling tree). It deserves a
  dedicated design round, not a dogfood-tail patch.
- **How to resume:** write a design note for a multi-root provider index
  (likely a `.maestro/workspaces.yaml` listing sibling roots, or an explicit
  `contracts.provides_repo` pointer on the consumer); decide whether matching is
  by registered name or by contract-file fingerprint across roots; keep it
  opt-in so a single-root workspace is unchanged. The near-miss debug log is the
  ready-made signal to validate against.

## Self-evolution (`src/learn`) — future hardening

From the design review of the learning subsystem
([docs/site/15-learning.md](../site/15-learning.md)). Not bugs; hardening so the
opt-in loop can't quietly degrade.

- **Live-skill outcome audit.** Record, per run, which promoted-via-`learn` skill
  triggered and whether that run passed, so a reviewer can flag a
  guardrail/playbook that stopped helping ("skill X triggered in 6 runs, 3
  failed"). This is the missing decay signal that otherwise lets an accepted
  skill become evidence for synthesizing more skills (the reinforcement loop).
- **Bound synthesized skills.** A per-scope cap, and have the refinement path
  MERGE into an existing skill rather than allow a near-duplicate trigger.

---

## Recently resolved (for traceability)

Landed on `main` from the 2026-06 review sweep + follow-ups: discovery.rs split
(#19/#35), `read_json`/`read_yaml` dedup (#33), contract-injection producer
fallback (#40), same-timestamp boundary seen-set dedup (#9), `channels`
listen/drain SIGINT handling (#37), and the bulk of the 48-finding review (40
fixes). See the git log and `CHANGELOG.md`.
