import { useState } from "react"
import { Check, X, Target, ChevronDown, ChevronRight } from "lucide-react"
import type { Goal, AcceptanceResult, RunState } from "../types"
import { t } from "../i18n"

interface Props {
  goal?: Goal | null
  results?: AcceptanceResult[]
  verified?: boolean
  status: RunState["status"]
}

const formatExit = (code: number | null | undefined): string => {
  if (code === null || code === undefined) return "spawn error"
  return `exit ${code}`
}

export function GoalPanel({ goal, results, verified, status }: Props) {
  if (!goal || (!goal.description && !goal.acceptance?.length)) return null

  const declared = goal.acceptance ?? []
  const ran = results ?? []
  const hasResults = ran.length > 0
  const dagFinished = status !== "running"

  const passed = ran.filter((r) => r.passed).length
  const total = hasResults ? ran.length : declared.length

  const headerVariant: "ok" | "fail" | "pending" =
    !hasResults
      ? "pending"
      : verified
        ? "ok"
        : "fail"

  const headerStyle: Record<typeof headerVariant, string> = {
    ok: "bg-emerald-500/10 border-emerald-500/30 text-emerald-200",
    fail: "bg-red-500/10 border-red-500/30 text-red-200",
    pending: "bg-bg-panel border-line text-ink-dim",
  }

  const headerLabel = (() => {
    if (!hasResults) return dagFinished ? t("tasks.acceptancePending") : t("tasks.acceptancePending")
    if (verified) return `${passed}/${total} · ${t("tasks.verified")}`
    return `${passed}/${total} · ${t("tasks.notVerified")}`
  })()

  return (
    <section className="bg-bg-panel border border-line rounded-xl">
      <div className="px-4 py-2.5 border-b border-line flex items-center gap-3">
        <Target size={13} className="text-ink-faint" />
        <span className="text-xs uppercase tracking-wider text-ink-faint">
          {t("tasks.goal")}
        </span>
        <span className={`ml-auto inline-flex items-center gap-1.5 px-2 py-0.5 rounded border text-[11px] ${headerStyle[headerVariant]}`}>
          {headerLabel}
        </span>
      </div>

      {goal.description && (
        <div className="px-4 py-3 border-b border-line/40 text-sm text-ink-dim">
          {goal.description}
        </div>
      )}

      {(declared.length > 0 || hasResults) && (
        <div className="divide-y divide-line/40">
          {/* If we have results, show those. Otherwise show declared checks
              dimmed so the user sees what's still pending. */}
          {hasResults
            ? ran.map((r, i) => <Row key={i} result={r} />)
            : declared.map((a, i) => (
                <div key={i} className="flex items-start gap-3 px-4 py-2.5">
                  <span className="mt-0.5 w-3.5 h-3.5 rounded-full border border-line/60 shrink-0" />
                  <div className="flex-1 min-w-0">
                    <div className="text-sm text-ink-dim truncate">{a.describe}</div>
                    <code className="block mt-0.5 text-[11px] text-ink-faint font-mono truncate">
                      {a.check}
                    </code>
                  </div>
                </div>
              ))}
        </div>
      )}
    </section>
  )
}

function Row({ result }: { result: AcceptanceResult }) {
  const [open, setOpen] = useState(false)
  const Icon = result.passed ? Check : X
  const iconClass = result.passed
    ? "text-emerald-300 bg-emerald-500/15 border-emerald-500/30"
    : "text-red-300 bg-red-500/15 border-red-500/30"

  const hasOutput = !!result.output

  return (
    <div className="px-4 py-2.5">
      <div className="flex items-start gap-3">
        <span className={`mt-0.5 w-4 h-4 rounded-full border flex items-center justify-center shrink-0 ${iconClass}`}>
          <Icon size={10} strokeWidth={3} />
        </span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm text-ink truncate">{result.describe}</span>
            <span className="text-[10px] text-ink-faint font-mono shrink-0">
              {formatExit(result.exit_code)}
            </span>
          </div>
          <code className="block mt-0.5 text-[11px] text-ink-faint font-mono truncate">
            {result.check}
          </code>
          {hasOutput && (
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              className="mt-1 inline-flex items-center gap-1 text-[11px] text-ink-faint hover:text-ink-dim"
              aria-label={open ? t("tasks.hideOutput") : t("tasks.showOutput")}
            >
              {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
              {open ? t("tasks.hideOutput") : t("tasks.showOutput")}
            </button>
          )}
          {open && hasOutput && (
            <pre className="mt-1.5 px-2 py-1.5 text-[11px] leading-snug bg-bg-deep border border-line/60 rounded-md overflow-x-auto scrollbar-thin text-ink-dim whitespace-pre-wrap break-all max-h-64 overflow-y-auto">
              {result.output}
            </pre>
          )}
        </div>
      </div>
    </div>
  )
}
