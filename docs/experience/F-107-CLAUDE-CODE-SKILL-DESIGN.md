# F-107 — Repo Maestro as a Claude Code skill

> **Scope.** Design note for the third Dynamic-Workflows borrow item
> (strategy B — make maestro callable *from* Claude Code). **Rules +
> decisions only**, no raw monorepo content. Owner: 街溜子-大福
> (drafting, implementation) · 大力 (review). Status: design —
> implementation after dali's GO.

## 1. Problem / opportunity

Anthropic's Dynamic Workflows scale work **inside a single Claude Code
session** — hundreds of subagents, one repo's test suite as the bar.
They do **not** address the cross-repo case: a change that has to land
in 3–5 repos in dependency order, each respecting a shared contract.
That's exactly maestro's lane.

Today maestro is a separate CLI a human drives. The opportunity: ship a
**Claude Code skill** so that when CC's own planning smells a multi-repo
job, it can *delegate* to maestro instead of trying to fan subagents
across repos it doesn't coordinate. This plugs maestro into the 4.8
ecosystem as the multi-repo orchestration layer rather than competing
with it.

maestro already speaks the SKILL.md format (it exports its own workflow
skills). F-107 is the **reverse direction**: a skill that teaches an
external CC host to invoke the `maestro` CLI.

## 2. Goals and non-goals

**Goals:**
- A distributable `SKILL.md` an external Claude Code user can drop into
  their skills dir, that makes CC delegate multi-repo work to maestro.
- A trigger (`description`) that fires on multi-repo / dependency-order
  work and stays quiet on single-repo work (so it never competes with
  CC's own editing).
- Safe-by-default command guidance: discover → plan → **`--dry` first**
  → human-visible blast radius → only then `--run`.

**Non-goals:**
- Not auto-installing into the user's CC config or editing their
  settings. The user copies it in (or installs via a marketplace later).
- Not wrapping every maestro subcommand — just the
  init → work → (dry) → run happy path plus reading the report.
- Not a Claude Code *plugin* manifest in v1 (bare SKILL.md first; the
  plugin packaging is a follow-up if we want marketplace distribution).
- Not a replacement for CC's single-repo editing — the skill explicitly
  declines single-repo scope.

## 3. The artifact

`skills/repo-maestro/SKILL.md` — a new top-level `skills/` dir, kept
separate from `.maestro/skills/` (maestro's self-dogfood skills) and
`.claude/skills/` (this repo dogfooding maestro on itself).

### 3.1 Frontmatter (the trigger)

```
---
name: repo-maestro
description: >
  Use when a coding task must land across MULTIPLE repos/projects in
  dependency order — e.g. a shared library/contract change plus the
  services and frontends that consume it, or a monorepo + sibling repos
  that must change together. Delegates planning and dependency-ordered
  execution to the local `maestro` CLI. Do NOT use for single-repo,
  single-surface changes — handle those directly.
---
```

The "Do NOT use for single-repo" clause is load-bearing: it keeps the
skill from hijacking ordinary edits.

### 3.2 Body (the instructions CC follows)

1. **Decide scope.** If the change touches only one repo/surface, stop —
   don't use this skill. If it spans ≥ 2 projects in dependency order,
   continue.
2. **Check maestro is installed** (`maestro --version`). If absent, show
   the install one-liner and stop.
3. **Discover** the workspace: `maestro init --analyze --root <dir>` —
   surfaces the project graph + contracts.
4. **Plan dry-first**: `maestro work "<goal>" --root <dir> --dry` —
   renders every task prompt + the per-task blast radius (which
   contracts it touches, how many downstream tasks it affects) without
   spending agent calls.
5. **Surface the plan to the user** — the DAG, the contract edges, the
   risk. Get explicit go before running.
6. **Execute** on approval: `maestro work "<goal>" --root <dir> --run`.
7. **Read the result**: `REPORT.md`, `RUN_STATE.json`, and
   `maestro open` for the dashboard. Report what ran / what blocked.

### 3.3 Guardrails in the body

- Always `--dry` before `--run`; never run without the user seeing the
  blast radius.
- Don't invent project names — use what `maestro init --analyze`
  discovered.
- Surface high-risk / contract-touching tasks explicitly before run.
- The skill drives maestro; it does not edit repos directly.

## 4. Verification / tests

- **Round-trips through maestro's own parser.** A unit test loads
  `skills/repo-maestro/SKILL.md` via `crate::skills` (or `parse_skill`)
  and asserts `name == "repo-maestro"` and a non-empty `description` —
  proving the artifact is well-formed by maestro's own rules.
- **Referenced commands exist.** A test (or a doc-lint) asserts every
  `maestro <subcommand>` named in the body is a real CLI subcommand, so
  the skill can't drift from the CLI surface.
- **Trigger-text guard.** A test asserts the description contains the
  "multiple repos" intent and the "not single-repo" disclaimer, so a
  future edit can't silently broaden the trigger.

## 5. Rollout

- Direct push to main + post-push CI + dali review.
- Implementation order: (1) write `skills/repo-maestro/SKILL.md`; (2)
  the 3 guard tests in §4; (3) a short README section + an install line
  ("copy `skills/repo-maestro/` into your Claude Code skills dir"); (4)
  link it from the launch plan as a launch-story bullet ("works *with*
  Claude Code, not against it").
- No behavior change to the maestro binary — this is an additive
  distributable artifact + tests + docs.

## 6. Open questions for dali

1. **Location/name**: `skills/repo-maestro/SKILL.md` at repo root (my
   recommendation) vs `dist/claude-skill/`. Prefer the former — it reads
   as "skills this repo ships."
2. **Bare SKILL.md vs a Claude Code plugin manifest in v1.**
   Recommendation: bare SKILL.md now; plugin/marketplace packaging as a
   separate follow-up once the skill content is proven.
3. **Dry-first as a hard rule in the skill body?** Recommendation: yes —
   the skill always instructs `--dry` then explicit user go before
   `--run`. Safer default for an agent-driven invocation.
