# Reference agent-runtime absorption (Round 0)

Status: design / boundary-setting · Owner: maestro · Reviewer: cross-review

Scope: decide which **general runtime mechanisms** from a reference TypeScript
agent-runtime are worth absorbing into Repo Maestro as transferable primitives —
and, equally important, which are explicitly out of scope. This is a Round 0
**absorption document only**: no feature code, one docs commit. Each candidate
below is abstracted to a portable primitive and mapped onto a Maestro surface
that already exists ([F-110](F-110-FINDING-LEDGER-DESIGN.md) findings,
[F-111](F-111-PLAN-PREVIEW-ISSUE-ENVELOPE-DESIGN.md) preview,
[F-112](F-112-RUN-MONITOR-PROJECTION-DESIGN.md) monitor,
[F-114](F-114-SPECIALIST-AGENT-PROFILES-DESIGN.md) specialist profiles).

Repo Maestro stays what it is: a local-first, dependency-ordered, **auditable
multi-repo / multi-agent workflow runtime**. We absorb *shapes and contracts*
(event envelope, context-layer inspectability, resume guardrails, capability
descriptor) — never an engine, a transport, a product, or a vendor SDK.

## Non-goals (hard)

- NOT copying the reference runtime's engine, server, daemon, web client, or any
  vendor/protocol SDK. We borrow the *abstract primitive*, re-implemented in
  Rust against Maestro's own types.
- NOT reproducing or evaluating the reference product. This document names no
  real project, brand, path, transcript, model integration, or transport. It
  describes general agent-runtime patterns only.
- NOT adding a live socket/RPC transport or a long-running agent daemon to
  Maestro in this round. F-115 defines a *shape* (an envelope + a closed event
  taxonomy), not a wire protocol or a server.
- NOT making any borrowed event stream the system of record. Maestro's authority
  stays where it is: `RUN_STATE.json` (the snapshot) and `findings.ndjson` (the
  durable audit ledger, F-110). Borrowed primitives are transport/projection
  shapes layered *on top* of that authority, never a replacement for it.

## Fit-gap matrix

General runtimes that drive an agent loop converge on four mechanisms. Mapping
each to what Maestro has today exposes the gap each candidate would close.

| Reference mechanism (abstracted) | Maestro today | Round-0 candidate | Out of scope (now) |
|---|---|---|---|
| Heterogeneous live events (text delta, tool start/result, approval ask, usage tick, lifecycle status) normalized into one sequenced envelope | three *separate* shapes: polled `RUN_STATE.json` snapshot over an SSE tick, append-only `findings.ndjson`, per-task logs/trajectories | **F-115** — one typed event envelope (sequence + status + display metadata + closed `kind` set) that all producers/consumers share | live wire transport, agent daemon, vendor event SDK |
| Model prompt assembled from layered sources (policy / tool descriptors / memory / conversation / ephemeral) with per-layer provenance | prompt/context assembled per run; no inspectable composition view | **F-116** — read-only "context composition" projection (which layers contributed, order, token weight, provenance) | exposing raw layer content anywhere external |
| Resume a session from prior state, guarded against drift / partial writes | `resume` exists (cycle borrow) but trusts the on-disk state | **F-117** — a resume descriptor (run identity / schema version / last-settled sequence) + a validation that refuses unsafe resume | cross-host session migration, server-side session store |
| Runtime declares capabilities (tools / models / limits) as a descriptor and applies intercept rules (pre/post gate) uniformly | per-agent capability is implicit in specialist profiles (F-114); the risk gate is one bespoke gate | **F-118** — machine-readable capability descriptor + a uniform intercept-rule layer | plugin marketplace, dynamic capability negotiation |

## Borrowed primitives (candidate order F-115 > F-116 > F-117 > F-118)

Round 0 commits to *naming and bounding* these four. Only F-115 is proposed for
the first implementation slot; F-116–F-118 are sketched so the fit-gap is honest,
and are sequenced behind F-115 because each is cheaper once the envelope exists.

### F-115 — Runtime Event Envelope + Stream Normalization (first candidate)

**The primitive.** A runtime that drives an agent emits a high-volume, mixed
stream: assistant text deltas, tool invocations and their results, approval
requests, usage/cost ticks, and lifecycle/status transitions. The transferable
idea is a **single normalized envelope**: every event carries a monotonic
`sequence`, a coarse `status`/lifecycle tag, optional `display` metadata, and a
typed payload discriminated by a *small closed set of kinds* (e.g.
`text | tool | approval | usage | status`). Consumers subscribe to one stream and
match on `kind`, instead of stitching together N bespoke channels.

**Why it goes first.** It is the closest neighbour to what Maestro just shipped.
F-112's monitor is a read-only *projection* of `RUN_STATE.json` + F-110 findings;
F-110 findings are discrete durable records; logs/trajectories are a third shape.
Maestro has **no unified live event shape** — the TUI header, the web monitor,
the findings producers, and the recorder each reason about progress differently.
F-115 gives all of them one envelope to emit and consume. Every later candidate
(context composition, resume sequence, intercept events) is naturally expressed
*as* envelope events, so building the envelope first lowers their cost.

**What F-115 does NOT do.**
- It does **not** replace `RUN_STATE.json`. The snapshot stays the authoritative
  current state; the envelope is the incremental transport the snapshot could be
  rebuilt from, not a second source of truth.
- It does **not** replace findings (F-110). Findings remain the durable, audited
  record that survives a run. Envelope events are ephemeral live transport that
  *may be projected into* a finding, but are never themselves the audit ledger.
- It does **not** add a wire protocol, socket, or daemon. Round 1 (if approved)
  defines the Rust envelope type + the closed `kind` taxonomy + the
  sequence/status contract, and adapts existing in-process producers (executor
  steps; risk/refute/doctor finding producers) to also emit envelope events.
- It does **not** emit absolute paths or raw model content — same discipline as
  F-112 (run-relative refs only; `display` metadata is labels/counts, not bodies).

**Boundary vs existing event / findings / monitor.**
- *Envelope vs findings (F-110):* findings are **durable + audited + queryable
  after the run**; the envelope is the **live, incremental** stream during the
  run. Direction is one-way: a producer may turn a significant envelope event
  into a finding; a finding is never downgraded to an envelope event.
- *Envelope vs monitor (F-112):* the monitor is a **periodic aggregate
  projection** (counts, progress buckets, task refs) computed on read; the
  envelope is the **per-event delta** the monitor could be folded from. F-115 and
  F-112 are the two ends of the same axis — stream vs snapshot — and must agree
  on the same `status`/lifecycle vocabulary (the 7-bucket progress model F-112
  already locked) so a folded stream and a fresh projection never disagree.
- *Envelope vs SSE tick:* today's `/api/events` SSE emits an opaque "something
  changed, re-read state" tick. F-115 would let that channel carry *typed* events
  with a sequence, so a client can apply deltas instead of re-fetching the whole
  snapshot — an additive upgrade, not a breaking change.

### F-116 — Prompt Context Layer Debug View

**The primitive.** The runtime assembles the model prompt from ordered layers
(system/policy, tool descriptors, memory/context, conversation, ephemeral). The
transferable idea is making that layering **inspectable**: a read-only view of
which layers contributed, in what order, their token weight, and their
provenance — without exposing raw layer content. Maps onto **F-114** (each
specialist profile is a distinct layer stack worth diffing) and **F-112** (a
per-task "context composition" panel). Privacy rule carries over verbatim:
provenance + token counts only; never raw layer bodies in any exported surface.

### F-117 — Session / Resume Guardrails

**The primitive.** Resuming a session needs guardrails, not blind trust: a resume
descriptor recording run identity, schema version, and last-settled sequence, plus
a validation that **refuses** resume on schema drift, version mismatch, or a
partial/torn write. Maps onto Maestro's existing `resume` and onto **F-112**'s run
identity + (once F-115 lands) the last-settled `sequence`. Turns resume from a
convenience into an auditable, fail-closed operation.

### F-118 — Runtime Capability Descriptor / Intercept Rules

**The primitive.** The runtime declares capabilities (tools, models, limits) as a
machine-readable descriptor and applies intercept rules (pre/post hooks, policy
gates) at one uniform point. Maps onto **F-114** (generalize implicit per-agent
capability into an explicit descriptor) and the existing risk gate (generalize
one bespoke gate into a uniform intercept layer). Lowest priority: it is the most
invasive and benefits most from F-115's events being in place first.

## Privacy boundary

This absorption is bounded by the project's zero-tolerance privacy hardline. The
boundary is enforced two ways: what may enter the document, and a count-only
local inventory of the reference workspace.

**Document rules (this file obeys all):**
- No real project name, internal brand, or product name. The source is referred
  to only as "the reference agent-runtime".
- No absolute paths, no environment keys, no raw tokens/credentials.
- No transcript, log, or session-context excerpts; no internal module/package
  names; no file listings copied from the reference workspace.
- Only general agent-runtime patterns and Maestro-side type/shape proposals.

**Count-only local inventory (generic categories; no words/paths/lists quoted).**
A read-only scan of the reference workspace was run to confirm what must *not*
leak. Results are reported as generic category + presence only — the matched
words, paths, and file lists were never read into this document or any channel:

| Category | Present in reference workspace | In this document |
|---|---|---|
| `internal brand/reference` | yes | none |
| `absolute path` | yes (source-local) | none |
| `credential/token-like` | yes (config/type surface) | none |
| `private integration` | yes | none |
| `raw transcript/log` | yes (session-context dir) | none |
| `workspace-specific naming` | yes | none |

**Verification gate.** This document is committed docs-only in a single commit
and then scanned at release scope (`SECRET_SCAN_NO_EXCLUSIONS=1`, private
wordlist loaded) before the absorption summary is reported. Any future
implementation round (F-115 onward) re-runs the same gate.

## Round 0 outcome

Round 0 produces this document and nothing else. The recommended next step is a
dedicated **F-115 design** (the runtime event envelope + closed `kind` taxonomy +
sequence/status contract, agreeing with F-112's progress vocabulary), to be
specified design-first before any code. F-116–F-118 stay sketched until F-115 is
in place.
