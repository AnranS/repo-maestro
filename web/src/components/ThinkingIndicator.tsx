import { useEffect, useState } from "react"
import { Sparkles, ChevronDown, ChevronRight } from "lucide-react"

interface Props {
  /** Whether a stream is currently in flight. */
  streaming: boolean
  /** Wall-clock timestamp (ms) of when the current turn started. */
  startedAt: number | null
  /**
   * Accumulated reasoning text from the model, IF the backend forwards
   * any. With the new provider plumbing (2026-05) cursor + claude both
   * stream thinking text; codex still only reports the token count.
   */
  thinkingText: string
  /**
   * Visible answer text streamed so far. We only show the indicator
   * while this is empty — once the model commits to a real answer the
   * normal bubble takes over and we can step out of the way.
   */
  draftText: string
  /** Active provider so we can show a more specific status line. */
  provider?: string | null
}

/**
 * Tiny "model is thinking" pill that lives above the empty streaming
 * bubble while a reasoning model is mulling before producing visible
 * text. Primary purpose: give the user a heartbeat (elapsed seconds +
 * animated sparkle) so the chat never looks frozen.
 *
 * Hidden when:
 *   - not streaming, OR
 *   - the model has started emitting visible text (the bubble itself
 *     signals progress in that case).
 */
/**
 * What's the agent CLI most plausibly doing right now? Research (TTFT blog
 * posts, 2025-2026) says a status line outperforms silence — "warming the
 * cursor agent…" frames a 15-second pause as forward progress instead of
 * a hang. Tunes copy to the chosen provider; falls back to a generic line.
 */
function statusLine(provider: string | null | undefined, elapsedSec: number): string {
  const p = (provider ?? "").toLowerCase()
  if (elapsedSec < 4) return "thinking"
  if (p === "cursor") {
    return elapsedSec < 12 ? "warming cursor-agent" : "still warming cursor-agent (cold starts take 15–20s)"
  }
  if (p === "codex") {
    return elapsedSec < 10 ? "codex is reasoning" : "codex is still reasoning"
  }
  if (p === "claude") {
    return elapsedSec < 10 ? "claude is composing" : "claude is still composing"
  }
  return elapsedSec < 12 ? "thinking" : "still thinking"
}

export function ThinkingIndicator({
  streaming,
  startedAt,
  thinkingText,
  draftText,
  provider,
}: Props) {
  // Drive a 1-Hz re-render so the elapsed counter advances. Cleaner than
  // useReducer or stateful refs for a value that just needs to tick.
  const [, force] = useState(0)
  useEffect(() => {
    if (!streaming) return
    const id = setInterval(() => force((n) => n + 1), 1000)
    return () => clearInterval(id)
  }, [streaming])

  const [open, setOpen] = useState(false)
  useEffect(() => {
    // Close the trace whenever a new turn starts, so we don't accidentally
    // show the previous turn's open-state.
    if (!streaming) setOpen(false)
  }, [streaming])

  if (!streaming) return null
  if (draftText.length > 0) return null // real answer is flowing — step aside

  const elapsed = startedAt ? Math.floor((Date.now() - startedAt) / 1000) : 0
  const hasTrace = thinkingText.trim().length > 0

  return (
    <div className="inline-flex flex-col gap-1 items-start max-w-full">
      <button
        type="button"
        onClick={() => hasTrace && setOpen((v) => !v)}
        disabled={!hasTrace}
        className={`inline-flex items-center gap-2 px-2.5 py-1 rounded-full border border-line bg-bg-panel text-[11px] ${
          hasTrace ? "hover:bg-bg-hover cursor-pointer" : "cursor-default"
        }`}
        title={hasTrace ? "click to see what the model is mulling" : undefined}
      >
        <Sparkles
          size={11}
          className="text-purple-300 animate-pulse"
        />
        <span className="text-ink-dim">
          {statusLine(provider, elapsed)}
          <ThinkingDots />
        </span>
        {elapsed > 0 && (
          <span className="text-ink-faint font-mono">{elapsed}s</span>
        )}
        {hasTrace &&
          (open ? (
            <ChevronDown size={10} className="text-ink-faint" />
          ) : (
            <ChevronRight size={10} className="text-ink-faint" />
          ))}
      </button>

      {open && hasTrace && (
        <pre className="mt-1 max-w-2xl text-[11px] leading-relaxed bg-bg-inset border border-line rounded-lg px-3 py-2 whitespace-pre-wrap break-words text-ink-faint">
          {thinkingText}
        </pre>
      )}
    </div>
  )
}

/** Three dots that fade in/out one at a time. CSS-only via Tailwind animate-pulse staggered with utility classes. */
function ThinkingDots() {
  return (
    <span className="inline-flex w-4">
      <span className="animate-pulse [animation-delay:0ms]">.</span>
      <span className="animate-pulse [animation-delay:200ms]">.</span>
      <span className="animate-pulse [animation-delay:400ms]">.</span>
    </span>
  )
}
