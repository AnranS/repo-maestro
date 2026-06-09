import { useEffect, useState } from "react"
import { CheckCircle2, ChevronDown, ChevronRight, FileText, GitFork, ScrollText, XCircle } from "lucide-react"
import type { RunOutcome, RunOutcomeTask } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"
import { RiskChip } from "../ui/Chips"
import { LoadingState } from "../ui/StatePane"
import { statusTextColor } from "../ui/StatusChip"

/**
 * One screen to validate the whole run's outcome — goal, every acceptance
 * check, and the aggregate change across all tasks' worktrees (grouped per
 * task, expandable diff) with an overall risk verdict. Pairs with the outcome
 * gate: review the complete result here instead of a dozen separate task diffs.
 * Auto-loads at the outcome gate; loads on demand otherwise.
 */
export function OutcomePanel({ runId, autoLoad }: { runId: string; autoLoad?: boolean }) {
  const [outcome, setOutcome] = useState<RunOutcome | null>(null)
  const [loading, setLoading] = useState(false)
  const [open, setOpen] = useState(!!autoLoad)

  const load = async () => {
    setLoading(true)
    try {
      setOutcome(await api.runOutcome(runId))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    setOutcome(null)
    if (autoLoad) {
      setOpen(true)
      void load()
    } else {
      setOpen(false)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId, autoLoad])

  const toggle = () => {
    const next = !open
    setOpen(next)
    if (next && !outcome && !loading) void load()
  }

  const accTotal = outcome?.acceptance_results?.length ?? 0
  const accPassed = outcome?.acceptance_results?.filter((r) => r.passed).length ?? 0

  return (
    <section className="rounded-lg border border-line bg-bg-panel">
      <button
        onClick={toggle}
        className="flex w-full items-center gap-2 px-4 py-2.5 text-left"
      >
        <ScrollText size={14} className="text-ink-dim" />
        <span className="text-sm font-semibold text-ink">{t("outcome.title")}</span>
        {outcome && <RiskChip level={outcome.risk.level} />}
        {outcome && (
          <span className="text-[11px] text-ink-faint">
            {t("outcome.fileCount", { n: outcome.total_files })}
          </span>
        )}
        <span className="ml-auto text-ink-faint">
          {open ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
        </span>
      </button>

      {open && (
        <div className="border-t border-line px-4 py-3">
          {loading && <LoadingState label={t("approval.loadingDiff")} />}
          {!loading && outcome && (
            <div className="space-y-3">
              {accTotal > 0 && (
                <div>
                  <div className="mb-1 text-[10px] uppercase tracking-wider text-ink-faint">
                    {t("outcome.acceptance", { passed: accPassed, total: accTotal })}
                  </div>
                  <ul className="space-y-1">
                    {outcome.acceptance_results!.map((r, i) => (
                      <li key={i} className="flex items-start gap-1.5 text-[12px]">
                        {r.passed ? (
                          <CheckCircle2 size={13} className={`mt-0.5 shrink-0 ${statusTextColor("done")}`} />
                        ) : (
                          <XCircle size={13} className={`mt-0.5 shrink-0 ${statusTextColor("failed")}`} />
                        )}
                        <span className="text-ink-dim">{r.describe || r.check}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {outcome.drift && outcome.drift.length > 0 && (
                <div className="rounded-md border border-amber-500/40 bg-amber-500/10 p-2">
                  <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-status-warning">
                    <GitFork size={12} />
                    {t("outcome.drift", { n: outcome.drift.length })}
                  </div>
                  <ul className="space-y-1">
                    {outcome.drift.map((d, i) => (
                      <li key={i} className="text-[11px] leading-snug text-status-warning/85">
                        <code className="text-status-warning">{d.producer}</code>
                        {" → "}
                        <code className="text-status-warning">{d.consumer}</code>{" "}
                        <span className="text-ink-faint">({d.contract})</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {outcome.tasks.length === 0 ? (
                <div className="text-xs text-ink-faint">{t("outcome.noChanges")}</div>
              ) : (
                <div className="space-y-1.5">
                  <div className="text-[10px] uppercase tracking-wider text-ink-faint">
                    {t("outcome.changes")}
                  </div>
                  {outcome.tasks.map((tk) => (
                    <TaskChanges key={tk.task} task={tk} />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  )
}

function TaskChanges({ task }: { task: RunOutcomeTask }) {
  const [show, setShow] = useState(false)
  const high = task.risk.level === "high"
  return (
    <div className="rounded-md border border-line/60 bg-bg/40">
      <button
        onClick={() => setShow((v) => !v)}
        className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[12px]"
      >
        {show ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <code className="text-ink-dim">{task.task}</code>
        <span className="text-ink-faint">{task.project}</span>
        <span className="inline-flex items-center gap-1 text-[11px] text-ink-faint">
          <FileText size={11} />
          {task.files.length}
        </span>
        {high && <RiskChip level={task.risk.level} />}
      </button>
      {show && (
        <div className="border-t border-line/60 px-2.5 py-2">
          {task.diff ? (
            <pre className="max-h-80 overflow-auto rounded bg-bg/70 p-2 font-mono text-[10px] leading-relaxed text-ink-dim">
              {task.diff}
            </pre>
          ) : (
            <div className="text-[11px] text-ink-faint">{t("approval.noDiff")}</div>
          )}
        </div>
      )}
    </div>
  )
}
