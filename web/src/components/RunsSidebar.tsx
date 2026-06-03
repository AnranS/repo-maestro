import { Layers, Clock, TrendingUp } from "lucide-react"
import type { RunSummary } from "../types"
import { t } from "../i18n"

function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return `${n}`
}

function median(xs: number[]): number {
  if (xs.length === 0) return 0
  const s = [...xs].sort((a, b) => a - b)
  const m = Math.floor(s.length / 2)
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2
}

/** Cross-run token spend over time: one bar per run (oldest left → newest
 *  right), height ∝ tokens, with anomalies (> 2× the median) flagged red so a
 *  runaway run stands out against the baseline. Hidden until there are at least
 *  two runs with recorded usage. */
function CostTrend({ runs }: { runs: RunSummary[] }) {
  // runs arrive newest-first; chart reads left→right in time order.
  const withUsage = [...runs].reverse().filter((r) => (r.total_tokens ?? 0) > 0)
  if (withUsage.length < 2) return null
  const shown = withUsage.slice(-24)
  const tokens = shown.map((r) => r.total_tokens ?? 0)
  const max = Math.max(...tokens)
  const med = median(tokens)
  const anomalyAt = med > 0 ? med * 2 : Infinity
  const totalCost = shown.reduce((s, r) => s + (r.cost_usd ?? 0), 0)
  return (
    <div className="border-b border-line px-3 py-2.5">
      <div className="mb-1.5 flex items-center gap-1.5 text-[10px] uppercase tracking-wider text-ink-faint">
        <TrendingUp size={11} />
        {t("cost.trend")}
        <span className="ml-auto font-mono normal-case tracking-normal text-ink-mute">
          {t("cost.median", { n: fmtTok(med) })}
          {totalCost > 0 && ` · $${totalCost.toFixed(2)}`}
        </span>
      </div>
      <div className="flex h-12 items-end gap-0.5">
        {shown.map((r) => {
          const tk = r.total_tokens ?? 0
          const anomaly = tk >= anomalyAt
          const h = max > 0 ? Math.max(6, Math.round((tk / max) * 100)) : 6
          return (
            <div
              key={r.run_id}
              title={`${r.run_id}\n${fmtTok(tk)} tokens${r.cost_usd != null ? ` · $${r.cost_usd.toFixed(2)}` : ""}${anomaly ? ` · ${t("cost.anomaly")}` : ""}`}
              className={`min-w-[3px] flex-1 rounded-sm ${anomaly ? "bg-red-400/80" : "bg-blue-400/60"}`}
              style={{ height: `${h}%` }}
            />
          )
        })}
      </div>
    </div>
  )
}

export function RunsSidebar({
  runs,
  selectedRunId,
  liveRunId,
  onSelect,
}: {
  runs: RunSummary[]
  /** The run currently being viewed (highlighted). */
  selectedRunId: string | null
  /** The live/current run id (gets a "live" dot). */
  liveRunId: string | null
  onSelect: (id: string) => void
}) {
  return (
    <aside className="flex max-h-44 w-full shrink-0 flex-col border-b border-line bg-bg-soft md:max-h-none md:w-64 md:border-b-0 md:border-r">
      <div className="px-4 py-3 border-b border-line flex items-center gap-2">
        <Layers size={14} className="text-ink-dim" />
        <span className="text-sm font-semibold">runs</span>
        <span className="ml-auto text-[11px] text-ink-faint">{runs.length}</span>
      </div>

      <CostTrend runs={runs} />

      <div className="flex-1 overflow-y-auto scrollbar-thin">
        {runs.length === 0 ? (
          <div className="p-4 text-xs text-ink-faint">
            No runs yet. Run <code className="bg-bg-inset px-1 rounded">maestro run plan.yaml</code> in a project workspace.
          </div>
        ) : (
          runs.map((r) => {
            const isSelected = r.run_id === selectedRunId
            const isLive = r.run_id === liveRunId
            return (
              <button
                key={r.run_id}
                onClick={() => onSelect(r.run_id)}
                className={`block w-full cursor-pointer border-b border-line/40 px-3 py-2 text-left transition-colors hover:bg-bg-hover/60 ${
                  isSelected ? "bg-bg-hover ring-1 ring-inset ring-blue-500/40" : ""
                }`}
              >
                <div className="flex items-center gap-2 text-xs">
                  <span
                    className={`w-1.5 h-1.5 rounded-full ${statusDot(r.status)}`}
                  />
                  <span className="font-mono text-[10px] text-ink-faint truncate">
                    {r.run_id.slice(0, 16)}
                  </span>
                  {isLive && (
                    <span className="rounded bg-blue-500/15 px-1 text-[9px] text-blue-300">live</span>
                  )}
                  <span className={`ml-auto text-[10px] ${statusColor(r.status)}`}>
                    {r.status}
                  </span>
                </div>
                <div className="mt-1 text-xs text-ink-dim line-clamp-2 leading-snug">
                  {r.spec}
                </div>
                <div className="mt-1 flex items-center gap-1 text-[10px] text-ink-faint">
                  <Clock size={9} />
                  {new Date(r.started_at).toLocaleString(undefined, {
                    month: "short",
                    day: "numeric",
                    hour: "2-digit",
                    minute: "2-digit",
                  })}
                  {(r.total_tokens ?? 0) > 0 && (
                    <span className="ml-auto font-mono">
                      {fmtTok(r.total_tokens ?? 0)}
                      {r.cost_usd != null && ` · $${r.cost_usd.toFixed(2)}`}
                    </span>
                  )}
                </div>
              </button>
            )
          })
        )}
      </div>
    </aside>
  )
}

function statusDot(s: string) {
  return (
    {
      done: "bg-emerald-400",
      running: "bg-blue-400 animate-pulse",
      failed: "bg-red-400",
      cancelled: "bg-amber-400",
    } as Record<string, string>
  )[s] || "bg-ink-faint"
}

function statusColor(s: string) {
  return (
    {
      done: "text-emerald-400",
      running: "text-blue-400",
      failed: "text-red-400",
      cancelled: "text-amber-400",
    } as Record<string, string>
  )[s] || "text-ink-faint"
}
