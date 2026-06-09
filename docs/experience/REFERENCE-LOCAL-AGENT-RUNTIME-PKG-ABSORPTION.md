# Reference local agent-runtime package — Round 0 absorption (docs-only)

Status: Round 0 / docs-only · Owner: dali design / dafu writing

This is a **documentation-only** round. It adds NO feature code. It records which
**local, non-cloud** primitives from a reference agent-runtime package are worth
borrowing into maestro, how each maps to an existing maestro surface (fit-gap),
the non-goals we explicitly will not absorb, and the privacy boundary the whole
absorption runs under. It is the design anchor for the F-118…F-122 slices that
follow; each gets its own design doc before any code.

## Source

The reference is a **local agent-runtime package** distributed as built
JavaScript over two release channels. It was read statically only, at the level
of module boundaries and observable mechanisms. This document never names the
package, its registry/URL, its internal modules, or any source excerpt; it
records only the generic, reimplementable mechanism. See **Privacy boundary**.

It is a *runtime/control layer*, not an agent execution kernel — maestro keeps
its own scheduler, adapters, and DAG. We borrow primitives, not the engine.

## Non-goals (explicitly NOT absorbed)

These would drag maestro from a local-first tool toward a hosted platform, so
they are out of scope for the whole absorption:

- cloud device registration, a remote device daemon, or a websocket control
  plane to a server;
- SSO / RBAC, personal access tokens (PAT), member invites, multi-tenant control
  surfaces;
- a **bot / chat / message entry**. maestro does not grow a chat product here. If
  bot / group-collaboration is ever wanted, it is a separate effort against a
  dedicated bot-collaboration reference — NOT borrowed from this local runtime
  package.
- self-update / telemetry as a core capability. At most a local `doctor`
  "version hint", never background reporting.

## Borrowed primitives (local, non-cloud)

Each primitive is reduced to a generic mechanism + the maestro surface it lands
on. Ordering follows the proposed implementation sequence.

### 1. Local runtime capability + health descriptor → **F-118** (first)

- Discover the locally-available agents/providers, their models and default
  parameters, and whether each is fully configured.
- Run a minimal, **timed** probe to produce a live local health status — not just
  a static read of config files.
- Surface it machine-readably: a `maestro doctor runtime` view, a WebUI health
  strip, and MCP/CLI JSON.

Why first: it sits closest to the existing `doctor` + WebUI surfaces and does not
touch executor behavior, so it is the lowest-risk, highest-leverage first cut. It
also subsumes the earlier capability-descriptor direction.

### 2. Local session ownership / control guard → **F-119** (after F-117 resume)

- Decide whether a session may be **reused** from a stable signature (cwd,
  provider, model, permission mode, instructions, skills, …); any change forces a
  reopen rather than silently reusing a drifted session.
- A **busy** session cannot be taken over by another run; the first-turn prompt is
  deduplicated so a resume/retry can't double-send.
- Maps to a maestro runtime **session guard** layered after the F-117 resume
  guardrails (resume answers "is this run safe to seed from"; this answers "is
  this session safe to reuse / who owns it now").

### 3. Event queue ack / replay / backpressure → **F-120**

- Every event carries a sequence; the consumer acks a high-water mark.
  Terminal / permission / resume-critical events are **never dropped**;
  low-value activity events may be rate-limited or shed.
- Complements F-115: the typed event stream already has a durable ledger, but no
  consumer-facing local pending/ack/backpressure layer on top of it.

### 4. Skill / profile visibility inventory (budgeted) → **F-121**

- When scanning local skills, apply a **budget**: max file count, max bytes, max
  depth, skip sensitive dirs/files, read only frontmatter / a summary — never the
  full body.
- Maps to F-114/F-116: a WebUI can explain "which skills this specialist can see"
  without ever rendering skill bodies.

### 5. Artifact / log privacy guard → **F-122**

- An artifact candidate is accepted only when it is inside the workspace, has a
  previewable extension, is size-capped, and its realpath does not escape the
  workspace.
- Log reads are uniformly capped (lines + bytes) and redacted for
  bearer/basic/token/secret/password/cookie/path-like values.
- Strengthens `doctor`, run-evidence, and the WebUI log/artifact panels.

### 6. Local agent/run activity store (supporting)

- Per agent/profile, keep a local **recent-run summary**: run_count, last_run,
  provider/model, an instruction *hash* — never the raw prompt.
- Not a standalone slice; it feeds the F-119 session view and the F-121 profile
  visibility so a specialist profile reads like an operable *local* capability —
  still with no cloud member/device/chat surface.

## Fit-gap summary

| Primitive | Existing maestro surface | Gap to close |
|---|---|---|
| Capability + health (F-118) | `doctor` checks; F-114 profiles; provider config | doctor reads config, doesn't *probe*; no machine-readable runtime health |
| Session ownership (F-119) | F-117 resume guard; adapter `resume_chat_id` | no signature-based reuse/own guard; double-send not deduped at runtime |
| Event ack/backpressure (F-120) | F-115 typed event ledger + per-run SSE | no consumer ack high-water / drop-policy / backpressure |
| Skill inventory (F-121) | F-114 profiles, F-116 context manifest | skills surfaced via YAML/CLI; no budgeted read-only inventory view |
| Artifact/log privacy (F-122) | run evidence; WebUI log/artifact panels; existing redaction | guards are scattered; no single workspace-scoped + size-capped + redacted policy |
| Activity store (supports F-119/121) | RUN_STATE / findings / F-116 | no per-profile recent-run rollup (hash-only) |

## Privacy boundary

This absorption runs under the standing hardline. In git **and** in chat:

- no package name, registry/internal URL, module name, or source excerpt;
- no token / env var / key / secret, no real path, no platform/brand/org name;
- tests and examples use neutral fixtures only;
- when reporting progress, give category + count, never raw borrowed content.

The reference is named only as "a local agent-runtime package". Mechanisms are
described generically so they are reimplementable from this document alone,
without reference to the original artifact.

## Implementation order

This sequence starts **after F-UI-001 establishes the operator-console
information architecture**; F-118 is the first runtime slice, not the next global
task. It lands as the first data source of the new Dashboard.

`F-118 capability + health` → `F-119 session ownership / control` →
`F-120 event ack / backpressure` → `F-121 skill inventory` →
`F-122 artifact / log privacy`.

F-118 is the first cut: closest to the current `doctor` / WebUI surfaces, highest
engineering value, and it does not change executor behavior. Each later slice
gets its own design doc (dali) before implementation (dafu), same design-first
cadence and private-only policy as the F-115…F-117 cycle.
