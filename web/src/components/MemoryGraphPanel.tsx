import { useEffect, useMemo, useState } from "react"
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
import "@xyflow/react/dist/style.css"
import { Boxes, GitCommitHorizontal } from "lucide-react"
import type { MemoryGraph } from "../types"
import { api } from "../api"
import { t } from "../i18n"
import { GRAPH_CANVAS } from "./graph/tokens"

const NODE_W = 188
const NODE_H = 48

interface ProjectData extends Record<string, unknown> {
  label: string
  weight: number
}
interface RunData extends Record<string, unknown> {
  label: string
  status?: string | null
  runId?: string | null
}

function ProjectNode({ data }: NodeProps<Node<ProjectData>>) {
  return (
    <div
      style={{ width: NODE_W, height: NODE_H }}
      className="flex flex-col justify-center rounded-lg border border-blue-500/40 bg-blue-500/10 px-3 hover:border-blue-400"
    >
      <Handle type="target" position={Position.Left} className="!w-1 !h-1 !bg-line-soft !border-0" />
      <div className="flex items-center gap-1.5">
        <Boxes size={12} className="shrink-0 text-accent" />
        <span className="truncate text-[12px] font-medium text-ink" title={data.label}>
          {data.label}
        </span>
      </div>
      <div className="mt-0.5 pl-[18px] text-[9px] text-ink-faint">
        {data.weight} {t("memory.graph.decisions")}
      </div>
      <Handle type="source" position={Position.Right} className="!w-1 !h-1 !bg-line-soft !border-0" />
    </div>
  )
}

function RunNode({ data }: NodeProps<Node<RunData>>) {
  const done = (data.status ?? "").toLowerCase() === "done"
  return (
    <div
      style={{ width: NODE_W, height: NODE_H }}
      className="flex flex-col justify-center rounded-full border border-amber-500/40 bg-amber-500/10 px-3 hover:border-amber-400"
      title={data.runId ?? undefined}
    >
      <Handle type="target" position={Position.Left} className="!w-1 !h-1 !bg-line-soft !border-0" />
      <div className="flex items-center gap-1.5">
        <GitCommitHorizontal size={12} className="shrink-0 text-status-warning" />
        <span className="truncate text-[11px] text-ink" title={data.label}>
          {data.label}
        </span>
      </div>
      <div className="mt-0.5 flex items-center gap-1 pl-[18px] text-[9px] text-ink-faint">
        <span className={`h-1.5 w-1.5 rounded-full ${done ? "bg-emerald-400" : "bg-ink-faint"}`} />
        {data.status ?? "run"}
      </div>
      <Handle type="source" position={Position.Right} className="!w-1 !h-1 !bg-line-soft !border-0" />
    </div>
  )
}

const nodeTypes = { project: ProjectNode, run: RunNode }

function layout(nodes: Node[], edges: Edge[]) {
  const g = new dagre.graphlib.Graph()
  g.setDefaultEdgeLabel(() => ({}))
  g.setGraph({ rankdir: "LR", nodesep: 22, ranksep: 130, marginx: 24, marginy: 24 })
  nodes.forEach((n) => g.setNode(n.id, { width: NODE_W, height: NODE_H }))
  edges.forEach((e) => g.setEdge(e.source, e.target))
  dagre.layout(g)
  return nodes.map((n) => {
    const dn = g.node(n.id)
    return { ...n, position: { x: dn.x - NODE_W / 2, y: dn.y - NODE_H / 2 } }
  })
}

/**
 * Graph mode for the Memory tab: runs (events) stitched to the projects they
 * touched, with each project's contract dependency overlaid. Makes the
 * cross-project coupling that the scheduler reasons about visible at a glance.
 */
export function MemoryGraphPanel() {
  const [graph, setGraph] = useState<MemoryGraph | null>(null)

  useEffect(() => {
    api.memoryGraph().then(setGraph).catch(() => setGraph({ nodes: [], edges: [] }))
  }, [])

  const { nodes, edges } = useMemo(() => {
    if (!graph) return { nodes: [], edges: [] }
    const baseNodes: Node[] = graph.nodes.map((n) => ({
      id: n.id,
      type: n.kind === "run" ? "run" : "project",
      position: { x: 0, y: 0 },
      data:
        n.kind === "run"
          ? { label: n.label, status: n.status, runId: n.run_id }
          : { label: n.label, weight: n.weight },
      draggable: true,
      selectable: false,
    }))
    const baseEdges: Edge[] = graph.edges.map((e, i) => {
      const consumes = e.kind === "consumes"
      const stroke = consumes ? "#f59e0b" : "#3b82f6"
      return {
        id: `${e.source}->${e.target}-${i}`,
        source: e.source,
        target: e.target,
        type: "smoothstep",
        animated: !consumes,
        style: { stroke, strokeWidth: 1.4, strokeDasharray: consumes ? "4 3" : undefined },
        markerEnd: { type: MarkerType.ArrowClosed, color: stroke, width: 14, height: 14 },
      }
    })
    return { nodes: layout(baseNodes, baseEdges), edges: baseEdges }
  }, [graph])

  if (graph && graph.nodes.length === 0) {
    return (
      <div className="flex h-72 items-center justify-center text-center text-sm text-ink-faint">
        <p className="max-w-md leading-relaxed">{t("memory.graph.empty")}</p>
      </div>
    )
  }

  return (
    <ReactFlowProvider>
      <div className="h-[calc(100vh-220px)] min-h-[360px] rounded-lg border border-line arch-flow">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          fitView
          fitViewOptions={GRAPH_CANVAS.fitView}
          minZoom={0.2}
          maxZoom={1.6}
          proOptions={{ hideAttribution: true }}
          panOnScroll
          zoomOnDoubleClick={false}
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1} color={GRAPH_CANVAS.grid} />
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
