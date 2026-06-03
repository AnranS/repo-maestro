# What is maestro?

`maestro` is a **multi-project Agent orchestrator** built for full-stack developers who own a feature end-to-end across several repositories — frontend, backend, mobile, infra — and want an AI assistant that can plan, dispatch, and verify the work as a single coherent unit.

It is **not** a chatbot. It is **not** a CI system. It sits in the middle:

- You tell `maestro` a goal in plain language.
- The orchestrator agent (Cursor by default, Codex when selected) breaks it into a `PLAN.yaml` — a DAG of tasks, each scoped to one project.
- After you confirm the plan, `maestro` runs the DAG in parallel where possible, streams logs into a web dashboard, and writes a final report.
- A **shared memory layer** (L1 facts + L2 decisions) is auto-injected into every sub-task's prompt so the agents stay coherent across repos.

```text
        ┌────────────────────────────────────────────────────────────┐
        │   You: "add username/password login across api + web"      │
        └─────────────────────────┬──────────────────────────────────┘
                                  │
                         ┌────────▼─────────┐
                         │  Planner agent   │  ← selected provider
                         └────────┬─────────┘
                  emits PLAN.yaml │  you review / edit
                                  ▼
                ┌─────────────────────────────────┐
                │  maestro DAG scheduler (Rust+tokio) │
                └──┬──────────────┬──────────────┬┘
                   │              │              │
              ┌────▼──┐      ┌────▼──┐      ┌────▼──┐
              │ T1    │      │ T2    │      │ T3    │
              │login- │ ───▶ │login- │ ───▶ │verify │
              │ api   │      │ web   │      │       │
              └───────┘      └───────┘      └───────┘
```

## Why does this exist?

If you've ever:

- Held five Cursor windows open just to keep mental context across repos,
- Re-typed the same API contract into three agents in a row,
- Forgotten which sub-task is blocked on what,

then you have the exact pain `maestro` is designed for. It turns your "one-person team" into a **coordinated** one-person team.

## Design pillars

| Pillar | Concrete form |
|---|---|
| **Declarative plans** | `PLAN.yaml` is a DAG you can read, edit, version. |
| **Human-in-the-loop** | Every plan is reviewable; tasks can mark `requires_approval_after` for hard gates. |
| **Shared context** | L1 facts and L2 decisions are markdown files auto-injected into agent prompts, following the dependency topology. |
| **Automatic wiring** | Discovery infers project dependencies and contracts from manifests *and* source imports; consumers are auto-ordered after producers and fed the producer's real contract content. |
| **Resilient + observable** | Bounded retries with a circuit breaker, human escalation when it gives up, and an automatic-actions ledger surfaced in the report, run summary, and dashboard. |
| **Monorepo & polyrepo** | Per-repo worktree isolation across separate git roots, with context scoped to each project's own subtree. |
| **Skill playbooks** | Reusable instructions stored as markdown with YAML frontmatter — mirrored into `.cursor/rules/` and `.claude/skills/`. |
| **One binary** | Rust + embedded React UI + embedded docs. Drop it in your `$PATH`, done. |
| **Agent-agnostic** | First-class Cursor, but the Adapter trait makes adding Claude Code, Codex, or a shell-based agent trivial. |

## What maestro is not

- ❌ A **replacement** for your CI. The local DAG runner is for design-time iteration, not production deployment.
- ❌ A **hosted product**. Everything is local: your code, your sessions, your memory.
- ❌ A **prompt manager**. Skills are about reusable workflows, not micro-tweaking system prompts.

## Where to go next

- **Just want to try it?** → [Quick start](#docs/quick-start)
- **Curious about the data model?** → [projects.yaml](#docs/projects-yaml) and [PLAN.yaml](#docs/plans)
- **Ready to build a feature?** → [Chat with the orchestrator](#docs/chat)
