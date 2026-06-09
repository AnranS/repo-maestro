// F-UI-001 Step 1b: the right-column task inspector. Re-homes the per-task
// surfaces (F-116 context, F-110 findings, trajectory/events, diff/artifacts,
// logs) as tabs over the EXISTING data sources — no new data logic. Overview
// carries at most one primary action. Long refs truncate to a sanitized tooltip
// (never a raw absolute path / prompt / body / secret).

import { useEffect, useRef, useState } from "react"
import type {
  RunEventGapWire,
  RunEventWire,
  TaskContextManifest,
  TaskDetail,
  TaskDiff,
  TaskState,
} from "../types"
import { api, eventConsumerId, runEventStreamUrl } from "../api"
import { StatusChip } from "./ui/StatusChip"
import { EmptyState, ErrorState, LoadingState } from "./ui/StatePane"
import { OmittedChip, TruncatedChip } from "./ui/Chips"

type TabKey = "overview" | "context" | "findings" | "events" | "artifacts" | "logs"
const TABS: { key: TabKey; label: string }[] = [
  { key: "overview", label: "Overview" },
  { key: "context", label: "Context" },
  { key: "findings", label: "Findings" },
  { key: "events", label: "Events" },
  { key: "artifacts", label: "Artifacts" },
  { key: "logs", label: "Logs" },
]

const fmtKB = (b: number) => `${(b / 1024).toFixed(1)} KB`
const fmtTok = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`)
function sevStatus(sev: string): string {
  if (sev === "critical" || sev === "high") return "failed"
  if (sev === "medium") return "risk_high"
  return "pending"
}
function duration(t: TaskState): string {
  if (!t.started_at) return ""
  const start = new Date(t.started_at).getTime()
  const end = t.ended_at ? new Date(t.ended_at).getTime() : Date.now()
  const ms = end - start
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

// F-UI-003 Slice 3: the inspector tabs speak the shared state language —
// LoadingState / EmptyState / ErrorState — so loading / empty / error look and
// read the same across every tab (empty is calm, an error is a soft neutral
// "unavailable", never a raw path/body).
const Loading = LoadingState
const Empty = EmptyState
// `title` lets a plain-text value stay inspectable on hover once it truncates
// (the value span is `truncate`); rows that render their own inner element with
// a title can omit it.
const Row = ({
  k,
  title,
  children,
}: {
  k: string
  title?: string
  children: React.ReactNode
}) => (
  <div className="flex items-center gap-2">
    <span className="w-20 shrink-0 text-ink-faint">{k}</span>
    <span className="min-w-0 flex-1 truncate text-ink-dim" title={title}>
      {children}
    </span>
  </div>
)

export function TaskInspector({ task, runId }: { task: TaskState; runId: string }) {
  const [tab, setTab] = useState<TabKey>("overview")
  const [detail, setDetail] = useState<TaskDetail | "error" | undefined>(undefined)
  // Reset to Overview whenever the selected task changes.
  useEffect(() => setTab("overview"), [task.id])
  useEffect(() => {
    let cancelled = false
    setDetail(undefined)
    api
      .taskDetail(runId, task.id)
      .then((r) => !cancelled && setDetail(r))
      .catch(() => !cancelled && setDetail("error"))
    return () => {
      cancelled = true
    }
  }, [runId, task.id])

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-2 border-b border-line px-3 py-2">
        <span className="truncate font-mono text-sm text-ink">{task.id}</span>
        <span className="text-xs text-ink-mute">{task.project}</span>
        <span className="text-xs text-ink-faint">{task.agent}</span>
        <span className="ml-auto">
          <StatusChip status={task.status} />
        </span>
      </div>
      <div className="flex items-center gap-1 overflow-x-auto border-b border-line px-2 py-1.5 scrollbar-thin">
        {TABS.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`rounded px-2 py-1 text-[11px] ${
              tab === t.key
                ? "bg-bg-hover text-ink"
                : "text-ink-faint hover:text-ink-dim"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-3 text-[11px]">
        {tab === "overview" && (
          <OverviewPane task={task} runId={runId} detail={detail} />
        )}
        {tab === "context" && <ContextPane task={task} runId={runId} />}
        {tab === "findings" && <FindingsPane detail={detail} />}
        {tab === "events" && <EventsPane task={task} runId={runId} />}
        {tab === "artifacts" && (
          <ArtifactsPane task={task} runId={runId} detail={detail} />
        )}
        {tab === "logs" && <LogsPane task={task} runId={runId} />}
      </div>
    </div>
  )
}

function OverviewPane({
  task,
  runId,
  detail,
}: {
  task: TaskState
  runId: string
  detail: TaskDetail | "error" | undefined
}) {
  const [busy, setBusy] = useState(false)
  const dur = duration(task)
  const projected = detail && detail !== "error" ? detail : null
  return (
    <div className="space-y-2">
      <div className="space-y-1">
        <Row k="status">
          <StatusChip status={task.status} />
        </Row>
        <Row k="agent" title={task.agent}>
          {task.agent}
        </Row>
        {task.role && (
          <Row k="role" title={task.role}>
            {task.role}
          </Row>
        )}
        {task.resolved_agent_profile && (
          <Row k="profile" title={task.resolved_agent_profile}>
            {task.resolved_agent_profile}
          </Row>
        )}
        {dur && (
          <Row k="duration">
            <span className="font-mono tabular-nums">{dur}</span>
          </Row>
        )}
        {task.risk_level && (
          <Row k="risk">
            <StatusChip
              status={task.risk_level === "high" ? "risk_high" : "pending"}
              label={task.risk_level}
            />
          </Row>
        )}
        {typeof task.steps === "number" && (
          <Row k="steps">
            <span className="font-mono tabular-nums">{task.steps}</span>
          </Row>
        )}
        {projected && (
          <>
            <Row k="depends">
              <span
                className="font-mono"
                title={projected.depends_on.join(", ")}
              >
                {projected.depends_on.length ? projected.depends_on.join(", ") : "none"}
              </span>
            </Row>
            {projected.downstream.length > 0 && (
              <Row k="downstream">
                <span className="font-mono" title={projected.downstream.join(", ")}>
                  {projected.downstream.join(", ")}
                </span>
              </Row>
            )}
            <Row k="artifacts">
              <span className="font-mono tabular-nums">{projected.artifacts.length}</span>
            </Row>
            {/* F-125: read-only tool-policy projection. */}
            <Row k="policy">
              <span className="font-mono text-[10px]">
                {projected.tool_policy.status}
                {projected.tool_policy.capabilities.some((c) => c.requested) &&
                  ` · ${projected.tool_policy.capabilities
                    .filter((c) => c.requested)
                    .map((c) => `${c.name}:${c.enforcement}`)
                    .join(" ")}`}
              </span>
            </Row>
            {(projected.tool_policy.declared_effects.length > 0 ||
              projected.tool_policy.observed_effects.length > 0) && (
              <Row
                k="effects"
                title={[
                  ...projected.tool_policy.declared_effects,
                  ...projected.tool_policy.observed_effects,
                ].join(", ")}
              >
                <span className="text-[10px]">
                  {[
                    ...projected.tool_policy.declared_effects,
                    ...projected.tool_policy.observed_effects,
                  ].join(", ")}
                </span>
              </Row>
            )}
            {projected.tool_policy.audit_gaps.length > 0 && (
              <Row k="audit" title={projected.tool_policy.audit_gaps.join("; ")}>
                <span className="text-[10px] text-status-warning/80">
                  {projected.tool_policy.audit_gaps.join("; ")}
                </span>
              </Row>
            )}
          </>
        )}
        {detail === "error" && (
          <Row k="detail">
            <span className="text-status-warning/80">projection unavailable</span>
          </Row>
        )}
      </div>
      {/* at most one primary action */}
      {task.status === "awaiting_approval" && (
        <button
          disabled={busy}
          onClick={async () => {
            setBusy(true)
            try {
              await api.runApprove(runId, task.id, "approve")
            } finally {
              setBusy(false)
            }
          }}
          className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-1.5 text-[12px] text-status-success hover:bg-emerald-500/15 disabled:opacity-50"
        >
          Approve
        </button>
      )}
    </div>
  )
}

function ContextPane({ task, runId }: { task: TaskState; runId: string }) {
  const [m, setM] = useState<TaskContextManifest | null | undefined>(undefined)
  const [err, setErr] = useState(false)
  useEffect(() => {
    if (task.kind !== "agent") return
    let cancelled = false
    setM(undefined)
    setErr(false)
    api
      .taskContext(runId, task.id)
      .then((r) => !cancelled && setM(r))
      .catch(() => !cancelled && setErr(true))
    return () => {
      cancelled = true
    }
  }, [runId, task.id, task.kind])

  if (task.kind !== "agent")
    return <Empty>context manifest is recorded for agent tasks only</Empty>
  if (err) return <ErrorState label="context manifest unavailable" />
  if (m === undefined) return <Loading />
  if (m === null) return <Empty>no context manifest</Empty>
  return (
    <div>
      <div className="mb-1 flex flex-wrap items-center gap-x-2 text-ink-mute">
        <span>{m.layers.length} layers</span>
        <span className="font-mono text-ink-faint">
          ~{fmtTok(m.estimated_input_tokens)} tok
        </span>
        <span className="font-mono text-ink-faint">
          {fmtKB(m.total_context_bytes)}
        </span>
      </div>
      <ol className="space-y-0.5 rounded bg-bg-inset p-2 text-[10px] leading-relaxed">
        {m.layers.map((l) => (
          <li
            key={l.order}
            className={`flex flex-wrap items-center gap-x-2 ${
              l.omitted ? "opacity-50" : ""
            }`}
          >
            <span className="shrink-0 font-mono text-ink-dim">{l.order}</span>
            <span className="shrink-0 font-mono text-ink-mute">{l.kind}</span>
            <span className="text-ink-faint">{l.label}</span>
            {!l.omitted && (
              <span className="font-mono text-ink-faint">
                {l.item_count}× · {fmtKB(l.content_bytes)} · ~
                {fmtTok(l.estimated_tokens)}t
              </span>
            )}
            {l.truncated && <TruncatedChip />}
            {l.omitted && <OmittedChip reason={l.omitted_reason ?? undefined} />}
            {(l.refs ?? []).slice(0, 6).map((r, i) => (
              <span
                key={i}
                title={`${r.kind}:${r.ref}`}
                className="max-w-[12rem] truncate rounded border border-line bg-bg-panel px-1 font-mono text-ink-faint"
              >
                {r.kind}:{r.ref}
              </span>
            ))}
            {(l.refs ?? []).length > 6 && (
              <span
                title={(l.refs ?? [])
                  .slice(6)
                  .map((r) => `${r.kind}:${r.ref}`)
                  .join("\n")}
                className="rounded border border-line bg-bg-panel px-1 text-ink-dim"
              >
                +{(l.refs ?? []).length - 6}
              </span>
            )}
          </li>
        ))}
      </ol>
    </div>
  )
}

function FindingsPane({ detail }: { detail: TaskDetail | "error" | undefined }) {
  // F-112 projects task-scoped findings on the backend. A projection read error
  // must NOT collapse to "no findings" (audit discipline).
  if (detail === undefined) return <Loading />
  if (detail === "error") return <ErrorState label="task detail unavailable" />
  const f = detail.findings
  if (f.length === 0) return <Empty>no findings for this task</Empty>
  return (
    <ul className="space-y-1.5">
      {f.map((x) => (
        <li key={x.finding_id} className="flex items-start gap-2">
          <StatusChip status={sevStatus(x.severity)} label={x.severity} />
          <span className="shrink-0 font-mono text-ink-mute">{x.kind}</span>
          <span className="flex-1 text-ink-dim">{x.summary}</span>
        </li>
      ))}
    </ul>
  )
}

type StreamRow =
  | { kind: "event"; ev: RunEventWire }
  | { kind: "gap"; gap: RunEventGapWire }

// map an event lifecycle status to a StatusChip token (succeeded → the green
// "done" token); unknown values fall back gracefully inside StatusChip.
function eventStatusToken(status: string): string {
  return status === "succeeded" ? "done" : status
}

// F-120: the live typed run-event stream in balanced mode. Run-scoped data (the
// stream + ack are per-run/per-consumer); rows for the inspected task are
// highlighted, the rest dimmed. A balanced `run_event_gap` renders as a compact
// "activity collapsed" row, NOT an error. The browser consumer's high-water is
// ack'd only after a frame is in local state, and never on unmount.
function EventsPane({ task, runId }: { task: TaskState; runId: string }) {
  const [rows, setRows] = useState<StreamRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const highWaterRef = useRef(0)
  const ackTimerRef = useRef<number | null>(null)

  useEffect(() => {
    setRows([])
    setError(null)
    highWaterRef.current = 0
    const consumerId = eventConsumerId()
    const es = new EventSource(runEventStreamUrl(runId, consumerId))

    // Ack is debounced and sent ONLY after the frame is already in state, carrying
    // the running high-water (event seq or gap to_seq). On unmount the pending timer
    // is cleared without flushing, so unprocessed frames are never ack'd.
    const scheduleAck = () => {
      if (ackTimerRef.current !== null) return
      ackTimerRef.current = window.setTimeout(() => {
        ackTimerRef.current = null
        const hw = highWaterRef.current
        if (hw > 0) void api.ackEvents(runId, consumerId, hw).catch(() => {})
      }, 400)
    }

    es.addEventListener("run_event", (e: MessageEvent) => {
      let ev: RunEventWire
      try {
        ev = JSON.parse(e.data)
      } catch {
        return
      }
      setError(null)
      setRows((prev) => [...prev, { kind: "event" as const, ev }].slice(-500))
      highWaterRef.current = Math.max(highWaterRef.current, ev.seq)
      scheduleAck()
    })
    es.addEventListener("run_event_gap", (e: MessageEvent) => {
      let gap: RunEventGapWire
      try {
        gap = JSON.parse(e.data)
      } catch {
        return
      }
      setError(null)
      setRows((prev) => [...prev, { kind: "gap" as const, gap }].slice(-500))
      highWaterRef.current = Math.max(highWaterRef.current, gap.to_seq)
      scheduleAck()
    })
    es.addEventListener("error", (e: Event) => {
      // both a server-emitted neutral `error` frame (corrupt ledger) and a transport
      // drop dispatch here. Show a neutral notice; never fake "caught up". EventSource
      // retries on its own.
      const data = (e as MessageEvent).data
      setError(typeof data === "string" && data ? data : "event stream interrupted")
    })

    return () => {
      es.close()
      if (ackTimerRef.current !== null) {
        window.clearTimeout(ackTimerRef.current)
        ackTimerRef.current = null
      }
    }
  }, [runId])

  if (rows.length === 0 && !error) return <Empty>no events yet</Empty>
  return (
    <div className="space-y-1">
      {error && (
        <div className="rounded border border-amber-500/20 bg-amber-500/10 px-2 py-1 text-[10px] text-status-warning/80">
          {error}
        </div>
      )}
      <ol className="space-y-0.5 rounded bg-bg-inset p-2 text-[10px] leading-relaxed">
        {rows.map((row) =>
          row.kind === "gap" ? (
            <li
              key={`g-${row.gap.from_seq}-${row.gap.to_seq}`}
              className="flex items-center gap-2 text-ink-faint"
            >
              <span className="shrink-0 font-mono text-ink-dim">
                {row.gap.from_seq}–{row.gap.to_seq}
              </span>
              <span className="italic">⋯ {row.gap.count} activity events collapsed</span>
            </li>
          ) : (
            <li
              key={`e-${row.ev.seq}`}
              className={`flex items-center gap-2 ${
                row.ev.task_id && row.ev.task_id !== task.id ? "opacity-60" : ""
              }`}
            >
              <span className="shrink-0 font-mono text-ink-dim">{row.ev.seq}</span>
              {row.ev.status && (
                <StatusChip status={eventStatusToken(row.ev.status)} label={row.ev.status} />
              )}
              <span className="truncate text-ink-dim">{row.ev.display?.label ?? row.ev.kind}</span>
              {row.ev.message && (
                <span className="truncate text-ink-faint">{row.ev.message}</span>
              )}
            </li>
          ),
        )}
      </ol>
    </div>
  )
}

function ArtifactsPane({
  task,
  runId,
  detail,
}: {
  task: TaskState
  runId: string
  detail: TaskDetail | "error" | undefined
}) {
  // A diff read error must NOT collapse to "no changed files" (audit discipline):
  // only a successful, empty result is "no changed files".
  const [d, setD] = useState<TaskDiff | "error" | undefined>(undefined)
  useEffect(() => {
    let cancelled = false
    setD(undefined)
    api
      .taskDiff(runId, task.id)
      .then((r) => !cancelled && setD(r))
      .catch(() => !cancelled && setD("error"))
    return () => {
      cancelled = true
    }
  }, [runId, task.id])
  if (d === undefined) return <Loading />
  if (d === "error") return <ErrorState label="changes unavailable" />
  // A task-detail projection error must surface explicitly (F-110/F-112 audit
  // discipline) — never silently render as "no artifact refs". The diff (`d`)
  // keeps its own independent state above.
  const artifactsErrored = detail === "error"
  const artifacts = detail && detail !== "error" ? detail.artifacts : []
  if (d.files.length === 0 && artifacts.length === 0 && !artifactsErrored)
    return <Empty>no changed files</Empty>
  return (
    <div>
      {artifactsErrored ? (
        <div className="mb-2 rounded bg-bg-inset p-2 text-[10px] text-status-warning/80">
          artifact refs unavailable
        </div>
      ) : (
        artifacts.length > 0 && (
          <ul className="mb-2 space-y-0.5 rounded bg-bg-inset p-2 font-mono text-[10px]">
            {artifacts.map((artifact, i) => (
              <li
                key={`${artifact.kind}-${artifact.path ?? i}`}
                className="flex items-center gap-2"
              >
                <span className="w-20 shrink-0 text-ink-mute">{artifact.kind}</span>
                <span className="truncate text-ink-faint" title={artifact.path ?? "endpoint"}>
                  {artifact.path ?? "endpoint"}
                </span>
              </li>
            ))}
          </ul>
        )
      )}
      {d.risk && (
        <div className="mb-1">
          <StatusChip
            status={d.risk.level === "high" ? "risk_high" : "pending"}
            label={`risk ${d.risk.level}`}
          />
        </div>
      )}
      {d.files.length > 0 ? (
        <ul className="space-y-0.5 font-mono text-[10px]">
          {d.files.map((file) => (
            <li key={file.path} className="flex items-center gap-2">
              <span className="w-4 shrink-0 text-ink-mute">{file.status}</span>
              <span className="truncate text-ink-dim" title={file.path}>
                {file.path}
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <Empty>no changed files</Empty>
      )}
    </div>
  )
}

function LogsPane({ task, runId }: { task: TaskState; runId: string }) {
  const [log, setLog] = useState("")
  const esRef = useRef<EventSource | null>(null)
  useEffect(() => {
    setLog("")
    const url = `/api/logs/${encodeURIComponent(runId)}/${encodeURIComponent(
      task.id,
    )}/stream`
    const es = new EventSource(url)
    es.addEventListener("log", (e: MessageEvent) => setLog(e.data))
    es.addEventListener("delta", (e: MessageEvent) =>
      setLog((prev) => prev + e.data),
    )
    esRef.current = es
    return () => es.close()
  }, [runId, task.id])
  if (!log) return <Empty>no log output yet</Empty>
  return (
    <pre className="whitespace-pre-wrap break-words rounded bg-bg-inset p-2 font-mono text-[10px] leading-relaxed text-ink-dim">
      {log}
    </pre>
  )
}
