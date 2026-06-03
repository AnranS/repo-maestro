import { memo, useEffect, useMemo, useState } from "react"
import {
  Background,
  BackgroundVariant,
  Controls,
  Edge,
  Handle,
  MarkerType,
  Node,
  NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
} from "@xyflow/react"
import dagre from "@dagrejs/dagre"
import { Loader2, Pause, Sparkles } from "lucide-react"
import type { TaskState } from "../types"
import { GRAPH_CANVAS, taskStatusColor, withAlpha } from "./graph/tokens"
import "@xyflow/react/dist/style.css"

const NODE_W = 210
const NODE_H = 64

function layout(nodes: Node[], edges: Edge[]) {
  const g = new dagre.graphlib.Graph()
  g.setDefaultEdgeLabel(() => ({}))
  g.setGraph({
    rankdir: "LR",
    nodesep: 32,
    ranksep: 90,
    marginx: 16,
    marginy: 16,
  })
  nodes.forEach((n) => g.setNode(n.id, { width: NODE_W, height: NODE_H }))
  edges.forEach((e) => g.setEdge(e.source, e.target))
  dagre.layout(g)

  // Snap only pure chains. Fan-out / fan-in layouts must keep dagre's Y,
  // otherwise sibling nodes with the same single upstream collapse together.
  const incoming = new Map<string, number>()
  const outgoing = new Map<string, number>()
  edges.forEach((e) => {
    incoming.set(e.target, (incoming.get(e.target) ?? 0) + 1)
    outgoing.set(e.source, (outgoing.get(e.source) ?? 0) + 1)
  })
  const ySnap = new Map<string, number>()
  nodes.forEach((n) => ySnap.set(n.id, g.node(n.id).y))
  for (const e of edges) {
    if ((incoming.get(e.target) ?? 0) === 1 && (outgoing.get(e.source) ?? 0) === 1) {
      const srcY = ySnap.get(e.source)
      if (srcY !== undefined) ySnap.set(e.target, srcY)
    }
  }

  return nodes.map((n) => {
    const dn = g.node(n.id)
    const y = ySnap.get(n.id) ?? dn.y
    return { ...n, position: { x: dn.x - NODE_W / 2, y: y - NODE_H / 2 } }
  })
}

interface TaskNodeData extends Record<string, unknown> {
  task: TaskState
  /** Current clock (ms), injected by DagView so running timers tick live. */
  now?: number
}

/** Elapsed wall-clock for a task: final once ended, live while running. */
function fmtDuration(t: TaskState, now: number): string {
  if (!t.started_at) return ""
  const start = new Date(t.started_at).getTime()
  const end = t.ended_at ? new Date(t.ended_at).getTime() : now
  const ms = Math.max(0, end - start)
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

function nodeStyle(status: string) {
  const color = taskStatusColor(status)
  const active = status === "running" || status === "failed" || status === "awaiting_approval"
  const running = status === "running"
  return {
    // Running nodes get a brighter fill + border so the active frontier reads
    // as "lit up" against the dimmed pending nodes (the pulse glow is layered
    // on via the .task-pulse keyframe).
    background: `linear-gradient(0deg, ${withAlpha(color, running ? 0.16 : 0.08)}, ${withAlpha(color, running ? 0.16 : 0.08)}), #161616`,
    borderColor: withAlpha(color, running ? 0.85 : 0.45),
    boxShadow: active && !running ? `0 0 18px ${withAlpha(color, 0.22)}` : undefined,
  }
}

function nodeChrome(status: string): string {
  switch (status) {
    case "running":
    case "failed":
    case "awaiting_approval":
      return "ring-1"
    default:
      return "ring-1 ring-transparent"
  }
}

const TaskNode = memo(function TaskNode({ data }: NodeProps<Node<TaskNodeData>>) {
  const t = data.task
  const running = t.status === "running"
  // Pipeline reading: spotlight the running frontier, fade the not-yet-reached
  // so the eye lands on "where execution is right now".
  const dim = t.status === "pending" || t.status === "skipped" || t.status === "cancelled"
  const pulse = running ? "task-pulse" : ""
  const color = taskStatusColor(t.status)
  const elapsed = fmtDuration(t, data.now ?? Date.now())
  return (
    <div
      style={nodeStyle(t.status)}
      className={`group h-[64px] w-[210px] rounded-xl border px-3 py-2.5 transition-all hover:border-line-soft ${nodeChrome(
        t.status,
      )} ${pulse} ${dim ? "opacity-50" : ""}`}
    >
      <Handle type="target" position={Position.Left} className="!w-1 !h-1 !bg-line-soft !border-0" />

      <div className="mb-1 flex items-center gap-2">
        {running ? (
          <Loader2 size={13} className="shrink-0 animate-spin" style={{ color }} />
        ) : (
          <span
            className="h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: color }}
          />
        )}
        <span className="min-w-0 truncate font-mono text-[12px] font-medium text-ink" title={t.id}>
          {t.id}
        </span>
        {running && (
          <span className="ml-auto shrink-0 text-[9px] font-semibold uppercase tracking-wider text-blue-300">
            running
          </span>
        )}
      </div>

      <div className="flex items-center gap-2 text-[10px] text-ink-mute">
        <span className="truncate font-medium" title={t.project}>{t.project}</span>
        <span className="text-ink-faint">·</span>
        <span className="text-ink-faint">{t.agent}</span>
        <span className="ml-auto flex items-center gap-1.5">
          {!!t.attempts && t.attempts > 0 && (
            <span
              className="rounded bg-amber-500/15 px-1 font-mono text-[9px] text-amber-300"
              title={`retried ${t.attempts}×`}
            >
              ↻{t.attempts}
            </span>
          )}
          {elapsed && (
            <span className={`font-mono tabular-nums ${running ? "text-blue-300" : "text-ink-faint"}`}>
              {elapsed}
            </span>
          )}
          {t.kind === "verify" && (
            <span title="verify task" className="text-purple-300/90">
              <Sparkles size={10} />
            </span>
          )}
          {t.requires_approval_after && (
            <span title="requires approval after" className="text-amber-300/90">
              <Pause size={10} />
            </span>
          )}
        </span>
      </div>

      <Handle type="source" position={Position.Right} className="!w-1 !h-1 !bg-line-soft !border-0" />
    </div>
  )
})

const nodeTypes = { task: TaskNode }

export function DagView({ tasks }: { tasks: TaskState[] }) {
  const { laidOut, edges } = useMemo(() => {
    const baseNodes: Node[] = tasks.map((t) => ({
      id: t.id,
      type: "task",
      position: { x: 0, y: 0 },
      data: { task: t },
      draggable: false,
      selectable: false,
    }))
    const byId = new Map(tasks.map((t) => [t.id, t]))
    const baseEdges: Edge[] = []
    for (const t of tasks) {
      for (const dep of t.depends_on || []) {
        const depTask = byId.get(dep)
        const stroke = edgeColor(t.status, depTask?.status)
        baseEdges.push({
          id: `${dep}->${t.id}`,
          source: dep,
          target: t.id,
          // Orthogonal routing, consistent with the architecture and code-graph
          // views — clean elbows read as deliberate wiring instead of arcs that
          // drift across the canvas.
          type: "smoothstep",
          // Every edge that is part of the active execution flow gets
          // animated dashes so the user can read direction at a glance. We
          // include `done` so completed paths still feel like a finished
          // pipeline rather than a static diagram.
          animated: shouldFlow(depTask?.status, t.status),
          style: {
            stroke,
            strokeWidth: t.status === "running" ? 2 : 1.6,
          },
          markerEnd: {
            type: MarkerType.ArrowClosed,
            color: stroke,
            width: 16,
            height: 16,
          },
        })
      }
    }
    return { laidOut: layout(baseNodes, baseEdges), edges: baseEdges }
  }, [tasks])

  // Tick once a second while anything is running so the elapsed timers on the
  // running nodes count up live (like a pipeline stage clock). Stops when no
  // task is running, so a finished/idle graph doesn't re-render needlessly.
  const hasRunning = useMemo(() => tasks.some((t) => t.status === "running"), [tasks])
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!hasRunning) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [hasRunning])

  // Inject the current clock into node data without re-running dagre layout, so
  // the running timers tick without the graph re-laying-out every second. Only
  // the running nodes actually display `now`, so we leave every other node's
  // reference untouched — that lets React.memo skip re-rendering them and keeps
  // the per-second tick from re-rendering the whole graph.
  const nodes = useMemo(
    () =>
      laidOut.map((n) =>
        (n.data as TaskNodeData)?.task?.status === "running"
          ? { ...n, data: { ...n.data, now } }
          : n,
      ),
    [laidOut, now],
  )

  // Size the canvas to the graph's vertical extent instead of a fixed 420px:
  // a snug box for a small chain, more room for wide fan-outs, capped so a
  // large DAG stays pannable rather than pushing the page absurdly tall.
  const height = useMemo(() => {
    if (laidOut.length === 0) return 320
    const ys = laidOut.map((n) => n.position.y)
    const span = Math.max(...ys) + NODE_H - Math.min(...ys)
    return Math.round(Math.min(Math.max(span + 96, 280), 680))
  }, [laidOut])

  if (tasks.length === 0) {
    return null
  }

  return (
    <ReactFlowProvider>
      <div
        style={{ height }}
        className="w-full rounded-xl bg-bg-inset border border-line/60 dag-flow"
      >
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          fitView
          fitViewOptions={GRAPH_CANVAS.fitView}
          minZoom={0.4}
          maxZoom={1.4}
          proOptions={{ hideAttribution: true }}
          nodesDraggable={false}
          nodesConnectable={false}
          panOnScroll
          zoomOnPinch
          zoomOnDoubleClick={false}
        >
          <Background
            variant={BackgroundVariant.Dots}
            gap={20}
            size={1}
            color={GRAPH_CANVAS.grid}
          />
          <Controls
            showInteractive={false}
            position="bottom-right"
            className="!bg-bg-panel !border-line !rounded !overflow-hidden"
          />
        </ReactFlow>
      </div>
    </ReactFlowProvider>
  )
}

function edgeColor(targetStatus: string, sourceStatus?: string) {
  // The edge takes the downstream (target) task's color so the user reads it
  // as "flowing into" the next task. If the target is still pending but the
  // source is already done, we tint the line emerald to convey the upstream
  // work has completed and the next step is ready.
  switch (targetStatus) {
    case "running":
      return taskStatusColor("running")
    case "done":
      return taskStatusColor("done")
    case "failed":
      return taskStatusColor("failed")
    case "awaiting_approval":
      return taskStatusColor("awaiting_approval")
    case "pending":
      return sourceStatus === "done" ? taskStatusColor("done") : taskStatusColor("pending")
    default:
      return taskStatusColor("pending")
  }
}

/**
 * Animate the edge whenever real work is moving through it. We treat `done`
 * as flowing too so a finished pipeline still feels like a working circuit
 * (the colour is what conveys the "this is in the past" status); only
 * `pending` (no upstream done yet), `skipped`, `failed`, and `cancelled`
 * stay static.
 */
function shouldFlow(sourceStatus: string | undefined, targetStatus: string): boolean {
  switch (targetStatus) {
    case "running":
    case "awaiting_approval":
    case "done":
      return true
    case "pending":
      // "Primed" edges (upstream done, downstream ready) get flow too so the
      // user's eye is pulled to the next step.
      return sourceStatus === "done"
    default:
      return false
  }
}
