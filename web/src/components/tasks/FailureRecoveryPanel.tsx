import { useState } from "react"
import { AlertTriangle, Check, Copy, GitBranch, RotateCw } from "lucide-react"
import type { RunState, TaskState } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"

const BLOCKED_PREFIX = "blocked by failed dependency"

/** A task skipped because an upstream task failed — the scheduler records the
 *  blocker id in `error` as "blocked by failed dependency: <id>". */
function blockerOf(task: TaskState): string | null {
  if (task.status !== "skipped" || !task.error?.startsWith(BLOCKED_PREFIX)) return null
  const idx = task.error.indexOf(":")
  return idx === -1 ? null : task.error.slice(idx + 1).trim()
}

/** Short, single-line error for a failed task — the first line, capped. */
function shortError(err?: string): string {
  if (!err) return ""
  const first = err.split("\n")[0].trim()
  return first.length > 160 ? first.slice(0, 160) + "…" : first
}

/**
 * Recovery view for a failed run: which tasks failed, which downstream tasks
 * they blocked, and the exact `maestro rerun` command to retry — rerun seeds
 * the tasks that already succeeded and only re-runs the failed + blocked ones,
 * so recovery is one command, not a full redo. Renders nothing for runs with
 * no failures.
 */
export function FailureRecoveryPanel({ state }: { state: RunState }) {
  const [copied, setCopied] = useState(false)
  const [rerun, setRerun] = useState<"idle" | "launching" | "launched" | "error">("idle")
  const tasks = Object.values(state.tasks)
  const failed = tasks.filter((tk) => tk.status === "failed")
  if (failed.length === 0) return null

  // Map each failed task to the downstream tasks blocked because of it. The
  // scheduler marks blocked tasks transitively, but each records its *direct*
  // blocker, so we walk the chain back to the originating failed task.
  const blocked = tasks.filter((tk) => blockerOf(tk) !== null)
  const byId = new Map(tasks.map((tk) => [tk.id, tk]))
  const rootFailureOf = (id: string, seen = new Set<string>()): string | null => {
    if (seen.has(id)) return null
    seen.add(id)
    const tk = byId.get(id)
    if (!tk) return null
    if (tk.status === "failed") return id
    const b = blockerOf(tk)
    return b ? rootFailureOf(b, seen) : null
  }
  const blockedByFailure = new Map<string, string[]>()
  for (const tk of blocked) {
    const root = rootFailureOf(tk.id)
    if (!root) continue
    const arr = blockedByFailure.get(root) ?? []
    arr.push(tk.id)
    blockedByFailure.set(root, arr)
  }

  const rerunCmd = `maestro rerun .maestro/runs/${state.run_id}/PLAN.yaml`
  const copy = async () => {
    await navigator.clipboard.writeText(rerunCmd)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1200)
  }
  const launchRerun = async () => {
    setRerun("launching")
    try {
      await api.rerunRun(state.run_id)
      setRerun("launched")
    } catch {
      setRerun("error")
      window.setTimeout(() => setRerun("idle"), 2500)
    }
  }

  return (
    <section className="rounded-lg border border-red-500/30 bg-red-500/5 p-3">
      <div className="mb-2 flex items-center gap-2">
        <span className="inline-flex h-5 w-5 items-center justify-center rounded border border-red-500/30 bg-red-500/10 text-red-300">
          <AlertTriangle size={12} />
        </span>
        <span className="text-[10px] uppercase tracking-wider text-red-300/80">
          {t("recover.title")}
        </span>
        <span className="text-[11px] text-ink-mute">
          {t("recover.summary", { failed: failed.length, blocked: blocked.length })}
        </span>
      </div>

      <ul className="space-y-2">
        {failed.map((tk) => {
          const blkd = blockedByFailure.get(tk.id) ?? []
          return (
            <li key={tk.id} className="rounded-md border border-line/60 bg-bg/40 p-2">
              <div className="flex flex-wrap items-center gap-2 text-[12px]">
                <code className="rounded bg-red-500/10 px-1.5 py-0.5 text-red-300">{tk.id}</code>
                <span className="text-ink-faint">{tk.project}</span>
                {tk.attempts != null && tk.attempts > 1 && (
                  <span className="text-[10px] text-ink-faint">
                    {t("recover.attempts", { n: tk.attempts })}
                  </span>
                )}
              </div>
              {tk.error && (
                <div className="mt-1 font-mono text-[11px] leading-snug text-ink-mute">
                  {shortError(tk.error)}
                </div>
              )}
              {blkd.length > 0 && (
                <div className="mt-1.5 flex flex-wrap items-center gap-1 text-[11px] text-ink-faint">
                  <GitBranch size={11} className="text-amber-400/80" />
                  {t("recover.blocked", { n: blkd.length })}
                  {blkd.map((id) => (
                    <code
                      key={id}
                      className="rounded bg-amber-500/10 px-1 py-0.5 text-amber-300/90"
                    >
                      {id}
                    </code>
                  ))}
                </div>
              )}
            </li>
          )
        })}
      </ul>

      <div className="mt-3 flex items-center gap-2">
        <button
          onClick={launchRerun}
          disabled={rerun === "launching" || rerun === "launched"}
          className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[12px] font-medium ${
            rerun === "launched"
              ? "border border-emerald-500/30 bg-emerald-500/10 text-emerald-300"
              : "border border-red-500/30 bg-red-500/15 text-red-200 hover:bg-red-500/25"
          } disabled:opacity-70`}
        >
          {rerun === "launched" ? (
            <Check size={13} />
          ) : (
            <RotateCw size={13} className={rerun === "launching" ? "animate-spin" : ""} />
          )}
          {rerun === "launching"
            ? t("recover.launching")
            : rerun === "launched"
              ? t("recover.launched")
              : rerun === "error"
                ? t("recover.error")
                : t("recover.rerun")}
        </button>
        <span className="text-[11px] text-ink-faint">{t("recover.hint")}</span>
      </div>
      <div className="mt-2 flex items-center gap-2">
        <span className="shrink-0 text-[10px] uppercase tracking-wider text-ink-faint">
          {t("recover.orTerminal")}
        </span>
        <code className="flex-1 truncate rounded border border-line/60 bg-bg/70 px-2 py-1 font-mono text-[11px] text-ink-dim">
          {rerunCmd}
        </code>
        <button
          onClick={copy}
          className="inline-flex shrink-0 items-center gap-1.5 rounded border border-line px-2 py-1 text-[11px] hover:border-accent/50 hover:text-ink-dim"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? t("common.copied") : t("common.copy")}
        </button>
      </div>
    </section>
  )
}
