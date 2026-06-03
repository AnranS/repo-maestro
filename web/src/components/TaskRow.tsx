import { useEffect, useRef, useState } from "react"
import {
  ChevronRight,
  ChevronDown,
  ExternalLink,
  Pause,
  Sparkles,
  Brain,
  Activity,
  User,
  Check,
  X,
  Eye,
  ShieldAlert,
  GitCompare,
  Footprints,
  AlertTriangle,
} from "lucide-react"
import type { TaskDiff, TaskState, TaskTrajectory } from "../types"
import { api } from "../api"
import { t } from "../i18n"
import { LogPane } from "./LogPane"

/** Approve/reject for a task at a `requires_approval_after` gate, with a
 *  "review changes" expander showing the real diff + a risk verdict — so the
 *  approval is an actual review, not a rubber-stamp (anti approval-fatigue).
 *  Writes the same markers `maestro approve` does. */
function ApprovalButtons({ task }: { task: string }) {
  const [busy, setBusy] = useState(false)
  const [open, setOpen] = useState(false)
  const [diff, setDiff] = useState<TaskDiff | null>(null)
  const [loading, setLoading] = useState(false)

  const review = async (e: React.MouseEvent) => {
    e.stopPropagation()
    const next = !open
    setOpen(next)
    if (next && !diff) {
      setLoading(true)
      try {
        setDiff(await api.taskDiff("current", task))
      } catch {
        /* show buttons even if diff fails */
      } finally {
        setLoading(false)
      }
    }
  }
  const decide = async (e: React.MouseEvent, decision: "approve" | "reject") => {
    e.stopPropagation()
    if (busy) return
    if (decision === "reject" && !confirm(t("approval.rejectConfirm"))) return
    setBusy(true)
    try {
      await api.runApprove("current", task, decision)
    } finally {
      setBusy(false)
    }
  }
  const high = diff?.risk.level === "high"
  return (
    <span className="inline-flex flex-col items-start gap-1" onClick={(e) => e.stopPropagation()}>
      <span className="inline-flex items-center gap-1">
        <button
          onClick={review}
          className="inline-flex items-center gap-1 rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-dim hover:bg-bg-hover hover:text-ink"
        >
          <Eye size={10} /> {t("approval.review")}
          {diff && (
            <span
              className={`ml-1 inline-flex items-center gap-0.5 rounded px-1 ${
                high ? "bg-red-500/15 text-red-300" : "bg-emerald-500/15 text-emerald-300"
              }`}
            >
              {high && <ShieldAlert size={9} />}
              {t(`approval.risk.${diff.risk.level}`)} · {diff.files.length}f
            </span>
          )}
        </button>
        <button
          onClick={(e) => decide(e, "approve")}
          disabled={busy}
          className="inline-flex items-center gap-1 rounded border border-emerald-500/40 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] text-emerald-300 hover:bg-emerald-500/20 disabled:opacity-50"
        >
          <Check size={10} /> {t("approval.approve")}
        </button>
        <button
          onClick={(e) => decide(e, "reject")}
          disabled={busy}
          className="inline-flex items-center gap-1 rounded border border-red-500/40 bg-red-500/10 px-1.5 py-0.5 text-[10px] text-red-300 hover:bg-red-500/20 disabled:opacity-50"
        >
          <X size={10} /> {t("approval.reject")}
        </button>
      </span>
      {open && (
        <div className="mt-1 w-full max-w-[40rem] rounded-lg border border-line bg-bg-panel p-2 text-[11px]">
          {loading && <div className="text-ink-faint">{t("approval.loadingDiff")}</div>}
          {diff && (
            <>
              <div
                className={`mb-1.5 flex items-start gap-1.5 ${high ? "text-red-300" : "text-ink-mute"}`}
              >
                {high && <ShieldAlert size={12} className="mt-0.5 shrink-0" />}
                <span>{diff.risk.reasons.join(" · ")}</span>
              </div>
              {diff.diff ? (
                <pre className="max-h-72 overflow-auto rounded bg-bg-inset p-2 font-mono text-[10px] leading-relaxed text-ink-mute whitespace-pre">
                  {diff.diff}
                </pre>
              ) : (
                <div className="text-ink-faint">{t("approval.noDiff")}</div>
              )}
            </>
          )}
        </div>
      )}
    </span>
  )
}

/** A task's receipt: which files it changed (durable, from recorded artifacts)
 *  + an on-demand diff (the same /diff endpoint the approval card uses) so any
 *  task answers "what did this actually do?" with links to the change. */
function ChangesReceipt({ task }: { task: TaskState }) {
  const [diff, setDiff] = useState<TaskDiff | null>(null)
  const [show, setShow] = useState(false)
  const [loading, setLoading] = useState(false)
  const files = task.artifacts?.files_changed ?? []
  const branch = task.artifacts?.branch

  const toggle = async () => {
    const next = !show
    setShow(next)
    if (next && !diff) {
      setLoading(true)
      try {
        setDiff(await api.taskDiff("current", task.id))
      } catch {
        /* worktree may be gone post-run — the file list above still stands */
      } finally {
        setLoading(false)
      }
    }
  }

  if (files.length === 0 && task.status !== "done" && task.status !== "failed") return null
  return (
    <div className="text-[11px]">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-ink-mute">
        <span className="inline-flex items-center gap-1 text-ink-dim">
          <GitCompare size={11} /> {t("task.changed", { n: files.length })}
        </span>
        {branch && <span className="font-mono text-ink-faint">{branch}</span>}
        {files.length > 0 && (
          <button onClick={toggle} className="text-blue-400 hover:text-blue-300">
            {show ? t("task.hideDiff") : t("task.viewDiff")}
          </button>
        )}
      </div>
      {files.length > 0 && (
        <div className="mt-0.5 font-mono text-[10px] text-ink-faint break-all">
          {files.slice(0, 12).join(", ")}
          {files.length > 12 ? ` … +${files.length - 12}` : ""}
        </div>
      )}
      {show && (
        <div className="mt-1">
          {loading && <div className="text-ink-faint">{t("approval.loadingDiff")}</div>}
          {diff &&
            (diff.diff ? (
              <pre className="max-h-72 overflow-auto rounded bg-bg-inset p-2 font-mono text-[10px] leading-relaxed text-ink-mute whitespace-pre">
                {diff.diff}
              </pre>
            ) : (
              <div className="text-ink-faint">{t("approval.noDiff")}</div>
            ))}
        </div>
      )}
    </div>
  )
}

const BUCKET_COLOR: Record<string, string> = {
  read: "text-sky-300",
  search: "text-violet-300",
  edit: "text-emerald-300",
  run: "text-amber-300",
  git: "text-orange-300",
  other: "text-ink-faint",
}

/** A task's trajectory: how the agent got to the diff — its tool calls bucketed
 *  (reads / searches / edits / runs) with the step-by-step command list on
 *  demand. Surfaces the "harness effect": same diff, very different paths. */
function TrajectoryReceipt({ task }: { task: TaskState }) {
  const [traj, setTraj] = useState<TaskTrajectory | null>(null)
  const [show, setShow] = useState(false)
  const [loading, setLoading] = useState(false)
  // Only agent tasks that actually ran have a trajectory worth showing.
  if (task.kind !== "agent" || !["done", "failed", "cancelled"].includes(task.status)) return null

  const toggle = async () => {
    const next = !show
    setShow(next)
    if (next && !traj) {
      setLoading(true)
      try {
        setTraj(await api.taskTrajectory("current", task.id))
      } catch {
        /* no trajectory recorded (e.g. shell/mock) */
      } finally {
        setLoading(false)
      }
    }
  }

  return (
    <div className="text-[11px]">
      <button
        onClick={toggle}
        className="inline-flex items-center gap-1 text-ink-dim hover:text-ink"
      >
        <Footprints size={11} />
        {show ? t("traj.hide") : t("traj.show")}
      </button>
      {show && (
        <div className="mt-1">
          {loading && <div className="text-ink-faint">{t("approval.loadingDiff")}</div>}
          {traj &&
            (traj.total_steps === 0 ? (
              <div className="text-ink-faint">{t("traj.none")}</div>
            ) : (
              <>
                <div className="mb-1 flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span className="text-ink-mute">{t("traj.steps", { n: traj.total_steps })}</span>
                  {Object.entries(traj.buckets).map(([b, n]) => (
                    <span key={b} className={`font-mono ${BUCKET_COLOR[b] ?? "text-ink-faint"}`}>
                      {n} {b}
                    </span>
                  ))}
                </div>
                <ol className="max-h-72 space-y-0.5 overflow-auto rounded bg-bg-inset p-2 font-mono text-[10px] leading-relaxed">
                  {traj.steps.map((s) => (
                    <li key={s.seq} className="flex gap-2">
                      <span className={`shrink-0 ${BUCKET_COLOR[s.bucket] ?? "text-ink-faint"}`}>
                        {s.bucket}
                      </span>
                      <span className="text-ink-mute break-all">{s.command}</span>
                    </li>
                  ))}
                  {traj.truncated && <li className="text-ink-faint">… {t("traj.truncated")}</li>}
                </ol>
              </>
            ))}
        </div>
      )}
    </div>
  )
}

export function TaskRow({ task }: { task: TaskState }) {
  const [open, setOpen] = useState(false)
  const [log, setLog] = useState<string>("")
  const esRef = useRef<EventSource | null>(null)

  useEffect(() => {
    if (!open) {
      esRef.current?.close()
      esRef.current = null
      return
    }
    setLog("")
    const url = `/api/logs/current/${encodeURIComponent(task.id)}/stream`
    const es = new EventSource(url)
    es.addEventListener("log", (e: MessageEvent) => setLog(e.data))
    es.addEventListener("delta", (e: MessageEvent) =>
      setLog((prev) => prev + e.data),
    )
    esRef.current = es
    return () => {
      es.close()
    }
  }, [open, task.id])

  return (
    <div>
      <div
        onClick={() => setOpen(!open)}
        className="px-4 py-2.5 flex items-center gap-3 cursor-pointer hover:bg-bg-hover/40"
      >
        {open ? (
          <ChevronDown size={13} className="text-ink-faint" />
        ) : (
          <ChevronRight size={13} className="text-ink-faint" />
        )}

        <span className={`w-4 text-center text-base ${textColor(task.status)}`}>
          {icon(task.status)}
        </span>

        <span className="font-mono text-sm flex-1 truncate">{task.id}</span>

        <span className="text-xs text-ink-mute hidden sm:inline">{task.project}</span>
        <span className="text-xs text-ink-faint hidden md:inline">{task.agent}</span>

        {task.role && (
          <span
            className={`text-[10px] px-1.5 py-0.5 rounded inline-flex items-center gap-1 border ${roleBadge(task.role)}`}
            title={`role: ${task.role}`}
          >
            <User size={9} />
            {roleShort(task.role)}
          </span>
        )}

        {task.kind === "verify" && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-purple-500/15 text-purple-300 border border-purple-500/20 inline-flex items-center gap-1">
            <Sparkles size={9} /> verify
          </span>
        )}
        {task.requires_approval_after && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 border border-amber-500/20 inline-flex items-center gap-1">
            <Pause size={9} /> gate
          </span>
        )}
        {task.risk_level === "high" && (
          <span
            className="text-[10px] px-1.5 py-0.5 rounded bg-red-500/15 text-red-300 border border-red-500/25 inline-flex items-center gap-1"
            title={t("risk.high.title")}
          >
            <AlertTriangle size={9} /> {t("risk.high")}
          </span>
        )}

        {task.status === "awaiting_approval" && <ApprovalButtons task={task.id} />}

        <span className={`text-[10px] px-2 py-0.5 rounded ${statusBadge(task.status)}`}>
          {task.status}
        </span>

        {typeof task.steps === "number" && task.steps > 0 && (
          <span
            className="text-[10px] text-ink-faint font-mono inline-flex items-center gap-0.5"
            title={`${task.steps} agent tool call${task.steps === 1 ? "" : "s"}`}
          >
            <Activity size={9} />
            {task.steps}
          </span>
        )}

        {task.usage && task.usage.input_tokens + task.usage.output_tokens > 0 && (
          <span
            className="text-[10px] text-ink-faint font-mono"
            title={`${task.usage.input_tokens} in + ${task.usage.output_tokens} out tokens${
              task.usage.cost_usd != null ? ` · $${task.usage.cost_usd.toFixed(4)}` : ""
            }`}
          >
            {(() => {
              const tot = task.usage.input_tokens + task.usage.output_tokens
              return tot > 1000 ? `${(tot / 1000).toFixed(1)}k` : `${tot}`
            })()}{" "}
            tok
          </span>
        )}

        <span className="text-[10px] text-ink-faint font-mono w-12 text-right">
          {duration(task)}
        </span>

        {task.artifacts?.pr_url && (
          <a
            href={task.artifacts.pr_url}
            target="_blank"
            rel="noreferrer"
            onClick={(e) => e.stopPropagation()}
            className="text-[10px] text-blue-400 hover:text-blue-300 inline-flex items-center gap-0.5"
          >
            Draft PR <ExternalLink size={9} />
          </a>
        )}
      </div>

      {open && (
        <div className="px-4 pb-3 pl-10 space-y-2">
          {(task.memory_used?.length || task.skills_triggered?.length) && (
            <div className="flex flex-wrap items-center gap-2 text-[11px]">
              {task.memory_used && task.memory_used.length > 0 && (
                <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-blue-500/10 text-blue-300 border border-blue-500/20">
                  <Brain size={10} />
                  memory:{" "}
                  <span className="font-mono">
                    {task.memory_used.join(", ")}
                  </span>
                </span>
              )}
              {task.skills_triggered && task.skills_triggered.length > 0 && (
                <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-purple-500/10 text-purple-300 border border-purple-500/20">
                  <Sparkles size={10} />
                  skill:{" "}
                  <span className="font-mono">
                    {task.skills_triggered.join(", ")}
                  </span>
                </span>
              )}
            </div>
          )}
          <ChangesReceipt task={task} />
          <TrajectoryReceipt task={task} />
          {(task.workspace_path || task.worktree_path) && (
            <div className="space-y-1 text-[11px] text-ink-faint font-mono">
              {task.workspace_path && <div>workspace: {task.workspace_path}</div>}
              {task.worktree_path && <div>worktree: {task.worktree_path}</div>}
            </div>
          )}
          <LogPane text={log} taskStatus={task.status} />
          {task.error && (
            <p className="mt-2 text-xs">
              <span className="text-red-400">error: </span>
              <span className="text-red-300">{task.error}</span>
            </p>
          )}
        </div>
      )}
    </div>
  )
}

function icon(s: string) {
  return (
    {
      done: "✓",
      running: "⟳",
      failed: "✗",
      pending: "○",
      awaiting_approval: "⏸",
      skipped: "—",
      cancelled: "⊘",
    } as Record<string, string>
  )[s] || "·"
}

function textColor(s: string) {
  return (
    {
      done: "text-emerald-400",
      running: "text-blue-400 animate-spin-slow",
      failed: "text-red-400",
      awaiting_approval: "text-amber-400",
      skipped: "text-ink-faint",
    } as Record<string, string>
  )[s] || "text-ink-dim"
}

function statusBadge(s: string) {
  return (
    {
      done: "bg-emerald-500/15 text-emerald-300 border border-emerald-500/20",
      running: "bg-blue-500/15 text-blue-300 border border-blue-500/20 animate-pulse",
      failed: "bg-red-500/15 text-red-300 border border-red-500/20",
      pending: "bg-bg-inset text-ink-dim border border-line",
      awaiting_approval: "bg-amber-500/15 text-amber-300 border border-amber-500/20",
      skipped: "bg-bg-inset text-ink-faint border border-line",
      cancelled: "bg-bg-inset text-ink-faint border border-line",
    } as Record<string, string>
  )[s] || "bg-bg-inset text-ink-dim border border-line"
}

function duration(t: TaskState) {
  if (!t.started_at) return ""
  const start = new Date(t.started_at).getTime()
  const end = t.ended_at ? new Date(t.ended_at).getTime() : Date.now()
  const ms = end - start
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

/// Color-code roles by family so a glance at the DAG shows which kind of
/// work each task is. Builder families pick warm tones; cross-cutting
/// roles (designer / architect / qa) get cool tones.
function roleBadge(role: string): string {
  if (role.startsWith("backend")) {
    return "bg-orange-500/10 text-orange-300 border-orange-500/25"
  }
  if (role.startsWith("frontend")) {
    return "bg-sky-500/10 text-sky-300 border-sky-500/25"
  }
  if (role === "mobile_ios") {
    return "bg-slate-500/15 text-slate-200 border-slate-500/30"
  }
  if (role === "mobile_android") {
    return "bg-lime-500/10 text-lime-300 border-lime-500/25"
  }
  if (role.startsWith("mobile")) {
    return "bg-pink-500/10 text-pink-300 border-pink-500/25"
  }
  if (role.startsWith("game")) {
    return "bg-violet-500/10 text-violet-300 border-violet-500/25"
  }
  if (role === "designer") {
    return "bg-fuchsia-500/10 text-fuchsia-300 border-fuchsia-500/25"
  }
  if (role === "architect") {
    return "bg-indigo-500/10 text-indigo-300 border-indigo-500/25"
  }
  if (role === "qa") {
    return "bg-emerald-500/10 text-emerald-300 border-emerald-500/25"
  }
  return "bg-bg-inset text-ink-dim border border-line"
}

/// Compact label so the badge fits on one row.
function roleShort(role: string): string {
  const map: Record<string, string> = {
    backend_rust: "BE-rs",
    backend_python: "BE-py",
    frontend: "FE",
    mobile_ios: "iOS",
    mobile_android: "Android",
    mobile_rn: "RN",
    game_unity: "Unity",
    game_cocos: "Cocos",
    designer: "Design",
    architect: "Arch",
    qa: "QA",
  }
  return map[role] || role
}
