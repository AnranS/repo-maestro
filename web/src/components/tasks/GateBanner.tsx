import { useEffect, useState } from "react"
import { Check, ClipboardCheck, Play, X } from "lucide-react"
import type { RunMonitor } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"
import { statusToken } from "../ui/StatusChip"

const GATE_MARKER: Record<string, string> = {
  plan: "__gate_plan__",
  outcome: "__gate_outcome__",
}

/**
 * Boundary review gate banner. When a run pauses at the plan gate (before any
 * task runs) or the outcome gate (after the work + acceptance checks), this is
 * the single decision point — approve to proceed or reject to cancel.
 *
 * F-123: this reads the run monitor's read-only gate projection
 * (`RunMonitor.gates`) instead of scraping raw `RunState`. A projection read
 * error shows an explicit "gate status unavailable" — it is NEVER silently
 * collapsed to "no gate", and the banner never recomputes gate state itself.
 */
export function GateBanner({ runId }: { runId: string }) {
  const [monitor, setMonitor] = useState<RunMonitor | "error" | undefined>(undefined)
  const [busy, setBusy] = useState<"approve" | "reject" | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = () =>
      api
        .runMonitor(runId)
        .then((m) => !cancelled && setMonitor(m))
        .catch(() => !cancelled && setMonitor("error"))
    load()
    // Poll so the banner appears/clears as the run reaches/passes a gate.
    const timer = setInterval(load, 2500)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [runId])

  // Loading: the banner is transient, so render nothing rather than a skeleton.
  if (monitor === undefined) return null
  // Projection error: explicit and visible — never silently "no gate".
  if (monitor === "error") {
    return (
      <section className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-4 py-2.5 text-[12px] text-status-warning/90">
        {t("gate.unavailable")}
      </section>
    )
  }

  // Only the run-level boundary gate (plan/outcome) renders here; task-approval
  // gates surface on the task rows. Absent = no run gate pending → render nothing.
  const gate = monitor.gates.find((g) => g.scope === "run")
  if (!gate) return null
  const isPlan = gate.kind === "plan"
  const ev = gate.evidence

  const decide = async (decision: "approve" | "reject") => {
    if (decision === "reject" && !confirm(t("gate.rejectConfirm"))) return
    setBusy(decision)
    try {
      await api.runApprove(runId, GATE_MARKER[gate.kind] ?? "", decision)
    } finally {
      setBusy(null)
    }
  }

  return (
    <section className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-4">
      <div className="flex items-start gap-3">
        <span className="mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-amber-500/40 bg-amber-500/15 text-status-warning">
          {isPlan ? <ClipboardCheck size={16} /> : <Check size={16} />}
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-status-warning">
            {isPlan ? t("gate.planTitle") : t("gate.outcomeTitle")}
          </h2>
          <p className="mt-0.5 text-[12px] leading-snug text-status-warning/85">
            {isPlan ? t("gate.planBody") : t("gate.outcomeBody")}
          </p>

          <div className="mt-2 flex flex-wrap gap-2 text-[11px] text-ink-mute">
            {isPlan ? (
              <>
                <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                  {t("gate.taskCount", { n: ev.task_count ?? 0 })}
                </span>
                <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                  {t("gate.projectCount", { n: ev.project_count ?? 0 })}
                </span>
              </>
            ) : (ev.acceptance_total ?? 0) > 0 ? (
              <>
                <span
                  className={`rounded px-1.5 py-0.5 ${
                    statusToken(ev.verified ? "done" : "failed").chip
                  }`}
                >
                  {ev.verified ? t("gate.verified") : t("gate.notVerified")}
                </span>
                <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                  {t("gate.checks", {
                    passed: ev.acceptance_passed ?? 0,
                    total: ev.acceptance_total ?? 0,
                  })}
                </span>
              </>
            ) : (
              <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                {t("gate.noChecks")}
              </span>
            )}
          </div>

          <div className="mt-3 flex items-center gap-2">
            <button
              onClick={() => decide("approve")}
              disabled={busy !== null}
              className="inline-flex items-center gap-1.5 rounded-md border border-emerald-500/40 bg-emerald-500/15 px-3 py-1.5 text-[12px] font-medium text-status-success hover:bg-emerald-500/25 disabled:opacity-60"
            >
              {isPlan ? <Play size={13} /> : <Check size={13} />}
              {busy === "approve"
                ? t("gate.working")
                : isPlan
                  ? t("gate.approvePlan")
                  : t("gate.approveOutcome")}
            </button>
            <button
              onClick={() => decide("reject")}
              disabled={busy !== null}
              className="inline-flex items-center gap-1.5 rounded-md border border-red-500/30 px-3 py-1.5 text-[12px] font-medium text-status-danger hover:bg-red-500/15 disabled:opacity-60"
            >
              <X size={13} />
              {t("gate.reject")}
            </button>
          </div>
        </div>
      </div>
    </section>
  )
}
