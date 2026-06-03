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
// Without this stylesheet `.react-flow__node` defaults to `position: static`
// and the per-node transforms collapse — nodes stack vertically instead of
// honouring their layout coordinates. Both DAG views must import it.
import "@xyflow/react/dist/style.css"
import {
  Box,
  ServerCog,
  Globe,
  Smartphone,
  Terminal,
  Library,
  Cpu,
  GitBranch,
  Plus,
  Trash2,
  ChevronDown,
  ArrowUpRight,
  ArrowDownLeft,
  Brain,
  Loader2,
} from "lucide-react"
import { api } from "../api"
import type {
  ArchEdgeView,
  ArchModuleView,
  ArchitectureView as ArchView,
} from "../types"
import { AddProjectModal } from "./AddProjectModal"
import { ProjectMemoryPanel } from "./ProjectMemoryPanel"
import { t, useLang } from "../i18n"
import { ARCH_EDGE_COLORS, GRAPH_CANVAS, moduleTypeColor } from "./graph/tokens"

// IMPORTANT: keep these in sync with `ModuleNode`'s real rendered size.
// dagre lays nodes out based on the dimensions we declare here; if the actual
// DOM box is bigger, two ranks end up at slightly different Y and the
// connecting edge becomes an L-shape instead of a straight line.
const NODE_W = 220
const NODE_H = 134

interface ModuleNodeData extends Record<string, unknown> {
  module: ArchModuleView
  onDelete: (name: string) => void
  decisions: number
}

function iconForType(t?: string | null) {
  const style = { color: moduleTypeColor(t) }
  switch (t) {
    case "backend":
      return <ServerCog size={14} style={style} />
    case "frontend":
      return <Globe size={14} style={style} />
    case "mobile":
      return <Smartphone size={14} style={style} />
    case "tool":
      return <Terminal size={14} style={style} />
    case "library":
      return <Library size={14} style={style} />
    default:
      return <Box size={14} style={style} />
  }
}

// Stable color per subspace, so projects in the same monorepo subspace read as
// a group even when they have no dependency edges between them.
const SUBSPACE_PALETTE = [
  "#6ea8d8", "#cf7e9c", "#5fb89a", "#c7a45e",
  "#9d8bd0", "#5aa9bd", "#cf8c63", "#7bbf7b",
]
function subspaceOf(m: { stack: string[] }): string | null {
  const s = m.stack.find((x) => x.startsWith("subspace:"))
  return s ? s.slice("subspace:".length) : null
}
function subspaceColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return SUBSPACE_PALETTE[h % SUBSPACE_PALETTE.length]
}

function ModuleNode({ data }: NodeProps<Node<ModuleNodeData>>) {
  const m = data.module
  // Group by subspace when present (monorepo); otherwise fall back to type.
  const sub = subspaceOf(m)
  const accent = sub ? subspaceColor(sub) : moduleTypeColor(m.type)
  return (
    <div
      // Locking width AND height keeps dagre's declared box and the actual DOM
      // box identical — required for clean straight edges in LR layouts.
      style={{ width: NODE_W, height: NODE_H, borderLeftColor: accent }}
      className="group rounded-xl border border-l-[3px] border-line bg-bg-panel shadow-sm px-3 py-2.5 hover:border-line-soft transition-colors relative overflow-hidden"
    >
      <Handle
        type="target"
        position={Position.Left}
        className="!w-1 !h-1 !bg-line-soft !border-0"
      />

      <button
        onClick={(e) => {
          e.stopPropagation()
          if (confirm(`Remove project "${m.name}" from projects.yaml?\n(This doesn't delete the directory on disk.)`)) {
            data.onDelete(m.name)
          }
        }}
        className="absolute top-1.5 right-1.5 opacity-0 group-hover:opacity-100 p-1 rounded text-ink-faint hover:text-red-300 hover:bg-red-500/10 transition"
        title="remove from projects.yaml"
      >
        <Trash2 size={11} />
      </button>

      <div className="flex items-center gap-2 mb-1 pr-5">
        {iconForType(m.type)}
        <span className="text-[13px] font-semibold truncate text-ink" title={m.name}>
          {m.name}
        </span>
      </div>

      <div className="text-[10px] text-ink-mute mb-1.5">
        {m.type ?? "module"}
      </div>

      {m.stack.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-1">
          {m.stack.slice(0, 3).map((s) => (
            <span
              key={s}
              className="text-[9px] font-mono px-1 rounded bg-bg-inset text-ink-mute border border-line/60"
            >
              {s}
            </span>
          ))}
          {m.stack.length > 3 && (
            <span className="text-[9px] text-ink-faint">
              +{m.stack.length - 3}
            </span>
          )}
        </div>
      )}

      {m.provides && (
        <div className="flex items-center gap-1 text-[10px] text-emerald-400/90 truncate" title={m.provides}>
          <ArrowUpRight size={10} className="shrink-0" /> <span className="truncate">{m.provides}</span>
        </div>
      )}
      {m.consumes && (
        <div className="flex items-center gap-1 text-[10px] text-blue-400/90 truncate" title={m.consumes}>
          <ArrowDownLeft size={10} className="shrink-0" /> <span className="truncate">{m.consumes}</span>
        </div>
      )}

      <div className="mt-1.5 flex items-center gap-1 text-[10px] text-ink-faint">
        <Brain size={10} className="shrink-0 text-violet-300/80" />
        {data.decisions > 0 ? (
          <span>{t("arch.decisions", { n: data.decisions })}</span>
        ) : (
          <span className="text-ink-faint/70">{t("arch.noDecisions")}</span>
        )}
      </div>

      <Handle
        type="source"
        position={Position.Right}
        className="!w-1 !h-1 !bg-line-soft !border-0"
      />
    </div>
  )
}

const nodeTypes = { module: ModuleNode }

function layout(
  nodes: Node[],
  edges: { source: string; target: string }[],
  maxLabelChars: number,
) {
  const g = new dagre.graphlib.Graph()
  g.setDefaultEdgeLabel(() => ({}))
  // Scale the column gap by the longest edge label so the label can sit
  // between two ranks without being occluded by the node bodies on either
  // side. ~7px/char + 32px padding is enough for the mono labels we use.
  const labelPx = Math.max(160, maxLabelChars * 7 + 32)
  g.setGraph({
    rankdir: "LR",
    nodesep: 56,
    ranksep: labelPx,
    marginx: 24,
    marginy: 24,
  })
  nodes.forEach((n) => g.setNode(n.id, { width: NODE_W, height: NODE_H }))
  edges.forEach((e) => g.setEdge(e.source, e.target))
  dagre.layout(g)

  // Snap only pure chains. Fan-out / fan-in layouts must keep dagre's Y,
  // otherwise siblings with one shared upstream collapse into the same card.
  const ySnap = new Map<string, number>()
  for (const n of nodes) {
    const dn = g.node(n.id)
    ySnap.set(n.id, dn.y)
  }
  const incoming = new Map<string, number>()
  const outgoing = new Map<string, number>()
  const incident = new Set<string>()
  edges.forEach((e) => {
    incoming.set(e.target, (incoming.get(e.target) ?? 0) + 1)
    outgoing.set(e.source, (outgoing.get(e.source) ?? 0) + 1)
    incident.add(e.source)
    incident.add(e.target)
  })
  for (const e of edges) {
    if ((incoming.get(e.target) ?? 0) === 1 && (outgoing.get(e.source) ?? 0) === 1) {
      const srcY = ySnap.get(e.source)
      if (srcY !== undefined) ySnap.set(e.target, srcY)
    }
  }

  const positioned = nodes.map((n) => {
    const dn = g.node(n.id)
    const y = ySnap.get(n.id) ?? dn.y
    return { ...n, position: { x: dn.x - NODE_W / 2, y: y - NODE_H / 2 } }
  })

  const isolated = positioned.filter((n) => !incident.has(n.id))
  if (isolated.length === 0) {
    return positioned
  }
  if (isolated.length === positioned.length) {
    // No edges at all (e.g. independent monorepo packages): a single dagre
    // column looks broken. Lay out as a grid grouped by subspace so the
    // grouping reads visually instead of one tall list.
    const bySub = new Map<string, Node[]>()
    for (const n of positioned) {
      const sub = subspaceOf((n.data as ModuleNodeData).module) ?? "·"
      if (!bySub.has(sub)) bySub.set(sub, [])
      bySub.get(sub)!.push(n)
    }
    const cols = Math.min(4, Math.max(2, Math.ceil(Math.sqrt(positioned.length))))
    const out: Node[] = []
    let y = 24
    for (const [, group] of [...bySub.entries()].sort((a, b) => b[1].length - a[1].length)) {
      group.forEach((n, i) => {
        out.push({
          ...n,
          position: {
            x: 24 + (i % cols) * (NODE_W + 40),
            y: y + Math.floor(i / cols) * (NODE_H + 28),
          },
        })
      })
      y += Math.ceil(group.length / cols) * (NODE_H + 28) + 56
    }
    return out
  }

  const connected = positioned.filter((n) => incident.has(n.id))
  const maxConnectedX = Math.max(...connected.map((n) => n.position.x))
  const centerY =
    connected.reduce((sum, n) => sum + n.position.y, 0) / connected.length
  // Aim for roughly square: with ~20 isolated nodes, 5 cols × 4 rows reads far
  // better than 3 cols × 7 rows. Capped so we don't sprawl horizontally.
  const isolatedCols = Math.min(8, Math.max(2, Math.ceil(Math.sqrt(isolated.length))))
  const isolatedRows = Math.ceil(isolated.length / isolatedCols)
  const startY = centerY - ((isolatedRows - 1) * (NODE_H + 32)) / 2

  return positioned.map((n) => {
    const isolatedIndex = isolated.findIndex((item) => item.id === n.id)
    if (isolatedIndex === -1) return n
    const col = isolatedIndex % isolatedCols
    const row = Math.floor(isolatedIndex / isolatedCols)
    return {
      ...n,
      position: {
        x: maxConnectedX + NODE_W + 56 + col * (NODE_W + 56),
        y: startY + row * (NODE_H + 32),
      },
    }
  })
}

/** "schemas/openapi.yaml" → "openapi.yaml" when path has dirs and exceeds limit. */
function shortLabel(file: string, limit = 28): string {
  if (file.length <= limit) return file
  const base = file.split("/").pop() ?? file
  return base.length <= limit ? base : "…" + base.slice(-limit + 1)
}

function edgeLabel(edge: ArchEdgeView): string {
  const kind = edge.kind ?? "contract"
  // A code-graph backbone can be 100+ edges; a text label on each is noise.
  // Weight is shown via thickness instead, with detail in the side panel.
  if (kind === "import" || kind === "import+contract") return ""
  const base =
    kind === "dependency"
      ? "depends_on"
      : kind === "dependency+contract"
        ? `${shortLabel(edge.file, 22)} + dep`
        : shortLabel(edge.file)
  // Source-inferred edges carry a confidence; surface it so the user knows the
  // edge was guessed from imports rather than declared.
  return edge.inferred && typeof edge.confidence === "number"
    ? `${base} · ~${edge.confidence}%`
    : base
}

function edgeTone(edge: ArchEdgeView): { stroke: string; animated: boolean } {
  const kind = edge.kind ?? "contract"
  if (kind === "dependency+contract") {
    return { stroke: ARCH_EDGE_COLORS.dependencyAndContract, animated: true }
  }
  if (kind === "dependency") {
    return { stroke: ARCH_EDGE_COLORS.dependency, animated: false }
  }
  // Code-graph import backbone: calm, static (no marching ants on 100+ edges).
  if (kind === "import") {
    return { stroke: ARCH_EDGE_COLORS.import, animated: false }
  }
  // Pair detected as both a code import AND an RPC contract — distinct hue.
  if (kind === "import+contract") {
    return { stroke: ARCH_EDGE_COLORS.importContract, animated: false }
  }
  // Cross-stack RPC contract coupling (warm amber, no animation to stay calm).
  if (kind === "contract") {
    return { stroke: ARCH_EDGE_COLORS.contract, animated: false }
  }
  // Declared-contract edges (with a contract file path) — fall through here.
  return { stroke: ARCH_EDGE_COLORS.contract, animated: true }
}

/** Edge stroke width from a code-graph rollup weight. Kept deliberately thin
 *  (≈1–2.4px) so a dense backbone reads as fine wiring, not heavy cabling —
 *  weight is conveyed by subtle gradation, not bulk. */
function edgeWidth(edge: ArchEdgeView): number {
  const w = edge.weight ?? 0
  return w > 0 ? Math.max(1, Math.min(2.4, 0.7 + Math.log2(w) * 0.32)) : 1.2
}

export function ArchitectureView() {
  useLang()
  const [arch, setArch] = useState<ArchView | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [addOpen, setAddOpen] = useState(false)
  /** Project whose memory panel is currently open (null = closed). */
  const [selectedProject, setSelectedProject] = useState<string | null>(null)
  /** project name → number of L2 decisions, from the memory graph. Ties this
   *  view to the memory tab so each module carries a sign of life. */
  const [decisions, setDecisions] = useState<Map<string, number>>(new Map())

  const reload = async () => {
    try {
      const a = await api.architecture()
      setArch(a)
      setErr(null)
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
    api
      .memoryGraph()
      .then((g) => {
        const m = new Map<string, number>()
        g.nodes.filter((n) => n.kind === "project").forEach((n) => m.set(n.label, n.weight))
        setDecisions(m)
      })
      .catch(() => {})
  }, [])

  const handleDelete = async (name: string) => {
    try {
      await api.deleteProject(name)
      await reload()
    } catch (e) {
      setErr(String(e))
    }
  }

  const { nodes, edges } = useMemo(() => {
    if (!arch) return { nodes: [], edges: [] }
    const baseNodes: Node[] = arch.modules.map((m) => ({
      id: m.name,
      type: "module",
      position: { x: 0, y: 0 },
      data: { module: m, onDelete: handleDelete, decisions: decisions.get(m.name) ?? 0 },
      draggable: true,
      selectable: false,
    }))
    const baseEdges: Edge[] = arch.edges.map((e, i) => {
      const tone = edgeTone(e)
      return {
        id: `${e.from}-${e.to}-${i}`,
        source: e.from,
        target: e.to,
        // Orthogonal routing reads as deliberate "wiring" for a dependency
        // diagram, instead of free-floating bezier arcs across the canvas.
        type: "smoothstep",
        pathOptions: { borderRadius: 12 },
        animated: tone.animated,
        label: edgeLabel(e),
        labelStyle: {
          fontSize: 10,
          fill: "#cbd5e1",
          fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        },
        labelShowBg: true,
        labelBgStyle: { fill: "#111111", fillOpacity: 1 },
        labelBgPadding: [6, 8],
        labelBgBorderRadius: 4,
        // Dash source-inferred edges so "guessed from imports" reads distinctly
        // from declared dependencies/contracts at a glance.
        style: {
          stroke: tone.stroke,
          strokeWidth: edgeWidth(e),
          // Dash only discovery-guessed declared edges; the import backbone is
          // solid (thickness already encodes its strength).
          ...(e.inferred && e.kind !== "import" ? { strokeDasharray: "2 3" } : {}),
        },
        markerEnd: {
          type: MarkerType.ArrowClosed,
          color: tone.stroke,
          width: 16,
          height: 16,
        },
      }
    })
    const maxLabelChars = arch.edges.reduce(
      (n, e) => Math.max(n, edgeLabel(e).length),
      0,
    )
    return { nodes: layout(baseNodes, baseEdges, maxLabelChars), edges: baseEdges }
  }, [arch, decisions])

  if (err) {
    return (
      <div className="flex-1 flex items-center justify-center text-sm text-red-300 p-6">
        {err}
      </div>
    )
  }

  // Distinguish "still loading" from "genuinely empty": the first
  // /api/architecture call scans the whole workspace and can take a few
  // seconds, during which `arch` is null — showing the "no modules" empty
  // state then reads as a bug (it tells you to add projects you already have).
  if (!arch) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center text-center text-sm text-ink-faint p-6 gap-3">
        <Loader2 size={24} className="animate-spin text-ink-faint" />
        <p className="text-ink-dim">{t("arch.loading")}</p>
      </div>
    )
  }

  if (arch.modules.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center text-center text-sm text-ink-faint p-6 gap-3">
        <Cpu size={28} className="text-ink-faint" />
        <p className="max-w-md leading-relaxed">
          <strong className="block mb-1 text-ink-dim">{t("arch.empty.title")}</strong>
          {t("arch.empty.body")}
        </p>
        <button
          onClick={() => setAddOpen(true)}
          className="mt-2 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-600 hover:bg-blue-500 text-white"
        >
          <Plus size={12} /> {t("arch.addProject")}
        </button>
        <p className="text-[11px] text-ink-faint mt-2">
          Or from a shell:{" "}
          <code className="bg-bg-inset px-1 rounded">
            maestro add ~/work/some-repo
          </code>
        </p>

        {addOpen && (
          <AddProjectModal
            existing={[]}
            onClose={() => setAddOpen(false)}
            onCreated={async () => {
              setAddOpen(false)
              await reload()
            }}
          />
        )}
      </div>
    )
  }

  return (
    <ReactFlowProvider>
      <div className="flex-1 flex flex-col min-h-0">
        <header className="px-6 py-3 border-b border-line flex items-center gap-4">
          <h1 className="text-base font-semibold">{t("arch.title")}</h1>
          <span className="text-xs text-ink-faint">
            {arch.modules.length}{" "}
            {arch.modules.length === 1 ? t("arch.modulesUnit") : t("arch.modulesUnitPlural")}{" "}·{" "}
            {arch.edges.length}{" "}
            {arch.edges.length === 1 ? t("arch.edgesUnit") : t("arch.edgesUnitPlural")}
            {(() => {
              const total = [...decisions.values()].reduce((a, b) => a + b, 0)
              return total > 0 ? ` · ${t("arch.decisions", { n: total })}` : ""
            })()}
          </span>

          <LegendMenu />

          <button
            onClick={() => setAddOpen(true)}
            className="ml-auto inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium border border-line text-ink-dim hover:text-ink hover:bg-bg-hover"
            title={t("arch.addProject")}
          >
            <Plus size={11} /> {t("arch.addProject")}
          </button>
        </header>

        <div className="flex-1 min-h-0 arch-flow">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            fitView
            onNodeClick={(_, node) => setSelectedProject(node.id)}
            // Tight padding + a higher max-zoom means a 2-node graph fills the
            // canvas with comfortably-sized nodes instead of rendering as a
            // pair of tiny boxes lost in empty space.
            fitViewOptions={GRAPH_CANVAS.fitView}
            minZoom={0.4}
            maxZoom={1.4}
            proOptions={{ hideAttribution: true }}
            panOnScroll
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

        {addOpen && (
          <AddProjectModal
            existing={arch.modules.map((m) => m.name)}
            onClose={() => setAddOpen(false)}
            onCreated={async () => {
              setAddOpen(false)
              await reload()
            }}
          />
        )}

        {selectedProject && (
          <ProjectMemoryPanel
            projectName={selectedProject}
            onClose={() => setSelectedProject(null)}
          />
        )}
      </div>
    </ReactFlowProvider>
  )
}

function LegendMenu() {
  const [open, setOpen] = useState(false)
  const types: { key: string; label: string; color: string; icon: React.ReactNode }[] = [
    { key: "backend", label: t("arch.legend.backend"), color: moduleTypeColor("backend"), icon: <ServerCog size={10} /> },
    { key: "frontend", label: t("arch.legend.frontend"), color: moduleTypeColor("frontend"), icon: <Globe size={10} /> },
    { key: "mobile", label: t("arch.legend.mobile"), color: moduleTypeColor("mobile"), icon: <Smartphone size={10} /> },
    { key: "tool", label: t("arch.legend.tool"), color: moduleTypeColor("tool"), icon: <Terminal size={10} /> },
    { key: "library", label: t("arch.legend.library"), color: moduleTypeColor("library"), icon: <Library size={10} /> },
  ]
  const edges = [
    { key: "dependency", label: "depends_on", color: ARCH_EDGE_COLORS.dependency, icon: <GitBranch size={10} /> },
    { key: "contract", label: "contract", color: ARCH_EDGE_COLORS.contract, icon: <span className="h-px w-4 border-t border-dashed" /> },
    { key: "both", label: "dep+contract", color: ARCH_EDGE_COLORS.dependencyAndContract, icon: <span className="h-px w-4 border-t border-dashed" /> },
    { key: "import", label: "import (code · thickness=count)", color: ARCH_EDGE_COLORS.import, icon: <span className="h-0.5 w-4 border-t-2" /> },
    { key: "contract-edge", label: "contract (RPC / cross-stack)", color: ARCH_EDGE_COLORS.contract, icon: <span className="h-0.5 w-4 border-t-2" /> },
    { key: "import-contract", label: "import + contract", color: ARCH_EDGE_COLORS.importContract, icon: <span className="h-0.5 w-4 border-t-2" /> },
    { key: "inferred", label: t("arch.legend.inferred"), color: ARCH_EDGE_COLORS.dependency, icon: <span className="h-px w-4 border-t border-dotted" /> },
  ]
  return (
    <div className="relative ml-auto">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="inline-flex items-center gap-1.5 rounded-md border border-line px-2.5 py-1 text-xs font-medium text-ink-dim hover:bg-bg-hover hover:text-ink"
      >
        legend <ChevronDown size={11} />
      </button>
      {open && (
        <div className="absolute right-0 top-full z-20 mt-2 w-64 rounded-lg border border-line bg-bg-panel p-3 text-xs text-ink-dim shadow-2xl">
          <div className="mb-2 text-[10px] uppercase tracking-wider text-ink-faint">types</div>
          <div className="grid grid-cols-2 gap-2">
            {types.map(({ key, label, color, icon }) => (
              <div key={key} className="flex items-center gap-1.5">
                <span className="flex items-center" style={{ color }}>{icon}</span>
                <span>{label}</span>
              </div>
            ))}
          </div>
          <div className="my-3 h-px bg-line" />
          <div className="mb-2 text-[10px] uppercase tracking-wider text-ink-faint">edges</div>
          <div className="space-y-2">
            {edges.map(({ key, label, color, icon }) => (
              <div key={key} className="flex items-center gap-2">
                <span className="flex items-center" style={{ color, borderColor: color }}>{icon}</span>
                <span>{label}</span>
              </div>
            ))}
            <div className="flex items-center gap-2">
              <ArrowUpRight size={10} className="text-emerald-400" />
              <span>{t("arch.legend.provides")}</span>
            </div>
            <div className="flex items-center gap-2">
              <ArrowDownLeft size={10} className="text-blue-400" />
              <span>{t("arch.legend.consumes")}</span>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
