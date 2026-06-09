import { useEffect, useMemo, useState } from "react"
import type { TaskState } from "../../types"
import { taskStatusColor, withAlpha } from "../graph/tokens"
import { t } from "../../i18n"

const LABEL_W = 144 // px, the left gutter for task ids (keep in sync w/ w-36)
const TICKS = 4 // axis divisions → 5 labels (0 … total)

/**
 * A trace-waterfall of the run, modelled on the patterns that make build
 * pipelines and request traces legible (Chrome DevTools' network waterfall,
 * Jaeger spans, GitHub Actions' job timeline):
 *
 *   - a real time axis with gridlines, so offsets are readable;
 *   - rows sorted by start time (top-to-bottom = chronological);
 *   - each bar annotated with its start offset (`+Xs`) and duration;
 *   - the lead-in gap before a bar is drawn faintly, so idle/blocked waits
 *     (e.g. a task held by the dependency gate) are visible, not mysterious.
 *
 * Running bars grow to "now" behind a moving cursor and shimmer; the clock
 * ticks once a second only while something runs.
 */
export function RunTimeline({ tasks, embedded }: { tasks: TaskState[]; embedded?: boolean }) {
  const hasRunning = useMemo(() => tasks.some((task) => task.status === "running"), [tasks])
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!hasRunning) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [hasRunning])

  const model = useMemo(() => {
    const started = tasks.filter((task) => task.started_at)
    if (started.length === 0) return null
    const starts = started.map((task) => new Date(task.started_at!).getTime())
    const ends = started.map((task) =>
      task.ended_at ? new Date(task.ended_at).getTime() : now,
    )
    const t0 = Math.min(...starts)
    const span = Math.max(Math.max(...ends, t0 + 1000) - t0, 1000)

    // Chronological waterfall: started tasks by start time, then the rest
    // (queued) keeping their given order so the list stays stable.
    const rows = [...tasks].sort((a, b) => {
      const sa = a.started_at ? new Date(a.started_at).getTime() : Infinity
      const sb = b.started_at ? new Date(b.started_at).getTime() : Infinity
      return sa - sb
    })
    return { t0, span, rows, nowPct: ((now - t0) / span) * 100 }
  }, [tasks, now])

  if (!model) return null
  const { t0, span, rows, nowPct } = model

  // When embedded in the merged tasks panel, drop the section chrome — the
  // parent supplies the header — and just render axis + rows + a total.
  const body = (
    <>
      <div className="px-4 pt-3 pb-4">
        {embedded && (
          <div className="mb-1 text-right font-mono text-[10px] tabular-nums text-ink-faint">
            {fmtSpan(span)}
          </div>
        )}
        {/* time axis */}
        <div className="flex items-center">
          <span style={{ width: LABEL_W }} className="shrink-0" />
          <div className="relative h-4 flex-1">
            {Array.from({ length: TICKS + 1 }, (_, i) => {
              const pct = (i / TICKS) * 100
              return (
                <span
                  key={i}
                  className="absolute top-0 font-mono text-[9px] text-ink-faint tabular-nums"
                  style={{
                    left: `${pct}%`,
                    transform:
                      i === 0 ? "none" : i === TICKS ? "translateX(-100%)" : "translateX(-50%)",
                  }}
                >
                  {fmtSpan((span * i) / TICKS)}
                </span>
              )
            })}
          </div>
          <span className="w-20 shrink-0" />
        </div>

        <div className="mt-1 space-y-1.5">
          {rows.map((task) => (
            <TimelineRow key={task.id} task={task} t0={t0} span={span} now={now} nowPct={nowPct} />
          ))}
        </div>
      </div>
    </>
  )

  if (embedded) return body

  return (
    <section data-pane="timeline" className="bg-bg-panel border border-line rounded-lg">
      <div className="px-4 py-2.5 border-b border-line flex items-center justify-between">
        <span className="text-xs uppercase tracking-wider text-ink-faint">{t("tasks.timeline")}</span>
        <span className="text-xs text-ink-faint font-mono tabular-nums">{fmtSpan(span)}</span>
      </div>
      {body}
    </section>
  )
}

// Vertical gridlines at each tick, painted behind the bars.
const GRID_STYLE: React.CSSProperties = {
  backgroundImage: "linear-gradient(to right, var(--tw-grid-line, rgba(255,255,255,0.06)) 1px, transparent 1px)",
  backgroundSize: `${100 / TICKS}% 100%`,
}

function TimelineRow({
  task,
  t0,
  span,
  now,
  nowPct,
}: {
  task: TaskState
  t0: number
  span: number
  now: number
  nowPct: number
}) {
  const color = taskStatusColor(task.status)
  const running = task.status === "running"
  const started = !!task.started_at

  let leftPct = 0
  let widthPct = 0
  let offsetMs = 0
  if (started) {
    const s = new Date(task.started_at!).getTime()
    const e = task.ended_at ? new Date(task.ended_at).getTime() : now
    offsetMs = s - t0
    leftPct = (offsetMs / span) * 100
    widthPct = Math.max(((e - s) / span) * 100, 1.2)
  }

  return (
    <div className="flex items-center gap-3">
      <span
        style={{ width: LABEL_W }}
        className="shrink-0 truncate font-mono text-[11px] text-ink-dim"
        title={task.id}
      >
        {task.id}
      </span>

      <div className="relative h-5 flex-1 overflow-hidden rounded bg-bg-inset" style={GRID_STYLE}>
        {/* now cursor (only while the run is live) */}
        {running && nowPct >= 0 && nowPct <= 100 && (
          <span
            className="absolute top-0 bottom-0 w-px bg-blue-400/50"
            style={{ left: `${Math.min(nowPct, 100)}%` }}
          />
        )}
        {started ? (
          <>
            {/* faint lead-in showing the wait before this task started */}
            {leftPct > 1 && (
              <div
                className="absolute top-1/2 h-px -translate-y-1/2 bg-line-soft/60"
                style={{ left: 0, width: `${leftPct}%` }}
                title={t("tasks.waited", { d: fmtSpan(offsetMs) })}
              />
            )}
            <div
              className={`absolute top-0 h-full rounded ${running ? "timeline-bar-running" : ""}`}
              style={{
                left: `${leftPct}%`,
                width: `${widthPct}%`,
                backgroundColor: withAlpha(color, running ? 0.9 : 0.65),
                boxShadow: running ? `0 0 10px ${withAlpha(color, 0.6)}` : undefined,
              }}
            />
          </>
        ) : (
          <span className="absolute left-2 top-1/2 -translate-y-1/2 text-[10px] text-ink-faint">
            {t("tasks.queued")}
          </span>
        )}
      </div>

      <span className="w-20 shrink-0 text-right font-mono text-[10px] tabular-nums leading-tight text-ink-faint">
        {started ? (
          <>
            {offsetMs > 500 && <span className="text-ink-faint/70">+{fmtSpan(offsetMs)} · </span>}
            <span className="text-ink-dim">{fmtDur(task, now)}</span>
          </>
        ) : (
          ""
        )}
      </span>
    </div>
  )
}

function fmtDur(task: TaskState, now: number): string {
  if (!task.started_at) return ""
  const start = new Date(task.started_at).getTime()
  const end = task.ended_at ? new Date(task.ended_at).getTime() : now
  return fmtSpan(Math.max(0, end - start))
}

function fmtSpan(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}
