import { useEffect, useRef, useState } from "react"
import { ArrowUp, Square, Eye, Play } from "lucide-react"
import type { ModelInfo } from "../types"

export type ChatMode = "plan" | "exec"

// `models` / `pinnedModel` were used by the inline per-message model
// override that lived inside the input row. That picker was removed
// because users couldn't tell it apart from the session-level pin
// (ModelPicker in the bar above) — see fix(chat): model picker selection.
// The props stay accepted (and ignored) to avoid churn at call sites
// while we settle on the final UX.
export function ChatInput({
  onSubmit,
  disabled,
  streaming,
  onStop,
}: {
  onSubmit: (
    text: string,
    modelOverride?: string | null,
    mode?: ChatMode,
  ) => void
  disabled?: boolean
  streaming?: boolean
  onStop?: () => void
  models?: ModelInfo[]
  pinnedModel?: string | null
}) {
  const [text, setText] = useState("")
  /**
   * Plan = the orchestrator analyses but does NOT emit action blocks.
   * Exec = full agency (default — current behaviour).
   * The mode is sticky across messages until you toggle it again, so a
   * "let me think about this" plan turn naturally fans into multiple
   * back-and-forths before you flip to exec.
   */
  const [mode, setMode] = useState<ChatMode>("exec")
  const ref = useRef<HTMLTextAreaElement>(null)

  // auto-resize
  useEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = "0px"
    el.style.height = Math.min(el.scrollHeight, 240) + "px"
  }, [text])

  const submit = () => {
    const t = text.trim()
    if (!t || disabled) return
    // Always send `null` as the model override — the session-level pin
    // in ModelPicker is what governs which model the agent uses.
    onSubmit(t, null, mode)
    setText("")
    // Note: do NOT reset `mode` — see the comment on useState above.
  }

  return (
    <div className="relative">
      <textarea
        ref={ref}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault()
            submit()
          }
        }}
        placeholder="message the orchestrator…"
        rows={1}
        disabled={disabled && !streaming}
        className="w-full resize-none bg-bg-panel border border-line rounded-2xl px-4 py-3 pr-12 text-sm leading-relaxed focus:outline-none focus:border-line-soft scrollbar-thin"
      />

      <div className="absolute right-2 bottom-2 flex items-center gap-1.5">
        {/* Plan / Exec mode toggle */}
        {!streaming && (
          <div className="flex items-center bg-bg-inset border border-line rounded-md p-0.5">
            <button
              onClick={() => setMode("plan")}
              className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium transition-colors ${
                mode === "plan"
                  ? "bg-amber-500/20 text-amber-200"
                  : "text-ink-faint hover:text-ink-dim"
              }`}
              title="plan mode — analyse only, no action blocks"
            >
              <Eye size={10} /> plan
            </button>
            <button
              onClick={() => setMode("exec")}
              className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium transition-colors ${
                mode === "exec"
                  ? "bg-emerald-500/20 text-emerald-200"
                  : "text-ink-faint hover:text-ink-dim"
              }`}
              title="exec mode — orchestrator can emit action blocks"
            >
              <Play size={10} /> exec
            </button>
          </div>
        )}

        {streaming ? (
          <button
            onClick={onStop}
            className="w-8 h-8 rounded-lg bg-bg-hover border border-line flex items-center justify-center text-ink-dim hover:text-ink"
            title="stop"
          >
            <Square size={12} fill="currentColor" />
          </button>
        ) : (
          <button
            onClick={submit}
            disabled={!text.trim()}
            className="w-8 h-8 rounded-lg bg-ink text-bg flex items-center justify-center disabled:bg-bg-hover disabled:text-ink-faint transition-colors"
            title="send"
          >
            <ArrowUp size={14} />
          </button>
        )}
      </div>
    </div>
  )
}
