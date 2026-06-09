import { Play, X, Check, Loader2, AlertTriangle, Terminal } from "lucide-react"
import { useState } from "react"
import type { Action } from "../types"
import { LogPane } from "./LogPane"

export function ActionCard({
  action,
  onDecide,
}: {
  action: Action
  onDecide: (decision: "approve" | "reject") => void
}) {
  // Default-expand the output when an action resolves — the inline-result
  // pattern from 2025 chat-UX guidance. A second click hides it.
  const [showOutput, setShowOutput] = useState(true)
  const status = action.status ?? "pending"

  const verbColor =
    {
      run: "text-status-info border-blue-500/30 bg-blue-500/5",
      approve: "text-status-warning border-amber-500/30 bg-amber-500/5",
      status: "text-ink-dim border-line bg-bg-panel",
      rerun: "text-purple-300 border-purple-500/30 bg-purple-500/5",
      plan_validate: "text-status-success border-emerald-500/30 bg-emerald-500/5",
      work: "text-orange-300 border-orange-500/30 bg-orange-500/5",
    }[action.verb] || "text-ink-dim border-line bg-bg-panel"

  const isResolved = status === "done" || status === "failed" || status === "rejected"

  return (
    <div className={`rounded-lg border p-3 ${verbColor}`}>
      <div className="flex items-center gap-2">
        <span className="text-xs uppercase tracking-wider font-semibold">
          {action.verb}
        </span>
        <code className="font-mono text-xs text-ink truncate">{action.label}</code>

        <span className="ml-auto flex items-center gap-1.5">
          {status === "pending" && (
            <>
              <button
                onClick={() => onDecide("approve")}
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-xs font-medium bg-emerald-500/20 hover:bg-emerald-500/30 text-status-success border border-emerald-500/30"
              >
                <Play size={11} /> run
              </button>
              <button
                onClick={() => onDecide("reject")}
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-xs text-ink-faint hover:text-ink hover:bg-bg-hover border border-line"
              >
                <X size={11} /> skip
              </button>
            </>
          )}
          {status === "running" && (
            <span className="inline-flex items-center gap-1 text-xs text-status-info">
              <Loader2 size={11} className="animate-spin" /> running…
            </span>
          )}
          {status === "done" && (
            <span className="inline-flex items-center gap-1 text-xs text-status-success">
              <Check size={11} /> done
            </span>
          )}
          {status === "failed" && (
            <span className="inline-flex items-center gap-1 text-xs text-status-danger">
              <AlertTriangle size={11} /> failed
            </span>
          )}
          {status === "rejected" && (
            <span className="inline-flex items-center gap-1 text-xs text-ink-faint">
              <X size={11} /> rejected
            </span>
          )}
        </span>
      </div>

      {/* Some actions (e.g. `verb: status`) have no args; the server returns
          `args: null` and Object.keys(null) used to crash the whole chat view.
          Guard the read so an argless action just renders without the grid. */}
      {action.args && Object.keys(action.args).length > 0 && (
        <div className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[11px]">
          {Object.entries(action.args).map(([k, v]) => (
            <span key={k} className="contents">
              <span className="text-ink-faint">{k}</span>
              <span className="font-mono text-ink-dim truncate">{v}</span>
            </span>
          ))}
        </div>
      )}

      {isResolved && action.output && (
        <div className="mt-2">
          <button
            onClick={() => setShowOutput((v) => !v)}
            className="inline-flex items-center gap-1 text-[11px] text-ink-faint hover:text-ink-dim"
          >
            <Terminal size={10} />
            {showOutput ? "hide output" : "show output"}
          </button>
          {showOutput && <LogPane text={action.output} className="mt-1" follow={false} />}
        </div>
      )}
      {/* Honest feedback when the server resolved the action without any
          captured output: a silent "done" with no inline log used to read
          as broken — say so explicitly so the user knows the action ran. */}
      {isResolved && !action.output && (
        <div className="mt-2 text-[11px] text-ink-faint italic">
          {status === "failed"
            ? "(action failed with no captured output)"
            : status === "rejected"
              ? "(skipped — no output)"
              : "(action acknowledged · no output)"}
        </div>
      )}
    </div>
  )
}
