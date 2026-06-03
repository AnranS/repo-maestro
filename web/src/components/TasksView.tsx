import { useEffect, useState } from "react"
import { MessageSquare, Square, Terminal, ChevronDown, ChevronUp } from "lucide-react"
import type { RunEvidence, RunReplay, RunState, TaskState } from "../types"
import { api } from "../api"
import { t } from "../i18n"
import { DagView } from "./DagView"
import { TaskRow } from "./TaskRow"
import { GoalPanel } from "./GoalPanel"
import { EvidencePanel } from "./tasks/EvidencePanel"
import { CoordinationPanel } from "./tasks/CoordinationPanel"
import { FindingsPanel } from "./tasks/FindingsPanel"
import { RunTimeline } from "./tasks/RunTimeline"
import { AutoActionsPanel, CostPanel, RunSummaryStrip, StatusPill, taskSummary } from "./tasks/RunSummary"
import { FailureRecoveryPanel } from "./tasks/FailureRecoveryPanel"
import { GateBanner } from "./tasks/GateBanner"
import { OutcomePanel } from "./tasks/OutcomePanel"

interface Props {
  state: RunState | null
  onJumpToSession?: (id: string) => void
}

export function TasksView({ state, onJumpToSession }: Props) {
  const [cancelling, setCancelling] = useState(false)
  const [titleExpanded, setTitleExpanded] = useState(false)
  const [taskTab, setTaskTab] = useState<"graph" | "lanes" | "list" | "timeline">("graph")
  const [evidence, setEvidence] = useState<RunEvidence | null>(null)
  const [replay, setReplay] = useState<RunReplay | null>(null)
  const [prBody, setPrBody] = useState("")

  useEffect(() => {
    setEvidence(null)
    setReplay(null)
    setPrBody("")
    if (!state?.run_id || state.status === "running") return
    let cancelled = false
    Promise.allSettled([
      api.runEvidence(state.run_id),
      api.runReplay(state.run_id),
      api.runPrBody(state.run_id),
    ])
      .then((results) => {
        if (cancelled) return
        const [evidenceResult, replayResult, prBodyResult] = results
        if (evidenceResult.status === "fulfilled") setEvidence(evidenceResult.value)
        if (replayResult.status === "fulfilled") setReplay(replayResult.value)
        if (prBodyResult.status === "fulfilled") setPrBody(prBodyResult.value)
      })
      .catch(() => {
        if (!cancelled) setEvidence(null)
      })
    return () => {
      cancelled = true
    }
  }, [state?.run_id, state?.status])

  if (!state) {
    return (
      <div className="flex-1 overflow-y-auto scrollbar-thin">
        <div className="max-w-4xl mx-auto px-6 py-16">
          <div className="border border-line bg-bg-panel rounded-lg p-6">
            <div className="flex items-start gap-4">
              <div className="mt-0.5 flex h-10 w-10 items-center justify-center rounded-md border border-blue-500/30 bg-blue-500/10 text-blue-300">
                <Terminal size={18} />
              </div>
              <div className="min-w-0 flex-1">
                <h1 className="text-base font-semibold text-ink">{t("tasks.emptyTitle")}</h1>
                <p className="mt-1 max-w-2xl text-sm leading-6 text-ink-dim">
                  {t("tasks.emptyBody")}
                </p>
                <div className="mt-4 grid gap-2 text-xs md:grid-cols-3">
                  <CommandHint label={t("tasks.emptyDemo")} command="maestro demo --run" />
                  <CommandHint label={t("tasks.emptyPlan")} command="maestro run .maestro/PLAN.yaml" />
                  <CommandHint label={t("tasks.emptyScan")} command={'maestro work "<goal>" --root .'} />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    )
  }

  const isRunning = state.status === "running"

  const handleCancel = async () => {
    if (!confirm(`Cancel run ${state.run_id}?`)) return
    setCancelling(true)
    try {
      await api.cancelRun(state.run_id)
    } catch (e) {
      console.error("cancel run failed:", e)
      window.alert(`Failed to cancel run: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setCancelling(false)
    }
  }

  const orderedIds =
    state.task_order && state.task_order.length > 0
      ? state.task_order
      : Object.keys(state.tasks)
  const tasks: TaskState[] = orderedIds.map((id) => state.tasks[id]).filter(Boolean)
  const summary = taskSummary(tasks, state)

  return (
    <div className="flex-1 overflow-y-auto scrollbar-thin">
      <div className="max-w-6xl mx-auto p-6 space-y-6">
        {/* Run header / actions */}
        <div className="flex flex-col gap-3 lg:flex-row lg:items-start">
          <div className="flex-1 min-w-0">
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <StatusPill status={state.status} />
              <span className="text-[11px] text-ink-faint">
                {t("tasks.completePct", { n: summary.percent })}
              </span>
            </div>
            {(() => {
              const full = state.spec.trim()
              const short = shortTitle(full)
              const truncated = short !== full
              if (!truncated) {
                return <h1 className="text-lg font-semibold leading-tight text-ink">{full}</h1>
              }
              return (
                <button
                  type="button"
                  onClick={() => setTitleExpanded((v) => !v)}
                  title={titleExpanded ? t("tasks.collapse") : t("tasks.expandTitle")}
                  className="group flex w-full items-start gap-1.5 text-left"
                >
                  <h1
                    className={`text-lg font-semibold leading-tight text-ink ${titleExpanded ? "" : "line-clamp-2"}`}
                  >
                    {titleExpanded ? full : short}
                  </h1>
                  {titleExpanded ? (
                    <ChevronUp size={16} className="mt-1 shrink-0 text-ink-faint group-hover:text-ink-dim" />
                  ) : (
                    <ChevronDown size={16} className="mt-1 shrink-0 text-ink-faint group-hover:text-ink-dim" />
                  )}
                </button>
              )
            })()}
            <div className="mt-1 text-[11px] text-ink-faint font-mono flex flex-wrap items-center gap-3">
              <span>{state.run_id}</span>
              {state.session_id && onJumpToSession && (
                <button
                  onClick={() => onJumpToSession(state.session_id!)}
                  className="inline-flex items-center gap-1 hover:text-ink-dim"
                  title={t("tasks.openLaunchingSession")}
                >
                  <MessageSquare size={10} />
                  {t("tasks.fromSession", { id: state.session_id.slice(0, 8) })}
                </button>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span className="hidden text-xs text-ink-faint lg:inline">
              {t("tasks.maxParallel", { n: state.max_parallel })}
            </span>
            {isRunning && (
              <button
                onClick={handleCancel}
                disabled={cancelling}
                className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-red-500/15 text-red-200 hover:bg-red-500/25 border border-red-500/30 disabled:opacity-50"
                title={t("tasks.cancelRun")}
              >
                <Square size={11} fill="currentColor" />
                {cancelling ? t("tasks.cancelling") : t("tasks.cancelRun")}
              </button>
            )}
          </div>
        </div>

        <GateBanner state={state} />

        <RunSummaryStrip summary={summary} />

        <FailureRecoveryPanel state={state} />

        <CostPanel state={state} />

        <AutoActionsPanel actions={state.auto_actions} />

        <GoalPanel
          goal={state.goal}
          results={state.acceptance_results}
          verified={state.verified}
          status={state.status}
        />

        {(state.pending_gate === "outcome" ||
          ["done", "failed", "cancelled"].includes(state.status)) && (
          <OutcomePanel
            runId={state.run_id}
            autoLoad={state.pending_gate === "outcome"}
          />
        )}

        {evidence && <EvidencePanel evidence={evidence} replay={replay} prBody={prBody} />}

        <FindingsPanel
          runId={state.run_id}
          refreshKey={`${state.run_id}:${summary.terminal}:${summary.running}`}
        />

        <CoordinationPanel
          refreshKey={`${state.run_id}:${summary.terminal}:${summary.running}`}
        />

        {/* One panel for tasks — graph / list / timeline are three lenses on
            the same set, so they share a header instead of stacking. */}
        <section className="bg-bg-panel border border-line rounded-xl">
          <div className="px-4 py-2.5 border-b border-line flex items-center gap-3">
            <div className="flex items-center rounded-lg border border-line bg-bg-inset p-0.5">
              {(["graph", "lanes", "list", "timeline"] as const).map((k) => (
                <button
                  key={k}
                  onClick={() => setTaskTab(k)}
                  className={`rounded px-2 py-1 text-[11px] ${taskTab === k ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
                >
                  {k === "graph"
                    ? t("tasks.depGraph")
                    : k === "lanes"
                      ? t("tasks.lanes")
                      : k === "list"
                        ? t("tasks.tasks")
                        : t("tasks.timeline")}
                </button>
              ))}
            </div>
            <span className="ml-auto text-xs text-ink-faint">
              {t("tasks.taskCountActive", { n: tasks.length, active: summary.activeLabel })}
            </span>
          </div>
          {taskTab === "graph" && (
            <div className="p-4">
              <DagView tasks={tasks} />
            </div>
          )}
          {taskTab === "lanes" && (
            <div className="p-4">
              <LanesView tasks={tasks} />
            </div>
          )}
          {taskTab === "list" && (
            <div className="divide-y divide-line/40">
              {tasks.map((task) => (
                <TaskRow key={task.id} task={task} />
              ))}
            </div>
          )}
          {taskTab === "timeline" && (
            <div className="p-4">
              <RunTimeline tasks={tasks} embedded />
            </div>
          )}
        </section>
      </div>
    </div>
  )
}

/** A glanceable run title from the raw spec: the first sentence, capped in
 *  length. The full spec stays available on hover and in the Goal panel, so we
 *  don't render the whole multi-sentence paragraph as the heading. */
function shortTitle(spec: string): string {
  const trimmed = spec.trim()
  // first sentence (stop at . 。 ! ! ? ? or newline)
  const first = trimmed.split(/(?<=[.。!！?？])\s|\n/)[0]?.trim() || trimmed
  const cap = 100
  if (first.length <= cap) return first
  return first.slice(0, cap).trimEnd() + "…"
}

function CommandHint({ label, command }: { label: string; command: string }) {
  return (
    <div className="rounded-md border border-line bg-bg-inset p-3">
      <div className="mb-1 text-[10px] uppercase tracking-wider text-ink-faint">{label}</div>
      <code className="block truncate font-mono text-[11px] text-ink-dim">{command}</code>
    </div>
  )
}

/** Swimlanes: one lane per project, tasks as status chips, so you can see at a
 *  glance which projects' agents are running in parallel (Cursor-style). */
function laneChip(status: string): string {
  return (
    {
      done: "border-emerald-500/30 bg-emerald-500/10 text-emerald-300",
      running: "border-blue-500/40 bg-blue-500/15 text-blue-300",
      failed: "border-red-500/30 bg-red-500/10 text-red-300",
      awaiting_approval: "border-amber-500/30 bg-amber-500/10 text-amber-300",
      cancelled: "border-line bg-bg-inset text-ink-dim",
      skipped: "border-line bg-bg-inset text-ink-faint",
    }[status] ?? "border-line bg-bg-inset text-ink-faint"
  )
}

function LanesView({ tasks }: { tasks: TaskState[] }) {
  const byProject = new Map<string, TaskState[]>()
  for (const tk of tasks) {
    const k = tk.project || "—"
    const arr = byProject.get(k) ?? []
    arr.push(tk)
    byProject.set(k, arr)
  }
  const lanes = [...byProject.entries()].sort((a, b) => a[0].localeCompare(b[0]))
  if (lanes.length === 0) return null
  return (
    <div className="space-y-2">
      {lanes.map(([project, ts]) => {
        const running = ts.filter((x) => x.status === "running").length
        return (
          <div key={project} className="flex items-start gap-3">
            <div
              className="w-32 shrink-0 truncate pt-1 text-[12px] font-medium text-ink-dim"
              title={project}
            >
              {project}
              {running > 0 && (
                <span className="ml-1 inline-block h-1.5 w-1.5 rounded-full bg-blue-400 align-middle animate-pulse" />
              )}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {ts.map((tk) => (
                <span
                  key={tk.id}
                  title={`${tk.id} · ${tk.status} · ${tk.agent}`}
                  className={`inline-flex items-center rounded border px-1.5 py-0.5 font-mono text-[10px] ${laneChip(
                    tk.status,
                  )} ${tk.status === "running" ? "animate-pulse" : ""}`}
                >
                  {tk.id.replace(/^T_(change_|verify_)?/, "")}
                </span>
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}
