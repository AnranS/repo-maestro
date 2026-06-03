import { useState } from "react"
import { Check, ClipboardCheck, Play, X } from "lucide-react"
import type { RunState } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"

const GATE_MARKER: Record<string, string> = {
  plan: "__gate_plan__",
  outcome: "__gate_outcome__",
}

/**
 * Boundary review gate banner. When a run pauses at the plan gate (before any
 * task runs) or the outcome gate (after the work + acceptance checks), this is
 * the single decision point — approve to proceed or reject to cancel — so human
 * judgment lands at the two boundaries instead of on every task. Renders
 * nothing when the run isn't waiting on a gate.
 */
export function GateBanner({ state }: { state: RunState }) {
  const [busy, setBusy] = useState<"approve" | "reject" | null>(null)
  const gate = state.pending_gate
  if (gate !== "plan" && gate !== "outcome") return null

  const decide = async (decision: "approve" | "reject") => {
    if (decision === "reject" && !confirm(t("gate.rejectConfirm"))) return
    setBusy(decision)
    try {
      await api.runApprove(state.run_id, GATE_MARKER[gate] ?? "", decision)
    } finally {
      setBusy(null)
    }
  }

  const tasks = Object.values(state.tasks)
  const projects = [...new Set(tasks.map((tk) => tk.project).filter(Boolean))]
  const accTotal = state.acceptance_results?.length ?? 0
  const accPassed = state.acceptance_results?.filter((r) => r.passed).length ?? 0

  return (
    <section className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-4">
      <div className="flex items-start gap-3">
        <span className="mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-amber-500/40 bg-amber-500/15 text-amber-200">
          {gate === "plan" ? <ClipboardCheck size={16} /> : <Check size={16} />}
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-amber-100">
            {gate === "plan" ? t("gate.planTitle") : t("gate.outcomeTitle")}
          </h2>
          <p className="mt-0.5 text-[12px] leading-snug text-amber-100/80">
            {gate === "plan" ? t("gate.planBody") : t("gate.outcomeBody")}
          </p>

          <div className="mt-2 flex flex-wrap gap-2 text-[11px] text-ink-mute">
            {gate === "plan" ? (
              <>
                <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                  {t("gate.taskCount", { n: tasks.length })}
                </span>
                {projects.slice(0, 6).map((p) => (
                  <span key={p} className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                    {p}
                  </span>
                ))}
              </>
            ) : (
              <>
                {accTotal > 0 ? (
                  <>
                    <span
                      className={`rounded border px-1.5 py-0.5 ${
                        state.verified
                          ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-300"
                          : "border-red-500/30 bg-red-500/10 text-red-300"
                      }`}
                    >
                      {state.verified ? t("gate.verified") : t("gate.notVerified")}
                    </span>
                    <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                      {t("gate.checks", { passed: accPassed, total: accTotal })}
                    </span>
                  </>
                ) : (
                  <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                    {t("gate.noChecks")}
                  </span>
                )}
                <span className="rounded border border-line/60 bg-bg/40 px-1.5 py-0.5">
                  {t("gate.tasksDone", { n: tasks.filter((x) => x.status === "done").length })}
                </span>
              </>
            )}
          </div>

          <div className="mt-3 flex items-center gap-2">
            <button
              onClick={() => decide("approve")}
              disabled={busy !== null}
              className="inline-flex items-center gap-1.5 rounded-md border border-emerald-500/40 bg-emerald-500/15 px-3 py-1.5 text-[12px] font-medium text-emerald-200 hover:bg-emerald-500/25 disabled:opacity-60"
            >
              {gate === "plan" ? <Play size={13} /> : <Check size={13} />}
              {busy === "approve"
                ? t("gate.working")
                : gate === "plan"
                  ? t("gate.approvePlan")
                  : t("gate.approveOutcome")}
            </button>
            <button
              onClick={() => decide("reject")}
              disabled={busy !== null}
              className="inline-flex items-center gap-1.5 rounded-md border border-red-500/30 px-3 py-1.5 text-[12px] font-medium text-red-200 hover:bg-red-500/15 disabled:opacity-60"
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
