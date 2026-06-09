// F-UI-001: the run-level health strip — the top band of the Run Inspector. It
// reads only existing RunState (no new data source); resume-safe has no WebUI
// data source yet (F-117 validate isn't exposed), so it shows a neutral
// "not wired" rather than faking a verdict. Needs-action metrics surface first.

import type { RunState } from "../types"
import { StatusChip } from "./ui/StatusChip"

export function RunHealthStrip({ state }: { state: RunState }) {
  const tasks = Object.values(state.tasks)
  const total = tasks.length
  const n = (s: string) => tasks.filter((t) => t.status === s).length
  const done = n("done")
  const running = n("running")
  const failed = n("failed")
  const approval = n("awaiting_approval")
  // A pending task whose dependency failed/cancelled is effectively blocked.
  const blocked = tasks.filter(
    (t) =>
      t.status === "pending" &&
      (t.depends_on ?? []).some((d) => {
        const dep = state.tasks[d]
        return dep && (dep.status === "failed" || dep.status === "cancelled")
      }),
  ).length
  const riskHigh = tasks.some((t) => t.risk_level === "high")

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-lg border border-line bg-bg-panel px-3 py-2 text-[11px]">
      <StatusChip status={state.status} />
      <span className="font-mono tabular-nums text-ink-dim">
        {done}/{total} <span className="text-ink-faint">done</span>
      </span>
      {running > 0 && (
        <span className="font-mono tabular-nums text-status-info">
          {running} <span className="text-ink-faint">running</span>
        </span>
      )}
      {/* needs-action metrics — only shown when non-zero, at full weight */}
      {blocked > 0 && <StatusChip status="blocked" label={`${blocked} blocked`} />}
      {approval > 0 && (
        <StatusChip status="awaiting_approval" label={`${approval} approval`} />
      )}
      {failed > 0 && <StatusChip status="failed" label={`${failed} failed`} />}
      {riskHigh && <StatusChip status="risk_high" />}
      {/* resume-safe: no WebUI data source yet (F-117 validate not exposed) */}
      <span className="ml-auto inline-flex items-center gap-1 rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-faint">
        resume-safe: not wired yet
      </span>
    </div>
  )
}
