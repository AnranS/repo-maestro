// F-UI-001 + F-118: the operator-console home (Console surface). It answers the
// three questions — can it run / what's running / what next. Readiness is now the
// real `GET /api/runtime/health` projection (F-118); loading shows a neutral
// "checking", a fetch error shows ONE neutral "unavailable" row, and we never
// fake-infer health from run history.

import { useEffect, useState } from "react"
import { ArrowRight, RotateCw } from "lucide-react"
import { api } from "../api"
import type { RunState, RunSummary, RuntimeHealthReport } from "../types"
import { StatusChip } from "./ui/StatusChip"

// the six v1 readiness ids, in stable order — used for neutral loading rows.
const READINESS_LABELS = [
  "Workspace detected",
  "Provider available",
  "Profiles valid",
  "Skills visible",
  "Plan preview valid",
  "Resume guard ready",
]

// health verdict → the shared F-UI-001 status token.
const STATUS_TO_TOKEN: Record<string, string> = {
  pass: "done",
  warn: "blocked",
  fail: "failed",
  skip: "pending",
}
// issue-first display order; the backend already returns checks in stable v1 order,
// and Array.sort is stable, so within-group order is preserved.
const ISSUE_ORDER = ["fail", "warn", "pass", "skip"]

function Readiness() {
  const [report, setReport] = useState<RuntimeHealthReport | null>(null)
  const [error, setError] = useState(false)
  const [loading, setLoading] = useState(true)
  const [reloadKey, setReloadKey] = useState(0)

  useEffect(() => {
    let alive = true
    setLoading(true)
    setError(false)
    // drop the previous report so a stale overall chip can never show through a
    // "checking" / "unavailable" state (no fake health during reload/error).
    setReport(null)
    api
      .runtimeHealth()
      .then((r) => {
        if (!alive) return
        setReport(r)
        setLoading(false)
      })
      .catch(() => {
        if (!alive) return
        setError(true)
        setLoading(false)
      })
    return () => {
      alive = false
    }
  }, [reloadKey])

  const checks = report
    ? [...report.checks].sort(
        (a, b) => ISSUE_ORDER.indexOf(a.status) - ISSUE_ORDER.indexOf(b.status),
      )
    : []

  return (
    <section className="rounded-lg border border-line bg-bg-panel">
      <header className="flex items-center gap-2 border-b border-line px-4 py-2 text-xs font-medium text-ink-mute">
        Readiness
        {/* overall chip ONLY in the final loaded state — never during
            checking/unavailable, so the header can't show stale health. */}
        {!loading && !error && report && (
          <span className="ml-auto">
            <StatusChip status={STATUS_TO_TOKEN[report.summary.overall]} />
          </span>
        )}
        {error && (
          <button
            type="button"
            onClick={() => setReloadKey((k) => k + 1)}
            className="ml-auto flex items-center gap-1 text-[11px] text-ink-faint hover:text-ink-dim"
          >
            <RotateCw size={12} /> retry
          </button>
        )}
      </header>

      {loading ? (
        // neutral "checking" — never a fake pass while the probe runs.
        <div className="divide-y divide-line/40">
          {READINESS_LABELS.map((label) => (
            <div key={label} className="flex items-center gap-2.5 px-4 py-2">
              <span className="rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-faint">
                checking
              </span>
              <span className="text-[12px] text-ink-faint">{label}</span>
            </div>
          ))}
        </div>
      ) : error || !report ? (
        // one neutral unavailable row — NOT six fake passes.
        <div className="flex items-center gap-2.5 px-4 py-3 text-[12px] text-ink-faint">
          <StatusChip status="pending" label="unavailable" />
          readiness check unavailable — the local server could not produce a report
        </div>
      ) : (
        <div className="divide-y divide-line/40">
          {checks.map((c) => (
            <div key={c.id} className="flex items-start gap-2.5 px-4 py-2">
              <StatusChip status={STATUS_TO_TOKEN[c.status]} />
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2">
                  <span className="text-[12px] text-ink-dim">{c.label}</span>
                  <span className="truncate text-[11px] text-ink-faint">{c.message}</span>
                </div>
                {c.fix && (c.status === "warn" || c.status === "fail") && (
                  <div className="mt-0.5 text-[11px] text-ink-faint">fix: {c.fix}</div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}

export function Dashboard({
  state,
  runs,
  onOpenRun,
}: {
  state: RunState | null
  runs: RunSummary[]
  /** runId selects a specific historical run; omit to open the current/live run. */
  onOpenRun: (runId?: string) => void
}) {
  const tasks = state ? Object.values(state.tasks) : []
  const done = tasks.filter((t) => t.status === "done").length
  // Active list is capped (first-viewport constraint); "view all" opens the
  // Run Inspector.
  const recent = runs.filter((r) => r.run_id !== state?.run_id).slice(0, 3)
  const lastFailed = runs.find((r) => r.status === "failed")

  return (
    <div className="mx-auto w-full max-w-4xl space-y-4 p-4">
      {/* READINESS — real F-118 health */}
      <Readiness />

      {/* ACTIVE */}
      <section className="rounded-lg border border-line bg-bg-panel">
        <header className="flex items-center border-b border-line px-4 py-2 text-xs font-medium text-ink-mute">
          Active
          {runs.length > 0 && (
            <button
              onClick={() => onOpenRun()}
              className="ml-auto text-[11px] text-ink-faint hover:text-ink-dim"
            >
              view all {runs.length} →
            </button>
          )}
        </header>
        <div className="divide-y divide-line/40">
          {state ? (
            <button
              onClick={() => onOpenRun()}
              className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-bg-hover/40"
            >
              <StatusChip status={state.status} />
              <span className="font-mono text-[12px] text-ink-dim">
                {state.run_id.slice(0, 16)}
              </span>
              <span className="flex-1 truncate text-[12px] text-ink-faint">
                {state.spec}
              </span>
              <span className="font-mono tabular-nums text-[11px] text-ink-faint">
                {done}/{tasks.length}
              </span>
              <ArrowRight size={13} className="shrink-0 text-ink-faint" />
            </button>
          ) : recent.length === 0 ? (
            <div className="px-4 py-6 text-center text-[12px] text-ink-faint">
              no runs yet — start a goal to see it here
            </div>
          ) : null}
          {recent.map((r) => (
            <button
              key={r.run_id}
              onClick={() => onOpenRun(r.run_id)}
              className="flex w-full items-center gap-3 px-4 py-2 text-left hover:bg-bg-hover/40"
            >
              <StatusChip status={r.status} />
              <span className="font-mono text-[12px] text-ink-dim">
                {r.run_id.slice(0, 16)}
              </span>
              <span className="flex-1 truncate text-[12px] text-ink-faint">
                {r.spec}
              </span>
            </button>
          ))}
        </div>
      </section>

      {/* NEXT */}
      <section className="rounded-lg border border-line bg-bg-panel">
        <header className="border-b border-line px-4 py-2 text-xs font-medium text-ink-mute">
          Next
        </header>
        <div className="flex flex-wrap gap-2 p-4">
          <button
            onClick={() => onOpenRun()}
            className="rounded-lg border border-blue-500/30 bg-blue-500/10 px-3 py-1.5 text-[12px] text-accent hover:bg-blue-500/15"
          >
            Open Run Inspector
          </button>
          {/* Not yet wired — non-interactive (no dead-clicks): disabled chips,
              not buttons, with no hover affordance. */}
          {(
            [
              "Run a goal",
              "Resume",
              "Doctor",
              "Configure",
              lastFailed ? "View last failure" : null,
            ].filter(Boolean) as string[]
          ).map((label) => (
            <span
              key={label}
              aria-disabled="true"
              title="not wired yet"
              className="cursor-default select-none rounded-lg border border-line bg-bg-inset px-3 py-1.5 text-[12px] text-ink-faint opacity-60"
            >
              {label}
            </span>
          ))}
        </div>
      </section>
    </div>
  )
}
