import { useEffect, useState } from "react"
import { ShieldAlert } from "lucide-react"
import type { Finding } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"
import { severityToken } from "../ui/StatusChip"
import { CollapsibleSection } from "../ui/CollapsibleSection"

// Auto-expand (F-UI-003 §6 matrix) when findings need handling: a high/critical
// severity, or a refute/doctor finding. Low/info-only findings stay collapsed.
const needsAction = (f: Finding) =>
  f.severity === "high" || f.severity === "critical" || f.kind === "refute" || f.kind === "doctor"

/**
 * Run-detail finding-ledger surface (F-110): a count header + compact list of the
 * findings (risk / refute / doctor / …) a human reviewer should see. `refreshKey`
 * re-fetches as the run progresses so findings produced mid-run appear without a
 * manual reload. Renders nothing when the run has no findings, to stay out of the
 * way on clean runs.
 */
export function FindingsPanel({
  runId,
  refreshKey,
}: {
  runId: string | null
  refreshKey?: string | number | null
}) {
  const [findings, setFindings] = useState<Finding[]>([])

  // Clear on run change ONLY (not on refreshKey): switching A->B must not show
  // run A's HOTL findings under run B while B's request is in flight, but a
  // same-run refresh tick should update in place without flickering to empty.
  useEffect(() => {
    setFindings([])
  }, [runId])

  useEffect(() => {
    if (!runId) return
    let cancelled = false
    api
      .runFindings(runId)
      .then((f) => {
        if (!cancelled) setFindings(f)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [runId, refreshKey])

  if (findings.length === 0) return null
  const actionable = findings.some(needsAction)

  return (
    <CollapsibleSection
      title={t("tasks.findings")}
      icon={<ShieldAlert size={12} className="shrink-0 text-ink-faint" />}
      summary={t("tasks.findingsCount", { n: findings.length })}
      defaultOpen={actionable}
    >
      <div data-pane="findings" className="divide-y divide-line/40">
        {findings.slice(0, 20).map((f) => (
          <div key={f.finding_id} className="px-4 py-2 flex items-start gap-2.5">
            <span
              className={`mt-1 h-2 w-2 shrink-0 rounded-full ${severityToken(f.severity).dot}`}
              title={f.severity}
            />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5 text-xs">
                <span className="font-mono text-ink-dim shrink-0">{f.kind}</span>
                {f.task_id && (
                  <>
                    <span className="text-ink-faint">·</span>
                    <span className="font-mono text-ink-faint shrink-0 truncate">
                      {f.task_id}
                    </span>
                  </>
                )}
              </div>
              <div className="mt-0.5 text-[11px] text-ink-mute">{f.summary}</div>
            </div>
            <span className="shrink-0 self-center text-[9px] uppercase tracking-wider text-ink-faint">
              {f.severity}
            </span>
          </div>
        ))}
      </div>
    </CollapsibleSection>
  )
}
