// F-128 — read-only Delivery Web UI. Left list (ok rows + visible corrupt stubs) +
// right detail (stage stepper, blocked_on, plan/run linkage, accept, closeout). No
// edit/execute — v1 only surfaces the PM→Delivery chain. Restrained tool-console
// style via the shared tokens; light/dark from the same vars as the graph views.
import { useEffect, useRef, useState, type ReactNode } from "react"
import { Package, ExternalLink, History } from "lucide-react"
import { api } from "../api"
import { StatusChip } from "./ui/StatusChip"
import { StatusBadge, statusToneClasses } from "./ui/StatusBadge"
import { CollapsibleSection } from "./ui/CollapsibleSection"
import { t, useLang } from "../i18n"
import type {
  AcceptVerdict,
  BlockedOn,
  DeliveryListEntry,
  DeliveryStage,
  DeliveryView,
  RunStatusLite,
  TimelineEvent,
  TimelineRef,
  WritebackReceipt,
  WritebackStatus,
} from "../types"

// One item per line, trimmed, blanks dropped — matches the server's clean_tokens /
// parse_evidence_refs (debt items and doc strings can contain spaces, so split on
// newlines, not whitespace).
function splitLines(raw: string): string[] {
  return raw
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
}

type AcceptInput = {
  verdict: AcceptVerdict
  notes?: string
  debt: string[]
  accept_failed_with_debt: boolean
}
type CloseoutInput = {
  commits: string[]
  ci: string[]
  reviews: string[]
  doc_revisions: string[]
  evidence: string[]
  writeback: boolean
}

const FORWARD_STAGES: DeliveryStage[] = [
  "intake",
  "clarify",
  "spec",
  "plan",
  "execute",
  "accept",
  "closeout",
]

const isBypass = (s: DeliveryStage) => !FORWARD_STAGES.includes(s)
const pretty = (s: string) => s.replace(/_/g, " ")

function blockedLabel(b: BlockedOn): { text: string; needsAction: boolean } {
  if (b === "nothing") return { text: "clear", needsAction: false }
  if (b === "spec_confirm") return { text: "awaiting spec confirm", needsAction: true }
  if (b === "pm_accept") return { text: "awaiting PM accept", needsAction: true }
  const n = b.open_clarify_questions.count
  return { text: `${n} clarify question${n === 1 ? "" : "s"}`, needsAction: true }
}

export function DeliveriesView({ onOpenRun }: { onOpenRun: (runId: string) => void }) {
  useLang()
  const [entries, setEntries] = useState<DeliveryListEntry[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<DeliveryView | null>(null)
  const [detailError, setDetailError] = useState<string | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  // F-131: inline live run status for the selected delivery (resolved server-side via
  // execute.run_id or the in-flight RunState.delivery_id back-ref). `runStatusErr` keeps
  // a corrupt/missing linked run VISIBLE (never silently shown as idle).
  const [runStatus, setRunStatus] = useState<RunStatusLite | null>(null)
  const [runStatusErr, setRunStatusErr] = useState<string | null>(null)
  // F-132: the audit timeline for the selected delivery. `timelineErr` keeps a corrupt /
  // inconsistent record VISIBLE (never a silent empty timeline).
  const [timeline, setTimeline] = useState<TimelineEvent[] | null>(null)
  const [timelineErr, setTimelineErr] = useState<string | null>(null)
  // F-129 forward actions + F-130 accept/closeout: which action is in flight + its
  // last error.
  const [busy, setBusy] = useState<
    null | "confirm" | "plan" | "run" | "accept" | "closeout" | "reopen"
  >(null)
  const [actionError, setActionError] = useState<string | null>(null)
  // F-131 live refresh: a tick bumped by the SSE stream (or the poll fallback) drives a
  // silent refetch of list + selected detail + run-status.
  const [tick, setTick] = useState(0)
  const [connected, setConnected] = useState(false)
  const selectedRef = useRef<string | null>(null)
  useEffect(() => {
    selectedRef.current = selectedId
  }, [selectedId])

  async function loadRunStatus(id: string, alive: () => boolean) {
    try {
      const r = await api.deliveryRunStatus(id)
      if (alive()) {
        setRunStatus(r.run)
        setRunStatusErr(null)
      }
    } catch (e) {
      // A corrupt/missing linked run is a 500 — surface it, never silent idle.
      if (alive()) {
        setRunStatus(null)
        setRunStatusErr(e instanceof Error ? e.message : String(e))
      }
    }
  }

  async function loadTimeline(id: string, alive: () => boolean) {
    try {
      const r = await api.deliveryTimeline(id)
      if (alive()) {
        setTimeline(r.events)
        setTimelineErr(null)
      }
    } catch (e) {
      // A corrupt / audit-node-inconsistent record is a 500 — surface it, never empty.
      if (alive()) {
        setTimeline(null)
        setTimelineErr(e instanceof Error ? e.message : String(e))
      }
    }
  }

  async function refreshAfterAction(id: string) {
    const [list, d] = await Promise.all([api.deliveries(), api.delivery(id)])
    setEntries(list)
    setDetail(d)
    const alive = () => selectedRef.current === id
    await Promise.all([loadRunStatus(id, alive), loadTimeline(id, alive)])
  }

  async function runAction(kind: "confirm" | "plan" | "run" | "reopen") {
    if (!selectedId) return
    setBusy(kind)
    setActionError(null)
    try {
      if (kind === "confirm") {
        await api.deliveryConfirmSpec(selectedId)
        await refreshAfterAction(selectedId)
      } else if (kind === "plan") {
        await api.deliveryPlan(selectedId)
        await refreshAfterAction(selectedId)
      } else if (kind === "reopen") {
        // F-133: supersedes the prior round + resets to spec — refetch lands at the
        // re-confirm action.
        await api.deliveryReopen(selectedId)
        await refreshAfterAction(selectedId)
      } else {
        // F-131: run starts detached. Stay on the Delivery detail — the inline live
        // status surfaces the in-flight run (resolved via RunState.delivery_id), no
        // force-jump to Tasks. A deep-link to Tasks stays available in the detail.
        await api.deliveryRun(selectedId)
        await loadRunStatus(selectedId, () => selectedRef.current === selectedId)
      }
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  // F-130: accept records the PM verdict (the run-outcome gate is server-side, so a
  // refusal surfaces as actionError); closeout records evidence + optional write-back.
  // Both refetch on success — the stage advances and the form unmounts.
  async function submitAccept(input: AcceptInput) {
    if (!selectedId) return
    setBusy("accept")
    setActionError(null)
    try {
      await api.deliveryAccept(selectedId, input)
      await refreshAfterAction(selectedId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  async function submitCloseout(input: CloseoutInput) {
    if (!selectedId) return
    setBusy("closeout")
    setActionError(null)
    try {
      await api.deliveryCloseout(selectedId, input)
      await refreshAfterAction(selectedId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  useEffect(() => {
    let cancelled = false
    api
      .deliveries()
      .then((e) => {
        if (!cancelled) {
          setEntries(e)
          setError(null)
        }
      })
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : String(e)))
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      setDetailError(null)
      setRunStatus(null)
      setRunStatusErr(null)
      setTimeline(null)
      setTimelineErr(null)
      return
    }
    let cancelled = false
    setDetailLoading(true)
    setDetail(null)
    setDetailError(null)
    setActionError(null)
    setRunStatus(null)
    setRunStatusErr(null)
    setTimeline(null)
    setTimelineErr(null)
    api
      .delivery(selectedId)
      .then((d) => !cancelled && setDetail(d))
      .catch((e) => !cancelled && setDetailError(e instanceof Error ? e.message : String(e)))
      .finally(() => !cancelled && setDetailLoading(false))
    loadRunStatus(selectedId, () => !cancelled)
    loadTimeline(selectedId, () => !cancelled)
    return () => {
      cancelled = true
    }
  }, [selectedId])

  // F-131: SSE tick (the watcher fires on .maestro/runs + .maestro/deliveries changes).
  // The payload is ignored — it's purely a "something changed, refetch" signal; the
  // status itself comes from the run-status resolver.
  useEffect(() => {
    const es = new EventSource("/api/events")
    const bump = () => {
      setConnected(true)
      setTick((n) => n + 1)
    }
    es.addEventListener("state", bump)
    es.addEventListener("ping", () => setConnected(true))
    es.onerror = () => setConnected(false)
    return () => es.close()
  }, [])

  // 7s poll fallback when the SSE link is down.
  useEffect(() => {
    if (connected) return
    const h = setInterval(() => setTick((n) => n + 1), 7000)
    return () => clearInterval(h)
  }, [connected])

  // Silent live refetch on each tick — list + the selected delivery + its run-status,
  // without the select-time loading flicker.
  useEffect(() => {
    if (tick === 0) return
    const id = selectedRef.current
    let cancelled = false
    api.deliveries().then((e) => !cancelled && setEntries(e)).catch(() => {})
    if (id) {
      api.delivery(id).then((d) => !cancelled && setDetail(d)).catch(() => {})
      const alive = () => !cancelled && selectedRef.current === id
      loadRunStatus(id, alive)
      loadTimeline(id, alive)
    }
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick])

  if (!entries && !error) {
    return (
      <div className="flex-1 flex items-center justify-center text-ink-faint text-sm">
        {t("common.loading")}
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex-1 overflow-auto p-6">
        <div className="max-w-2xl mx-auto rounded-lg border border-red-500/30 bg-red-500/10 p-4 text-sm text-status-danger">
          {t("deliveries.error")}: {error}
        </div>
      </div>
    )
  }
  const list = entries ?? []
  if (list.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto scrollbar-thin">
        <div className="max-w-3xl mx-auto px-6 py-16">
          <div className="flex items-start gap-4 rounded-lg border border-line bg-bg-panel p-6">
            <div className="mt-0.5 flex h-10 w-10 items-center justify-center rounded-md border border-accent/30 bg-accent/10 text-accent">
              <Package size={18} />
            </div>
            <div>
              <h1 className="text-base font-semibold text-ink">{t("deliveries.emptyTitle")}</h1>
              <p className="mt-1 max-w-2xl text-sm leading-6 text-ink-dim">
                {t("deliveries.emptyBody")}
              </p>
            </div>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 flex overflow-hidden">
      <aside className="w-72 shrink-0 border-r border-line bg-bg-panel overflow-y-auto scrollbar-thin">
        {list.map((e) => {
          const active = selectedId === e.delivery_id
          const base = `w-full text-left px-4 py-3 border-b border-line/60 hover:bg-bg-hover transition-colors ${
            active ? "bg-bg-hover" : ""
          }`
          if (e.status === "corrupt") {
            return (
              <button key={e.delivery_id} onClick={() => setSelectedId(e.delivery_id)} className={base}>
                <div className="font-mono text-xs text-ink-faint truncate">{e.delivery_id}</div>
                <StatusBadge tone="danger" className="mt-1">
                  {t("deliveries.corrupt")}
                </StatusBadge>
              </button>
            )
          }
          const blk = blockedLabel(e.view.blocked_on)
          return (
            <button key={e.delivery_id} onClick={() => setSelectedId(e.delivery_id)} className={base}>
              <div className="flex items-center justify-between gap-2">
                <div className="font-mono text-xs text-ink-faint truncate">{e.delivery_id}</div>
                {blk.needsAction && (
                  <span
                    className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400"
                    title={blk.text}
                  />
                )}
              </div>
              <div className="mt-1 text-sm capitalize text-ink">{pretty(e.view.stage)}</div>
            </button>
          )
        })}
      </aside>

      <section className="flex-1 overflow-y-auto scrollbar-thin">
        {!selectedId ? (
          <div className="flex h-full items-center justify-center px-6 text-center text-sm text-ink-faint">
            {t("deliveries.selectOne")}
          </div>
        ) : detailLoading ? (
          <div className="flex h-full items-center justify-center text-sm text-ink-faint">
            {t("common.loading")}
          </div>
        ) : detailError ? (
          <div className="p-6">
            <div className="max-w-2xl rounded-lg border border-red-500/30 bg-red-500/10 p-4 text-sm text-status-danger">
              {t("deliveries.detailError")}: {detailError}
            </div>
          </div>
        ) : detail ? (
          <DeliveryInspector
            v={detail}
            onOpenRun={onOpenRun}
            busy={busy}
            actionError={actionError}
            onAction={runAction}
            onAccept={submitAccept}
            onCloseout={submitCloseout}
            runStatus={runStatus}
            runStatusErr={runStatusErr}
            timeline={timeline}
            timelineErr={timelineErr}
          />
        ) : null}
      </section>
    </div>
  )
}

function DeliveryInspector({
  v,
  onOpenRun,
  busy,
  actionError,
  onAction,
  onAccept,
  onCloseout,
  runStatus,
  runStatusErr,
  timeline,
  timelineErr,
}: {
  v: DeliveryView
  onOpenRun: (runId: string) => void
  busy: null | "confirm" | "plan" | "run" | "accept" | "closeout" | "reopen"
  actionError: string | null
  onAction: (kind: "confirm" | "plan" | "run" | "reopen") => void
  onAccept: (input: AcceptInput) => void
  onCloseout: (input: CloseoutInput) => void
  runStatus: RunStatusLite | null
  runStatusErr: string | null
  timeline: TimelineEvent[] | null
  timelineErr: string | null
}) {
  const blk = blockedLabel(v.blocked_on)
  const bypass = isBypass(v.stage)
  const curIdx = FORWARD_STAGES.indexOf(v.stage)

  // F-131: a run is in flight when the resolver returns a running run (linked or via the
  // back-ref). While one is in flight, the Start-run action is hidden — the page looked
  // "idle" mid-run before (the server already 409s a duplicate launch).
  const runInFlight = runStatus?.status === "running"

  // Forward action available at this stage (derived from stage + blocked_on; no new
  // field). confirm-spec keeps stage=Spec, plan advances Spec→Plan, run Plan→Execute.
  // The `run` action is suppressed while a run is already in flight.
  const action: null | "confirm" | "plan" | "run" =
    v.stage === "spec" && v.blocked_on === "spec_confirm"
      ? "confirm"
      : v.stage === "spec" && v.blocked_on === "nothing"
        ? "plan"
        : v.stage === "plan" && v.blocked_on === "nothing" && !runInFlight
          ? "run"
          : null

  return (
    <div className="mx-auto max-w-3xl space-y-6 px-6 py-6">
      <div className="flex items-center justify-between gap-3">
        <h2 className="font-mono text-sm text-ink">{v.delivery_id}</h2>
        <div className="flex items-center gap-2">
          {/* F-133: which rework round (only once it's been reopened at least once). */}
          {v.round > 1 && (
            <span
              className="rounded border border-line bg-bg-inset px-2 py-0.5 text-[10px] text-ink-faint"
              title={t("deliveries.supersededRounds", { n: v.superseded_count })}
            >
              {t("deliveries.round", { n: v.round })}
            </span>
          )}
          <span
            className={`rounded border px-2 py-0.5 text-xs capitalize ${
              bypass ? statusToneClasses("danger") : statusToneClasses("neutral")
            }`}
          >
            {pretty(v.stage)}
          </span>
        </div>
      </div>

      {action && (
        <Field label={t("deliveries.actions")}>
          <ActionButton
            key={action}
            label={
              action === "confirm"
                ? t("deliveries.confirmSpec")
                : action === "plan"
                  ? t("deliveries.genPlan")
                  : t("deliveries.startRun")
            }
            ask={
              action === "confirm"
                ? t("deliveries.confirmSpecAsk")
                : action === "plan"
                  ? t("deliveries.genPlanAsk")
                  : t("deliveries.startRunAsk")
            }
            danger={action === "run"}
            busy={busy === action}
            disabled={busy !== null}
            onConfirm={() => onAction(action)}
          />
          {actionError && <div className="mt-2 text-xs text-status-danger">{actionError}</div>}
        </Field>
      )}

      {/* F-133: reopen for rework — only at changes_requested. */}
      {v.stage === "changes_requested" && (
        <Field label={t("deliveries.actions")}>
          <ActionButton
            key="reopen"
            label={t("deliveries.reopen")}
            ask={t("deliveries.reopenAsk")}
            busy={busy === "reopen"}
            disabled={busy !== null}
            onConfirm={() => onAction("reopen")}
          />
          {actionError && <div className="mt-2 text-xs text-status-danger">{actionError}</div>}
        </Field>
      )}

      {/* F-130: accept @ execute; closeout @ accept (verdict accepted/partial). Pure
          stage + accept_verdict derivation — no new projection field. */}
      {v.stage === "execute" && (
        <AcceptForm
          busy={busy === "accept"}
          disabled={busy !== null}
          error={actionError}
          onSubmit={onAccept}
        />
      )}
      {v.stage === "accept" &&
        (v.accept_verdict === "accepted" || v.accept_verdict === "partial") && (
          <CloseoutForm
            busy={busy === "closeout"}
            disabled={busy !== null}
            error={actionError}
            onSubmit={onCloseout}
          />
        )}

      <div className="rounded-lg border border-line bg-bg-panel p-4">
        <div className="flex flex-wrap items-center gap-x-1.5 gap-y-2">
          {FORWARD_STAGES.map((s, i) => {
            const reached = !bypass && i <= curIdx
            const current = !bypass && i === curIdx
            return (
              <div key={s} className="flex items-center gap-1.5">
                <span
                  title={s}
                  className={`h-2 w-2 shrink-0 rounded-full ${
                    current ? "bg-accent ring-2 ring-accent/30" : reached ? "bg-accent" : "bg-ink-faint"
                  }`}
                />
                <span
                  className={`text-[11px] capitalize ${
                    current ? "font-medium text-ink" : reached ? "text-ink-dim" : "text-ink-faint"
                  }`}
                >
                  {s}
                </span>
                {i < FORWARD_STAGES.length - 1 && (
                  <span className={`mx-0.5 h-px w-4 ${reached ? "bg-accent/40" : "bg-line"}`} />
                )}
              </div>
            )
          })}
        </div>
        {bypass && (
          <div className="mt-2 text-xs capitalize text-status-danger">terminal — {pretty(v.stage)}</div>
        )}
      </div>

      <Field label="blocked on">
        <span
          className={`inline-flex items-center rounded border px-2 py-0.5 text-xs ${
            blk.needsAction ? statusToneClasses("warning") : statusToneClasses("success")
          }`}
        >
          {blk.text}
        </span>
      </Field>

      {(runStatus || runStatusErr) && (
        <Field label={t("deliveries.runStatus")}>
          <RunStatusInline
            runStatus={runStatus}
            runStatusErr={runStatusErr}
            linkedRunId={v.run_id ?? null}
            onOpenRun={onOpenRun}
          />
        </Field>
      )}

      {v.plan_path && (
        <Field label="plan">
          <div className="font-mono text-xs text-ink-dim">{v.plan_path}</div>
        </Field>
      )}

      {(v.accept_verdict || v.pm_accepted_by) && (
        <Field label="accept">
          <div className="text-sm text-ink-dim">
            {v.accept_verdict && <span className="capitalize">{pretty(v.accept_verdict)}</span>}
            {v.pm_accepted_by && <span className="text-ink-faint"> · by {v.pm_accepted_by}</span>}
          </div>
        </Field>
      )}

      {(v.closeout_summary || v.writeback_status) && (
        <Field label="closeout">
          <div className="space-y-1 text-sm text-ink-dim">
            {v.closeout_summary && <div>{v.closeout_summary}</div>}
            {v.writeback_status && (
              <WritebackLine status={v.writeback_status} receipt={v.writeback_receipt ?? null} />
            )}
          </div>
        </Field>
      )}

      {v.source_refs.length > 0 && (
        <Field label="source">
          <ul className="space-y-1 text-sm">
            {v.source_refs.map((r, i) => (
              <li key={i} className="text-ink-dim">
                <span className="text-ink-faint">{r.kind}</span>
                {r.name && <span> · {r.name}</span>}
                {r.uri && (
                  <a
                    href={r.uri}
                    target="_blank"
                    rel="noreferrer"
                    className="ml-1 inline-flex items-center gap-0.5 text-accent hover:underline"
                  >
                    {r.uri}
                    <ExternalLink size={10} />
                  </a>
                )}
              </li>
            ))}
          </ul>
        </Field>
      )}

      {/* F-132: audit timeline — collapsed by default (the stepper is the at-a-glance). */}
      <TimelineSection timeline={timeline} timelineErr={timelineErr} onOpenRun={onOpenRun} />
    </div>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <div className="mb-1 text-[11px] uppercase tracking-wide text-ink-faint">{label}</div>
      {children}
    </div>
  )
}

/// F-134: the write-back status + the external post receipt (posted → message/doc ref;
/// failed → error; intent_emitted → pending external post). Refs only — never the body.
function WritebackLine({
  status,
  receipt,
}: {
  status: WritebackStatus
  receipt: WritebackReceipt | null
}) {
  const isUri = (s?: string | null) => !!s && /^https?:\/\//i.test(s)
  return (
    <div className="space-y-0.5">
      <div className="text-xs text-ink-faint">
        write-back — <span className="text-ink-dim">{pretty(status)}</span>
        {status === "intent_emitted" && (
          <span className="text-ink-faint"> · {t("deliveries.writebackPending")}</span>
        )}
      </div>
      {status === "posted" && receipt && (
        <div className="text-[10px] text-status-success/80">
          {isUri(receipt.message_ref) ? (
            <a
              href={receipt.message_ref!}
              target="_blank"
              rel="noreferrer"
              className="text-accent hover:underline"
            >
              {t("deliveries.writebackMessage")}
            </a>
          ) : receipt.message_ref ? (
            <span className="font-mono">
              {t("deliveries.writebackMessage")}: {receipt.message_ref}
            </span>
          ) : null}
          {receipt.doc_revision && <span className="ml-2 font-mono">rev {receipt.doc_revision}</span>}
        </div>
      )}
      {status === "failed" && receipt?.error && (
        <div className="text-[10px] text-status-danger">
          {t("deliveries.writebackFailed")}: {receipt.error}
        </div>
      )}
    </div>
  )
}

/// F-131: inline live run status. A fetch failure (corrupt/missing linked run → 500)
/// shows an explicit "unavailable" notice — never silently rendered as idle. `linked`
/// distinguishes a written-back run from an in-flight one found via the back-ref.
function RunStatusInline({
  runStatus,
  runStatusErr,
  linkedRunId,
  onOpenRun,
}: {
  runStatus: RunStatusLite | null
  runStatusErr: string | null
  linkedRunId: string | null
  onOpenRun: (runId: string) => void
}) {
  if (runStatusErr) {
    return (
      <div className="space-y-1.5">
        <p className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-[11px] text-status-warning">
          {t("deliveries.runStatusUnavailable")}
        </p>
        {linkedRunId && (
          <button
            onClick={() => onOpenRun(linkedRunId)}
            className="inline-flex items-center gap-1 font-mono text-xs text-accent hover:underline"
          >
            {linkedRunId}
            <ExternalLink size={11} />
            <span className="text-ink-faint">({t("deliveries.openRun")})</span>
          </button>
        )}
      </div>
    )
  }
  if (!runStatus) return null
  const p = runStatus.progress
  return (
    <div className="flex flex-wrap items-center gap-2">
      <StatusChip status={runStatus.status} />
      <span className="text-xs text-ink-dim">
        {p.done}/{p.total}
        {p.failed > 0 ? ` · ${p.failed} failed` : ""}
        {p.running > 0 ? ` · ${p.running} running` : ""}
      </span>
      <span className="inline-flex items-center rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-faint">
        {runStatus.linked ? t("deliveries.linkedRun") : t("deliveries.inFlight")}
      </span>
      <button
        onClick={() => onOpenRun(runStatus.run_id)}
        className="ml-auto inline-flex items-center gap-1 font-mono text-xs text-accent hover:underline"
      >
        {runStatus.run_id}
        <ExternalLink size={11} />
        <span className="text-ink-faint">({t("deliveries.openRun")})</span>
      </button>
    </div>
  )
}

// F-132: an absolute timestamp, lightly trimmed to "YYYY-MM-DD HH:MM:SS" (no relative
// time — the pin keeps RFC3339 precision).
function fmtTs(at: string): string {
  const m = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})/.exec(at)
  return m ? `${m[1]} ${m[2]}` : at
}

/// F-132: the audit timeline — collapsed by default (the stepper is the at-a-glance).
/// A fetch failure (corrupt / audit-node inconsistency → 500) shows an explicit
/// "unavailable" notice, NEVER a silent empty list.
function TimelineSection({
  timeline,
  timelineErr,
  onOpenRun,
}: {
  timeline: TimelineEvent[] | null
  timelineErr: string | null
  onOpenRun: (runId: string) => void
}) {
  return (
    <CollapsibleSection
      title={t("deliveries.timeline")}
      icon={<History size={13} className="shrink-0 text-ink-faint" />}
    >
      <div className="px-4 py-3">
        {timelineErr ? (
          <p className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-[11px] text-status-warning">
            {t("deliveries.timelineUnavailable")}
          </p>
        ) : !timeline ? (
          <p className="text-[11px] text-ink-faint">{t("common.loading")}</p>
        ) : timeline.length === 0 ? (
          <p className="text-[11px] text-ink-faint">{t("deliveries.timelineEmpty")}</p>
        ) : (
          <ol className="space-y-2.5">
            {timeline.map((e, i) => (
              <TimelineRow key={i} e={e} onOpenRun={onOpenRun} />
            ))}
          </ol>
        )}
      </div>
    </CollapsibleSection>
  )
}

function TimelineRow({ e, onOpenRun }: { e: TimelineEvent; onOpenRun: (runId: string) => void }) {
  return (
    <li className="flex gap-3">
      <div className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-accent/60" />
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] capitalize text-ink-dim">
            {pretty(e.stage)}
          </span>
          <span className="font-mono text-[10px] text-ink-faint">{fmtTs(e.at)}</span>
          {e.by && <span className="text-[10px] text-ink-faint">· {e.by}</span>}
        </div>
        {e.reason && <div className="text-xs text-ink-dim">{e.reason}</div>}
        {e.refs && e.refs.length > 0 && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pt-0.5">
            {e.refs.map((r, i) => (
              <TimelineRefView key={i} r={r} onOpenRun={onOpenRun} />
            ))}
          </div>
        )}
      </div>
    </li>
  )
}

function TimelineRefView({
  r,
  onOpenRun,
}: {
  r: TimelineRef
  onOpenRun: (runId: string) => void
}) {
  if (r.kind === "run" && r.value) {
    return (
      <button
        onClick={() => onOpenRun(r.value!)}
        className="inline-flex items-center gap-1 font-mono text-[10px] text-accent hover:underline"
      >
        run {r.value}
        <ExternalLink size={9} />
      </button>
    )
  }
  if (r.uri) {
    return (
      <a
        href={r.uri}
        target="_blank"
        rel="noreferrer"
        className="inline-flex items-center gap-0.5 text-[10px] text-accent hover:underline"
      >
        {r.kind}
        <ExternalLink size={9} />
      </a>
    )
  }
  return (
    <span className="text-[10px] text-ink-faint">
      <span className="text-ink-dim">{r.kind}</span>
      {r.value && <span className="ml-1 font-mono">{r.value}</span>}
      {r.summary && <span className="ml-1">· {r.summary}</span>}
    </span>
  )
}

/// A forward-action button with an inline confirm row (no native window.confirm).
/// `danger` styles the run action; `busy` shows the in-flight state.
function ActionButton({
  label,
  ask,
  danger,
  busy,
  disabled,
  onConfirm,
}: {
  label: string
  ask: string
  danger?: boolean
  busy: boolean
  disabled: boolean
  onConfirm: () => void
}) {
  const [confirming, setConfirming] = useState(false)
  const tone = danger
    ? `${statusToneClasses("danger")} hover:bg-red-500/25`
    : "border-accent/30 bg-accent/15 text-accent hover:bg-accent/25"

  if (confirming) {
    return (
      <div className="flex flex-wrap items-center gap-2 rounded-md border border-line bg-bg-inset px-3 py-2">
        <span className="text-xs text-ink-dim">{ask}</span>
        <div className="ml-auto flex items-center gap-2">
          <button
            onClick={() => {
              setConfirming(false)
              onConfirm()
            }}
            disabled={disabled}
            className={`rounded border px-2.5 py-1 text-xs font-medium disabled:opacity-60 ${tone}`}
          >
            {t("deliveries.confirmYes")}
          </button>
          <button
            onClick={() => setConfirming(false)}
            disabled={disabled}
            className="rounded border border-line px-2.5 py-1 text-xs text-ink-dim hover:text-ink disabled:opacity-60"
          >
            {t("deliveries.cancel")}
          </button>
        </div>
      </div>
    )
  }
  return (
    <button
      onClick={() => setConfirming(true)}
      disabled={disabled}
      className={`rounded-md border px-3 py-1.5 text-sm font-medium disabled:opacity-60 ${tone}`}
    >
      {busy ? t("deliveries.working") : label}
    </button>
  )
}

// ── F-130 accept / closeout forms ────────────────────────────────────────────

const INPUT_CLS =
  "w-full rounded border border-line bg-bg-inset px-2.5 py-1.5 text-sm focus:border-accent focus:outline-none"
const TEXTAREA_CLS =
  "w-full resize-none rounded-md border border-line bg-bg-inset p-2 font-mono text-xs focus:border-accent focus:outline-none"

const VERDICTS: AcceptVerdict[] = ["accepted", "partial", "changes_requested", "rejected"]

function FormField({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: ReactNode
}) {
  return (
    <div>
      <label className="mb-1 block text-[11px] uppercase tracking-wide text-ink-faint">
        {label}
      </label>
      {children}
      {hint && <p className="mt-1 text-[10px] text-ink-faint">{hint}</p>}
    </div>
  )
}

/// Accept form (stage=execute). The run-outcome rules (failed → only
/// changes_requested/rejected; unverified accept/partial needs debt) are enforced by
/// the server; a refusal surfaces as `error`. The failed-with-debt checkbox is always
/// shown but its copy is explicit that the server is the final judge.
function AcceptForm({
  busy,
  disabled,
  error,
  onSubmit,
}: {
  busy: boolean
  disabled: boolean
  error: string | null
  onSubmit: (input: AcceptInput) => void
}) {
  const [verdict, setVerdict] = useState<AcceptVerdict>("accepted")
  const [notes, setNotes] = useState("")
  const [debtText, setDebtText] = useState("")
  const [failedWithDebt, setFailedWithDebt] = useState(false)

  const submit = () =>
    onSubmit({
      verdict,
      notes: notes.trim() || undefined,
      debt: splitLines(debtText),
      accept_failed_with_debt: failedWithDebt,
    })

  return (
    <Field label={t("deliveries.acceptTitle")}>
      <div className="space-y-3 rounded-lg border border-line bg-bg-panel p-4">
        <FormField label={t("deliveries.acceptVerdict")}>
          <select
            value={verdict}
            onChange={(e) => setVerdict(e.target.value as AcceptVerdict)}
            className={INPUT_CLS}
          >
            {VERDICTS.map((vd) => (
              <option key={vd} value={vd}>
                {t(`deliveries.verdict.${vd}`)}
              </option>
            ))}
          </select>
        </FormField>
        <FormField label={t("deliveries.debt")} hint={t("deliveries.debtHint")}>
          <textarea
            rows={2}
            value={debtText}
            onChange={(e) => setDebtText(e.target.value)}
            className={TEXTAREA_CLS}
          />
        </FormField>
        <FormField label={t("deliveries.notes")}>
          <textarea
            rows={2}
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            className={TEXTAREA_CLS}
          />
        </FormField>
        <label className="flex cursor-pointer items-start gap-2 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={failedWithDebt}
            onChange={(e) => setFailedWithDebt(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            {t("deliveries.acceptFailedWithDebt")}
            <span className="block text-[10px] text-ink-faint">
              {t("deliveries.acceptFailedWithDebtHint")}
            </span>
          </span>
        </label>
        <ActionButton
          label={t("deliveries.recordVerdict")}
          ask={t("deliveries.acceptAsk")}
          busy={busy}
          disabled={disabled}
          onConfirm={submit}
        />
        {error && <div className="text-xs text-status-danger">{error}</div>}
      </div>
    </Field>
  )
}

/// Closeout form (stage=accept, verdict accepted/partial). Submit is disabled until
/// at least one evidence-bearing field is present — mirroring the server's
/// empty-closeout guard (the server stays authoritative). Write-back, when on, makes
/// the confirm copy explicit that maestro emits an intent (no in-process post).
function CloseoutForm({
  busy,
  disabled,
  error,
  onSubmit,
}: {
  busy: boolean
  disabled: boolean
  error: string | null
  onSubmit: (input: CloseoutInput) => void
}) {
  const [commits, setCommits] = useState("")
  const [ci, setCi] = useState("")
  const [reviews, setReviews] = useState("")
  const [docRevisions, setDocRevisions] = useState("")
  const [evidence, setEvidence] = useState("")
  const [writeback, setWriteback] = useState(false)

  const input: CloseoutInput = {
    commits: splitLines(commits),
    ci: splitLines(ci),
    reviews: splitLines(reviews),
    doc_revisions: splitLines(docRevisions),
    evidence: splitLines(evidence),
    writeback,
  }
  const hasEvidence =
    input.commits.length +
      input.ci.length +
      input.reviews.length +
      input.doc_revisions.length +
      input.evidence.length >
    0

  return (
    <Field label={t("deliveries.closeoutTitle")}>
      <div className="space-y-3 rounded-lg border border-line bg-bg-panel p-4">
        <FormField label={t("deliveries.evidence")} hint={t("deliveries.evidenceHint")}>
          <textarea
            rows={2}
            value={evidence}
            onChange={(e) => setEvidence(e.target.value)}
            className={TEXTAREA_CLS}
          />
        </FormField>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <FormField label={t("deliveries.commits")}>
            <textarea
              rows={2}
              value={commits}
              onChange={(e) => setCommits(e.target.value)}
              className={TEXTAREA_CLS}
            />
          </FormField>
          <FormField label={t("deliveries.ci")}>
            <textarea
              rows={2}
              value={ci}
              onChange={(e) => setCi(e.target.value)}
              className={TEXTAREA_CLS}
            />
          </FormField>
          <FormField label={t("deliveries.reviews")}>
            <textarea
              rows={2}
              value={reviews}
              onChange={(e) => setReviews(e.target.value)}
              className={TEXTAREA_CLS}
            />
          </FormField>
          <FormField label={t("deliveries.docRevisions")}>
            <textarea
              rows={2}
              value={docRevisions}
              onChange={(e) => setDocRevisions(e.target.value)}
              className={TEXTAREA_CLS}
            />
          </FormField>
        </div>
        <label className="flex cursor-pointer items-center gap-2 text-xs text-ink-dim">
          <input
            type="checkbox"
            checked={writeback}
            onChange={(e) => setWriteback(e.target.checked)}
          />
          {t("deliveries.writeback")}
        </label>
        {!hasEvidence && (
          <p className="text-[10px] text-status-warning/80">{t("deliveries.needEvidence")}</p>
        )}
        <ActionButton
          label={t("deliveries.closeoutSubmit")}
          ask={writeback ? t("deliveries.closeoutAskWriteback") : t("deliveries.closeoutAsk")}
          busy={busy}
          disabled={disabled || !hasEvidence}
          onConfirm={() => onSubmit(input)}
        />
        {error && <div className="text-xs text-status-danger">{error}</div>}
      </div>
    </Field>
  )
}
