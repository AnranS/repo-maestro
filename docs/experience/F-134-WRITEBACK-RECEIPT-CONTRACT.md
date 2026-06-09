# F-134 — Delivery Feishu write-back receipt — contract

Status: **IMPLEMENTED** (大力 GO'd all 6 pins + hard constraints). Single commit:
`Writeback` gains `idempotency_key` + `receipt{message_ref?,doc_revision?,error?,at}`;
`WritebackStatus` += `posted`/`failed`; `OutboundReply` gains `delivery_id` +
`idempotency_key`. At emit the closeout computes `stable_hash_bytes` of the canonical
payload `[delivery_id, run_id, doc_uri, event_kind, title, body]` (`\x1f`-joined, trimmed
title/body, body never stored) and persists the SAME key on both the `OutboundReply` and
`closeout.writeback.idempotency_key`. New drainer-callback `POST
/api/deliveries/:id/writeback-receipt` records the receipt — maestro posts nothing.
`record_writeback_receipt` validates all-before-save: `posted` needs a durable
`message_ref`/`doc_revision` (else 409); `failed` needs a non-empty `error` (else 409);
the receipt key must match the persisted key (else 409 stale); reconcile = same-terminal
same-payload no-op / `failed→posted` allowed / `posted` immutable (diff → 409).
DeliveryView gains `writeback_receipt`; the closeout UI shows posted (message link + doc
rev) / failed (error) / intent-emitted (pending). Tests: 5 server_api covering 大力's
full matrix (dual-key emit, posted/failed, reconcile incl. posted-immutable 409,
key-mismatch/no-ref/no-error/no-intent/invalid-status, three-state) + 4 light/dark
screenshots. Do-not respected: no in-process Feishu write, no body copy, drain_once
untouched. Full gate green.

---

Original contract (design-only) follows. Sixth Delivery slice. Today a
closeout `--writeback` only EMITS an intent (an `OutboundReply` appended to the run's
`outbound_replies.ndjson`) and records `Writeback{status: intent_emitted}` — there is NO
machine-readable loop for "did the external Feishu post actually succeed / fail / what's
the posted message id (receipt) / can we reconcile after a restart". F-134 closes that
loop by **recording the receipt the external drainer reports back** — maestro still NEVER
posts Feishu in-process. Absorbs the botmux side-effect protocol shape
(attempted→emitted→posted/failed, idempotency-keyed, reconcilable, no blind retry).

## 1. Seam map (current)
| Seam | Where | Note |
|------|-------|------|
| `OutboundReply` | `src/channel/outbound.rs:7` — `{channel, run_id, event_kind, title, body, attachments}` | no idempotency key today |
| append | `src/channel/subscribe.rs:51` `append_outbound_reply` → `run_dir/outbound_replies.ndjson` | append-only, no dedup |
| closeout emit | `src/delivery.rs:~1231` builds the `delivery.closeout` `OutboundReply` (doc uri from `intake.source_refs`) + `Writeback{IntentEmitted, at, doc_ref}` | the emit point |
| `Writeback` / `WritebackStatus` | `src/schema/delivery.rs:309/320` — `{Skipped, IntentEmitted}` | two-state; no receipt |
| in-process drain | `src/channel/drain.rs:43` `drain_once` (cursor `channel_drain_cursor.json`) | **for OTHER channels** — the delivery closeout is drained EXTERNALLY (lark-* skill); maestro doesn't post Feishu |
| idempotency helper | `src/file_guard.rs:12` `stable_hash_bytes` (FNV1a; reused by plan hash + chat turn) | reuse for the receipt key |
| handler pattern | `handlers/deliveries.rs` `read_for_action` (400/404/500) + `refused`→409 + `actor_of` | mirror |

## 2. The receipt lifecycle (botmux protocol → maestro states)
maestro records the TERMINAL receipt the external drainer reports; the transient
drainer-side steps stay drainer-side.
| botmux | maestro `WritebackStatus` | who records |
|--------|---------------------------|-------------|
| (no writeback) | `skipped` | closeout |
| attempted + emitted | `intent_emitted` | closeout (today) |
| drained | *(drainer-side, not recorded in v1)* | — |
| **posted** | `posted` + receipt refs | the receipt callback |
| **failed** | `failed` + error | the receipt callback |
| reconciled | *(idempotent re-report, not a new status)* | the receipt callback |

## 3. Schema / API / DeliveryView / UI
- **`Writeback`** (richer, refs-first): add `idempotency_key: String` (set at emit),
  `receipt: Option<WritebackReceipt>` where `WritebackReceipt { message_ref?: String,
  doc_revision?: String, error?: String, at: String }`. `WritebackStatus` += `Posted`,
  `Failed`.
- **`OutboundReply`**: add `idempotency_key: Option<String>` (generic dedup key; the
  closeout sets it = the writeback input hash; run-event emits leave it `None`). So the
  external drainer reads the key from the queue and echoes it back on the receipt.
- **`stable_hash_bytes`** the emit content: `idempotency_key =
  stable_hash_bytes(delivery_id | run_id | event_kind | title | body | doc_uri)` — a
  stable `inputHash` identifying THIS intent (so a stale receipt after a re-closeout/reopen
  can't be applied).
- **API** (the external drainer calls back): `POST /api/deliveries/:id/writeback-receipt`
  `{ idempotency_key, status: "posted"|"failed", message_ref?, doc_revision?, error? }` →
  `200 DeliveryView` | three-state | `409`. maestro **only records** what it's told — it
  posts nothing.
- **DeliveryView**: `writeback_status` now spans `skipped|intent_emitted|posted|failed`;
  add a small `writeback_receipt: Option<{message_ref?, doc_revision?, error?}>` (refs
  only) so list/detail can show "posted ✓ (msg ref)" / "failed (error)".
- **UI**: the closeout section's write-back line shows the receipt state — `intent
  emitted (pending external post)` / `posted` + a message/doc deep-link / `failed` + the
  error. A receipt-fetch failure shows explicit "unavailable", never silent.

## 4. Idempotency & replay (no blind retry)
- The receipt callback verifies `idempotency_key` matches the recorded
  `Writeback.idempotency_key` (the receipt is for the CURRENT intent) — mismatch → 409
  ("receipt for a stale/mismatched intent").
- **Reconcile (restart-safe)**: re-reporting the SAME terminal state (same key + status +
  refs) → `200` no-op. `failed → posted` (a drainer retry that finally succeeded) is
  allowed. `posted` is terminal: `posted → failed` or `posted → posted` with a DIFFERENT
  `message_ref` → 409 (never un-post, never flip — surfaces a double-post bug rather than
  hiding it).
- maestro never retries the post itself; the external drainer drives retries and reports
  the terminal outcome. The `idempotency_key` is what makes a drainer restart safe.

## 5. Three-state / corrupt (explicit failure, never silent)
`read_for_action` (bad id 400 / missing 404 / corrupt 500). Store refusals → 409: no
write-back intent to receipt (closeout `skipped` / no closeout); idempotency mismatch;
conflicting terminal receipt. Never a silent no-op or a fabricated success.

## 6. Do-not-absorb (v1)
NO in-process Feishu write (maestro records the receipt the external drainer reports);
NO copying the Feishu body (refs only — `message_ref` / `doc_revision`); NO new gate
engine; NO replacing RUN_STATE/events; NO full event ledger (just the closeout write-back
receipt — that's F-137); NO blind retry; NO change to F-131/F-132/F-133; the in-process
`drain_once` (other channels) is untouched.

## 7. Test matrix
- closeout `--writeback` → `Writeback{intent_emitted, idempotency_key set}` + the
  emitted `OutboundReply` carries the same `idempotency_key`.
- receipt `posted` (matching key) → `Writeback{posted, receipt{message_ref, doc_revision}}`;
  DeliveryView `writeback_status == posted`.
- receipt `failed` → `Writeback{failed, receipt{error}}`.
- receipt with a MISMATCHED `idempotency_key` → 409 (stale intent).
- receipt when there is no write-back intent (closeout skipped / not closed out) → 409.
- **reconcile**: re-post the same `posted` receipt → 200 no-op; `failed`→`posted` allowed;
  `posted`→different `message_ref` → 409.
- three-state (bad id / missing / corrupt).
- refs-first: the receipt payload carries no Feishu body (assert no body copy).

## 8. Open decisions for 大力 to pin
1. **Drainer → delivery routing**: add `delivery_id` to the `OutboundReply` for the
   closeout (explicit) vs the drainer resolves `delivery_id` from `RunState.delivery_id`
   (the F-131 back-ref). *(lean: add `delivery_id` to the closeout `OutboundReply` — explicit,
   no extra lookup; the `idempotency_key` field is added regardless.)*
2. **`WritebackStatus` set**: `{skipped, intent_emitted, posted, failed}` (recommended —
   maestro records terminal only) vs also a `drained`/`posting` transient. *(lean: terminal
   only; `drained` deferred.)*
3. **idempotency_key content**: `hash(delivery|run|kind|title|body|doc_uri)` (recommended)
   vs a simpler `delivery_id|round`. *(lean: the content hash — survives nothing-changed
   re-emit, distinguishes a changed intent.)*
4. **Receipt endpoint auth/actor**: local-only, no auth (recommended — consistent with the
   LOCAL-only line; `by` defaults to a drainer id) vs require an actor. *(lean: local, no auth.)*
5. **Reconcile policy**: `failed→posted` allowed + `posted` immutable (recommended) vs
   strict (any terminal is immutable). *(lean: allow the success-after-retry, lock posted.)*
6. **DeliveryView receipt surface**: add `writeback_receipt` (message_ref/doc_revision/error,
   refs-only) to the view (recommended, so the list/detail show it) vs only `writeback_status`.
   *(lean: add the small receipt.)*

## 9. Review axes: seam → §1; lifecycle → §2; schema/API/view/UI → §3; idempotency/replay →
§4; three-state → §5; Do-not → §6; tests → §7; recommended + open → §8. No code until GO.
