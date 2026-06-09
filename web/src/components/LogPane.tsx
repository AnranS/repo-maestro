import { useMemo, useRef, useEffect } from "react"

interface Props {
  text: string
  className?: string
  /** scroll to bottom whenever `text` changes (default true). */
  follow?: boolean
  /**
   * Task status used to pick the empty-state placeholder. For a running
   * task an empty `text` legitimately means "still arriving" — we say
   * `(streaming…)`. For a task that's already terminal (done / failed /
   * skipped / cancelled), empty `text` means the command genuinely
   * produced no output, and `(streaming…)` would be a lie.
   */
  taskStatus?: string
}

/**
 * Renders task log output as a series of color-coded lines. Recognised
 * patterns (handled in priority order):
 *
 *   [maestro] / [shell] prefix       — muted gray
 *   diff --git / index / @@      — gray header, cyan hunks
 *   --- a/path / +++ b/path      — muted bar
 *   +<…> (not "+++")            — green
 *   -<…> (not "---")            — red
 *   ✓ …                          — emerald
 *   ✗ …                          — red
 *   ⚠ …                          — amber
 *   [stderr]                     — amber section header
 *   blank lines                  — preserved
 */
const TERMINAL_STATUSES = new Set(["done", "failed", "skipped", "cancelled"])

export function LogPane({ text, className, follow = true, taskStatus }: Props) {
  const ref = useRef<HTMLDivElement>(null)

  const lines = useMemo(() => (text ? text.split("\n") : []), [text])
  const isTerminal = taskStatus ? TERMINAL_STATUSES.has(taskStatus) : false
  const emptyPlaceholder = isTerminal ? "(no log output)" : "(streaming…)"

  useEffect(() => {
    if (follow && ref.current) {
      ref.current.scrollTop = ref.current.scrollHeight
    }
  }, [lines, follow])

  return (
    <div
      ref={ref}
      className={
        "bg-bg-inset border border-line rounded-md p-3 font-mono text-[11.5px] leading-relaxed max-h-80 overflow-y-auto scrollbar-thin " +
        (className ?? "")
      }
    >
      {lines.length === 0 ? (
        <span className="text-ink-faint">{emptyPlaceholder}</span>
      ) : (
        <pre className="whitespace-pre-wrap break-words">
          {lines.map((raw, i) => (
            <LogLine key={i} raw={raw} />
          ))}
        </pre>
      )}
    </div>
  )
}

function LogLine({ raw }: { raw: string }) {
  const cls = classifyLine(raw)
  return (
    <div className={cls}>
      {raw || "\u200B" /* keep blank line height */}
    </div>
  )
}

function classifyLine(raw: string): string {
  const line = raw

  // maestro / shell scaffolding lines
  if (/^\[maestro\]/.test(line)) return "text-ink-faint"
  if (/^\[shell\]/.test(line)) return "text-ink-mute"
  if (/^\[stderr\]/.test(line)) return "text-amber-400 font-semibold"

  // diff hunks come first to win over +/- rules
  if (/^diff --git /.test(line)) return "text-ink-mute font-semibold"
  if (/^index [0-9a-f]/.test(line)) return "text-ink-faint"
  if (/^@@ /.test(line)) return "text-cyan-300"
  if (/^---\s+a\//.test(line) || line === "---") return "text-ink-faint"
  if (/^\+\+\+\s+b\//.test(line)) return "text-ink-faint"

  // diff content
  if (/^\+/.test(line) && !line.startsWith("+++"))
    return "text-status-success/95 bg-emerald-500/[0.06]"
  if (/^-/.test(line) && !line.startsWith("---"))
    return "text-status-danger/95 bg-red-500/[0.06]"

  // success/failure marks
  if (/^\s*✓/.test(line)) return "text-status-success"
  if (/^\s*✗/.test(line)) return "text-status-danger"
  if (/^\s*⚠/.test(line) || /WARN(ING)?/.test(line)) return "text-status-warning"
  if (/(FAILED|ERROR|error)/.test(line)) return "text-status-danger"
  if (/(PASSED|ok\b|done)/.test(line)) return "text-status-success"

  // section / heading conventions
  if (/^==[^=]/.test(line) || /^--[^-]/.test(line))
    return "text-ink/95 font-semibold"

  return "text-ink-dim"
}
