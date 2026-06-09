# F-127 — PM-to-Delivery Flow (design v3 → F-127a GO'd, in implementation)

Status: **v3 design accepted; sliced 3 ways; F-127a in implementation.** See §10 for
the per-slice scope. New arc (高鹏): support the full PM
lifecycle "需求提出 → 落地" inside Maestro as a standardized, **auditable**
transformation. v3 is aligned to 大力's conditional confirmation + external
benchmarking.

Thesis: the gap is **not a task system** — it's a single `DeliverySpec` that binds
PM requirement, PRD, PLAN, run, evidence, acceptance, and closeout together, with a
gated transformation chain (not "PM 提一句 → AI 拆任务 → 直接开发"). Maestro is the
**orchestration + audit layer over F-122~F-126**, never a replacement for
RUN_STATE / events / GitHub / CI, and never a workflow builder / canvas.

## 1. Standard process (gated transformation chain)

Intake (建档, no judgement) → Triage (初筛: dup/reject/park/discovery, +reason — no
requirement black hole) → Clarify (用户/问题/成功指标/边界/非目标/风险依赖; open
questions block) → Spec/Shape (PRD/pitch/acceptance/non-goals/rollout/risk +
appetite) → Prioritize/Approve (value/cost/risk/resource/bet) → Plan (DAG, owner,
acceptance cmds, F-122 pin) → Execute (reuse run/gate/evidence/tool-policy/CI) →
Accept (auto + PM verdict) → Closeout (commit/CI/review/doc/evidence/debt/rollout).

## 2. External benchmark — what we absorb (and don't)

| source | absorbed design point | maps to |
|---|---|---|
| Jira Product Discovery + Jira | Discovery item ↔ Delivery ticket **separate but strongly linked**; the request tracks delivery progress | `DeliverySpec` binds run/tasks by ref — **never PM-requirement-as-task** |
| Productboard | feedback/insight bound to feature; **evidence drives priority** | Feishu discussion / doc segment / customer feedback / CI / review become `*_refs` — **never copy the body** |
| Aha! Roadmaps | upstream roadmap ↔ downstream tool **two-way sync, not replacement** | Maestro is orchestration + audit only; GitHub/CI stay the execution truth |
| Linear Customer Requests | lightweight `Request → issue` link so dev sees "why" | keep v1 light (`delivery_id ↔ run_id`), no heavy PM platform |
| GitHub Issues/Projects | execution closure lives next to issue/PR/CI/code | execution stays on GitHub/CI; Maestro traces requirement→execution |
| Azure Boards | Epic/Feature/Story/Task **roll-up** | support request → spec/feature → task-DAG roll-up view (v1 shallow) |
| Basecamp Shape Up | **shaping then betting** — unshaped work doesn't reach dev; appetite + bet | the Spec/Shape stage (`appetite`) + the spec_confirm/bet before Plan |

**Do NOT absorb:** generic workflow builder / canvas; PM-requirement == task;
replacing RUN_STATE/events/GitHub/CI; complex prioritization algorithms first;
AI auto-crossing PM thresholds; Feishu doc as runtime truth; LLM-autonomy loop.

## 3. State machine — 7 states + 2 recorded thresholds

Keeps the 7 primary states; Triage and Prioritize/Bet are **recorded decisions**
within the chain (with bypass states), and the two boundaries 大力 called out are
**recorded thresholds, NOT a new gate engine** — they are audit rows + a guard that
refuses the forward transition, reusing the existing approval-marker machinery.

```
intake → clarify → spec → plan → execute → accept → closeout
                       │ spec_confirm        │ pm_accept
 bypass: rejected / parked / duplicate / changes_requested / cancelled
```

Per-transition contract:

| transition | input artifact | output artifact | human decision? | reused gate / mechanism |
|---|---|---|---|---|
| → intake | PM raw text + source (Feishu/doc) | `DeliverySpec.intake` (+`source_refs`) | no — 建档 only | — |
| intake → clarify | intake | `triage` decision (+reason) | **yes — Triage**: accepted_for_discovery / parked / rejected / duplicate | recorded decision (no runtime gate) |
| clarify → spec | clarify questions | `clarify` {Q/A, decisions} | **yes — answer blocking questions** | **Clarify guard**: any open blocking question refuses the transition (recorded) |
| **spec → plan** | spec (PRD/acceptance/non-goals/appetite) | `spec` + **`spec_confirm`** {by, at} | **yes — PM/owner confirm + bet** | **`spec_confirm` recorded threshold** — an unconfirmed PRD MUST NOT auto-generate a plan (NOT a gate engine; an audit row + guard) |
| plan → execute | confirmed spec | `PLAN.yaml` + **F-122 PlanPreview pin** | optional | reuse **F-122 pin + F-123 plan gate** |
| execute → accept | run + tasks | `run_id` binding, evidence/findings refs | no — automatic | reuse **F-123 outcome gate + F-126 policy gate + F-124 evidence** |
| **accept → closeout** | `acceptance_results` + PM verdict | `accept` + **`pm_accept`** {verdict} | **yes — PM verdict**: accepted / changes_requested / partial / rejected | **`pm_accept` recorded threshold** — CI-green alone is NOT "delivered" (NOT a gate engine; an audit row + guard) |
| closeout | commit/CI/review/evidence | `closeout` + doc/kanban write-back | no | reuse **SOP + F-124 evidence + `lark-doc`** |

## 4. Schema — single record `DeliverySpec` (`maestro.delivery_spec.v1`)

One machine-readable file per requirement, **refs-first** (never copies the Feishu
body or run state), persisted at **`.maestro/deliveries/<delivery_id>/DELIVERY.json`**
— the v1 landing spot. A delivery **precedes** a run and may span **multiple
runs/retries**, so it does NOT live in a run dir; the run is only ref'd.

```
DeliverySpec {
  schema_version, delivery_id, stage,            // the 7-state cursor (+ bypass)
  proposer, created_at,
  intake:   { objective, background, target_users?, business_goal?, constraints[], time_constraint?, source_refs[] },
  triage:   { decision: accepted_for_discovery|parked|rejected|duplicate, reason, owner?, duplicate_of? },
  clarify:  { questions[] {q, blocking, answer?, by?, at?}, decisions[], rejected[], deferred[] },
  spec:     { prd, pitch?, acceptance[] (== Goal.acceptance shape), non_goals[], rollout?, risks[], appetite? },
  spec_confirm: { confirmed: bool, by?, at?, notes? },          // threshold: spec -> plan
  plan:     { plan_path: "PLAN.yaml", plan_hash, preview_ref },  // F-122 pin, ref only
  execute:  { run_id?, status? },                                // bind to the run (ref)
  accept:   { verdict: pending|accepted|changes_requested|partial|rejected, acceptance_results_ref?, debt[] },
  pm_accept: { by?, at?, notes? },                               // threshold: accept -> closeout
  evidence_refs[],                                               // Productboard: feedback/doc-segment/CI/review refs (run-local or external uri)
  closeout: { commits[], ci[], reviews[], doc_revisions[], evidence_refs[] },  // SOP + F-124
  audit: { stage_history[] {stage, at, by, reason?} }
}
```

`source_refs` / `evidence_refs` / `plan_hash` reuse F-124 run-local rules (run-local
path OR external uri; no absolute leak). **Evidence chain unbroken**: intake source
→ clarify decisions → spec acceptance → plan_hash → run evidence → closeout, all by id.

## 5. Reuse vs new

**Reuse:** `work`/`plan::synthesize` (Spec→Plan), `Goal`/`Acceptance` (Spec/Accept),
`deliberate`/`discuss` (Clarify reasoning, later), F-122 `PlanPreview`, F-123
`ReviewGate`, F-124 evidence/`ArtifactRef`, F-126 tool-policy gate, the closeout SOP,
`lark-doc`. **New (small):** the `DeliverySpec` record + 7-state cursor; the discovery
fields (intake/triage/clarify/spec/appetite); the spec_confirm / pm_accept recorded
thresholds; the `delivery_id ↔ run_id` binding; a read-only **stage projection**
(stage / blocked-on / linked run·tasks·findings·evidence·debt) mirroring F-112/F-123;
a doc→DeliverySpec intake parser and a run→closeout writer-back.

## 6. Minimal v1 — the requirement-transformation closed loop (not a platform)

1. **Intake**: parse a Feishu/doc/markdown requirement → `DeliverySpec`
   (`.maestro/deliveries/<id>/DELIVERY.json`, stage `intake`), keeping raw text +
   `source_refs`. Read-only on the doc.
2. **Clarify + Spec**: AI authors clarify questions + a Spec draft + acceptance
   criteria (recorded as data; AI output is *proposed*). Clarify guard blocks open
   questions; `spec_confirm` is the human "this is what we build" audit row.
3. **Plan**: after `spec_confirm`, generate `PLAN.yaml` + acceptance checklist by
   reusing `work`/`plan::synthesize` + the `Goal` block; pin via F-122; ref the plan.
4. **Execute**: run through the existing run/gate/evidence path (F-122~126); bind
   `run_id`. **Accept**: auto acceptance + `pm_accept` verdict.
5. **Closeout**: auto-produce the closeout (SOP + F-124) and write it back to the
   doc/kanban via `lark-doc`; advance stage to `closed`.

UI/API v1 = **read-only projection only** (current stage, blocked-on gate, linked
run/tasks/evidence/debt). No editing UI, no canvas.

## 7. Do-not-absorb

- No generic visual workflow builder / node canvas.
- PM requirement is NOT an engineering task — `DeliverySpec` refs run/tasks.
- No replacement of `RUN_STATE.json` / `events.ndjson` / GitHub / CI — refs only.
- No complex prioritization/scoring algorithm in v1.
- AI may author spec/plan but may NOT auto-cross Triage / spec_confirm / pm_accept.
- Feishu doc is intake-in / closeout-out only — never a runtime source of truth.
- Don't rewrite `work`/`plan::synthesize` / `deliberate` — wrap them.

## 8. Test matrix + risk table

**Tests:** intake parse (valid doc → DeliverySpec; malformed → explicit error, never
a silent empty spec); state transitions (legal vs illegal, incl. bypass); the two
thresholds (open clarify question refuses spec→plan; unconfirmed spec refuses plan
generation; CI-green alone does NOT auto-cross accept→closeout); plan generation
reuses the synthesizer (DeliverySpec acceptance == PLAN `Goal.acceptance`);
`delivery_id ↔ run_id` round-trip; closeout writer captures commit/CI/review/evidence;
read-only stage projection three-state (present / absent / corrupt → explicit error,
never silent "done"); refs are run-local or uri (no absolute leak); Rust↔TS parity.

| risk | mitigation |
|---|---|
| scope creep into a PM platform | v1 = the closed loop + read-only projection only; prioritization/roadmap deferred |
| DeliverySpec drifts from the run | bind `delivery_id ↔ run_id` + reuse F-122 `plan_hash`; projection reads live run state |
| requirement black hole | every Triage out-state records a reason; nothing drops silently |
| AI silently crossing a PM threshold | spec_confirm / pm_accept are explicit human audit rows + a guard; AI output is "proposed" |
| copying the Feishu body / run state | refs-first by contract; only ids/paths/uris stored |
| CI-green mistaken for "delivered" | pm_accept threshold — delivery requires the PM verdict, not just green CI |

## 9. Decisions confirmed by 大力 (conditional) + open

Confirmed: 7-state machine + `spec_confirm`/`pm_accept` recorded thresholds; single
refs-first `DeliverySpec`; v1 four-step closed loop; the Do-not-absorb list;
`.maestro/deliveries/<delivery_id>/DELIVERY.json` landing spot.

Resolved at GO (大力):
1. The loop is sliced into 3 commits — **F-127a = schema + store + intake parser +
   read-only stage projection** first; Plan/Execute wiring (F-127b) and Accept/
   Closeout + Feishu write-back (F-127c) are fast-follow slices.
2. The intake parser is **best-effort extraction + clarify-on-missing**, never a fixed
   template and never a silent fill.

## 10. Slice plan & F-127a scope (GO'd)

| Slice | Scope | Status |
|-------|-------|--------|
| **F-127a** | `DeliverySpec` schema (`maestro.delivery_spec.v1`) + file store (create/read/list/update_stage, transition-validated, corrupt→explicit error) + intake parser (markdown/plain → spec; missing required → blocking clarify) + read-only stage projection (`DeliveryView`: stage / blocked_on / source_refs / run_id / plan ref / accept verdict / closeout summary; present/absent/corrupt) + `maestro delivery intake\|show\|ls` CLI | **in implementation** |
| F-127b | PLAN generation from a confirmed spec + run linkage (after `spec_confirm`) | not started |
| F-127c | Accept verdict + closeout + Feishu write-back (after `pm_accept`) | not started |

**F-127a does NOT** generate `PLAN.yaml`, start a run, write back to Feishu, build any
edit UI / canvas / workflow builder, replace `RUN_STATE` / events, or let AI auto-cross
`spec_confirm` / `pm_accept`. The inbound doc is a `source_ref` only — its body is never
copied into `DELIVERY.json`. Refs are validated run-local: no absolute path, no `file:`
uri (reusing F-124's `schema::artifacts` validators); a name-only ref is an allowed
opaque named reference.

F-127a landing: `.maestro/deliveries/<delivery_id>/DELIVERY.json`. Tests: schema serde
round-trip; legal/illegal stage transitions; ref-violation (absolute path + `file:` uri
rejected); parser structured/frontmatter → spec; missing-required → blocking clarify
(not silent); projection present/absent/corrupt; CLI end-to-end incl. corrupt→exit-nonzero.
