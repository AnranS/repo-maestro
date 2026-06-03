# Failure-driven learning (guardrails)

Maestro already learns *passively*: after a verified run it archives an **L2
decision** and fans it back into related future tasks (see
[Shared memory](12-memory.md)). Active learning adds the first *opt-in* loop —
turning a run's **failures** into reusable **guardrails**, and its **successes**
into reusable **skill playbooks** — while keeping Maestro deterministic,
auditable, and human-governed.

The whole feature is **off by default** and follows one rule: Maestro only ever
*proposes*; nothing changes a future run until a human *promotes* it.

## PROPOSE → REVIEW → PROMOTE

1. **Propose (inert).** Three opt-in producers write **proposals** under
   `.maestro/proposals/` — all purely deterministic (no LLM), each keyed by a
   **fingerprint** so a *recurring* signal bumps an `occurrences` counter instead
   of creating duplicates. A proposal is **not a skill** and is in **no injection
   path** — it cannot affect any run.

   - **Guardrails from failures** (`learning.propose_guardrails`): a run that
     **failed** (a failed acceptance check, a circuit-break, an integration
     conflict, or a failed task) is distilled into a guardrail keyed by a
     fingerprint of the failure signature (the same normalization the
     circuit-breaker uses).
   - **Playbooks from successes** (`learning.synthesize_skills`): a **verified,
     complex** run is distilled into a draft skill **playbook**. "Complex"
     means it did real work — **at least one agent task** — *and* shows a
     breadth signal (multi-project, contract-wiring, or many tasks); a
     verify-only run has no work shape to replay, so it is never synthesized.
     Maestro detects the reusable shape and
     drafts a skeleton (ordered steps, contracts, what verified it) keyed by the
     run's shape, with a deterministic confidence bucket. You turn the skeleton
     into a real how-to at promote time.
   - **Memory curation** (`maestro learn scan-memory`, on demand): scans archived
     [L2 decisions](12-memory.md) for **near-duplicate** records within a project
     (a local TF-IDF cosine) and proposes consolidating each cluster. Records you
     have hand-edited (their "what to remember" section changed) are **excluded**.
     Promoting **keeps the newest** record and tombstones the older
     near-duplicates — no content is merged, so two different contract versions
     are never fused into one.

2. **Review.** A human reads the queue, highest recurrence first:

   ```bash
   maestro learn list                 # pending proposals, ×occurrences
   maestro learn show <fingerprint>   # full body + provenance (which runs)
   ```

3. **Promote (the only thing that changes behavior).**

   ```bash
   maestro learn promote <fingerprint> --trigger "<phrase the skill fires on>"
   maestro learn reject  <fingerprint>   # keep as an audit record; never re-proposed
   ```

   For a **guardrail/playbook**, promotion writes a normal **skill** (see
   [Skills](13-skills.md)) via the same path a hand-authored skill uses — so it
   mirrors to `.cursor/rules` and `.claude/skills`, and fires for matching future
   tasks through the **unchanged** skill machinery. For a **memory-curation**
   proposal, promotion instead consolidates the L2 cluster (keep newest,
   tombstone the rest) — no trigger needed.

## Why a trigger is required

A guardrail is a skill, and skills fire when their `trigger` phrase appears in a
task's prompt. Auto-derived triggers are kept **conservative** — only
high-specificity tokens (paths, dotted names, identifiers), never generic words
like `test`/`build` — and are often left **empty**. Promotion therefore
**requires** a concrete trigger (authored or confirmed by you), so a learned
guardrail can never silently flood unrelated prompts.

## Governance & determinism

- **Off by default.** Set `learning.propose_guardrails: true` in
  `.maestro/settings.yaml` to enable proposal generation.
- **Propose ≠ apply.** Proposals are inert files; only `maestro learn promote`
  changes future behavior, and it produces an ordinary, reviewable skill.
- **Auditable.** `.maestro/proposals/` is the one part of `.maestro/` that is
  **not** gitignored, so proposals (and rejections, kept as records) can be
  reviewed — and optionally committed — as a diff. Each carries `source_runs`
  provenance back to the runs that produced it.
- **Deterministic.** Fingerprinting and distillation use no LLM, so the same
  failures always produce the same proposal.

## Enable it

```yaml
# .maestro/settings.yaml
learning:
  propose_guardrails: true   # guardrails from failed runs
  synthesize_skills: true    # playbook drafts from verified, complex runs
```

Either flag is independent and off by default. Run as usual; then check
`maestro learn list`.
