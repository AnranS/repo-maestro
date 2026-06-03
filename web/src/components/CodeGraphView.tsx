import { useEffect, useMemo, useState } from "react"
import {
  Background,
  Panel,
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
  useReactFlow,
} from "@xyflow/react"
import dagre from "@dagrejs/dagre"
import "@xyflow/react/dist/style.css"
import {
  FileCode,
  Search,
  X,
  Map as MapIcon,
  Sparkles,
  Layers,
  ChevronLeft,
  ChevronDown,
  Cpu,
  Loader2,
} from "lucide-react"
import type { CodeFileNode, CodeGraph, CodeGraphEngine, CodeSymbol, CodeTourStep } from "../types"
import { api } from "../api"
import { t, useLang } from "../i18n"
import { GRAPH_CANVAS } from "./graph/tokens"

// Hard cap on file nodes rendered at once. xyflow draws DOM/SVG per node, so a
// few thousand nodes locks the browser. Drilling a big module (e.g. "app" with
// 5k files) is exactly when this bites — show the most substantial files and a
// "narrow with search" hint instead of freezing.
const MAX_FILE_NODES = 350
const NODE_W = 184
const NODE_H = 46
const LAYER_W = 210
const LAYER_H = 64

const LANG_COLOR: Record<string, string> = {
  rust: "#f59e0b",
  typescript: "#3b82f6",
  tsx: "#60a5fa",
  javascript: "#fcd34d",
}
const EDGE_COLOR: Record<string, string> = {
  calls: "#3b82f6",
  imports: "#f59e0b",
  references: "#64748b",
  implements: "#8b5cf6",
  exports: "#10b981",
  depends_on: "#ef4444",
  module: "#a78bfa",
}
const LAYER_PALETTE = [
  "#60a5fa",
  "#f59e0b",
  "#34d399",
  "#f472b6",
  "#a78bfa",
  "#22d3ee",
  "#fb7185",
  "#facc15",
]

function base(path: string): string {
  return path.split("/").pop() || path
}

// ─── node renderers ────────────────────────────────────────────────────

interface FileNodeData extends Record<string, unknown> {
  path: string
  language?: string | null
  symbols: number
  color: string
  complexity?: string | null
  dim: boolean
}

function FileNodeBox({ data }: NodeProps<Node<FileNodeData>>) {
  return (
    <div
      style={{
        width: NODE_W,
        height: NODE_H,
        borderLeftColor: data.color,
        borderLeftWidth: 3,
        opacity: data.dim ? 0.4 : 1,
      }}
      className="flex flex-col justify-center rounded-lg border border-line bg-bg-panel px-2.5 transition-opacity hover:border-line-soft"
    >
      <Handle type="target" position={Position.Left} className="!w-1 !h-1 !bg-line-soft !border-0" />
      <div className="flex items-center gap-1.5">
        <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: data.color }} />
        <span className="truncate font-mono text-[11px] text-ink" title={data.path}>
          {base(data.path)}
        </span>
      </div>
      <div className="mt-0.5 pl-3.5 text-[9px] text-ink-faint">
        {data.language ?? "?"} · {data.symbols} {t("codegraph.symbols")}
        {data.complexity ? ` · ${data.complexity}` : ""}
      </div>
      <Handle type="source" position={Position.Right} className="!w-1 !h-1 !bg-line-soft !border-0" />
    </div>
  )
}

interface LayerNodeData extends Record<string, unknown> {
  label: string
  count: number
  color: string
}

function LayerNodeBox({ data }: NodeProps<Node<LayerNodeData>>) {
  return (
    <div
      style={{ width: LAYER_W, height: LAYER_H, borderColor: data.color }}
      className="flex flex-col justify-center rounded-xl border-2 bg-bg-panel px-3.5 shadow-lg hover:bg-bg-hover"
    >
      <Handle type="target" position={Position.Left} className="!w-1 !h-1 !bg-line-soft !border-0" />
      <div className="flex items-center gap-2">
        <span className="h-2.5 w-2.5 shrink-0 rounded" style={{ backgroundColor: data.color }} />
        <span className="truncate text-[13px] font-medium text-ink" title={data.label}>
          {data.label}
        </span>
      </div>
      <div className="mt-1 pl-[18px] text-[10px] text-ink-faint">
        {data.count} {t("codegraph.files")} · {t("codegraph.clickToExpand")}
      </div>
      <Handle type="source" position={Position.Right} className="!w-1 !h-1 !bg-line-soft !border-0" />
    </div>
  )
}

const nodeTypes = { file: FileNodeBox, layer: LayerNodeBox }

function layout(nodes: Node[], edges: Edge[], w: number, h: number) {
  const g = new dagre.graphlib.Graph()
  g.setDefaultEdgeLabel(() => ({}))
  g.setGraph({ rankdir: "LR", nodesep: 20, ranksep: 120, marginx: 24, marginy: 24 })
  nodes.forEach((n) => g.setNode(n.id, { width: w, height: h }))
  edges.forEach((e) => g.setEdge(e.source, e.target))
  dagre.layout(g)
  return nodes.map((n) => {
    const dn = g.node(n.id)
    return { ...n, position: { x: dn.x - w / 2, y: dn.y - h / 2 } }
  })
}

// ─── directory rollup ────────────────────────────────────────────────────
// A code graph with no LLM-derived layers (the free `codegraph`/`native`
// engines) renders thousands of file nodes — illegible. Roll files up by their
// leading path segments into a handful of directory "modules" so the default
// view is a readable backbone you can drill into.

interface DirGroup {
  id: string
  name: string
  files: string[]
}

/** Group file paths by their first `depth` segments. Root-level files (fewer
 *  than `depth` segments) bucket under "(root)". Sorted by size, descending. */
function groupByDir(nodes: { path: string }[], depth: number): DirGroup[] {
  const m = new Map<string, string[]>()
  for (const n of nodes) {
    const parts = n.path.split("/")
    const id = parts.length > depth ? parts.slice(0, depth).join("/") : parts.length > 1 ? parts.slice(0, -1).join("/") : "(root)"
    if (!m.has(id)) m.set(id, [])
    m.get(id)!.push(n.path)
  }
  return [...m.entries()]
    .map(([id, files]) => ({ id, name: id, files }))
    .sort((a, b) => b.files.length - a.files.length)
}

/** Grouping depth (1–3) whose module count lands closest to a legible band
 *  (~8–28 boxes). A deep monorepo with thousands of services stays at its
 *  coarse top level by default (expandable via the depth stepper / drill-in)
 *  rather than exploding into a hairball. */
function pickDepth(nodes: { path: string }[]): number {
  const LO = 8
  const HI = 28
  let best = 1
  let bestDist = Infinity
  for (let d = 1; d <= 3; d++) {
    const c = groupByDir(nodes, d).length
    const dist = c < LO ? LO - c : c > HI ? c - HI : 0
    if (dist < bestDist) {
      bestDist = dist
      best = d
    }
  }
  return best
}

/** Build grouped (layer/module) nodes + aggregated cross-group edges. Mutual
 *  dependencies collapse to one double-headed edge; weight = file-edge count. */
function groupedView(
  groups: DirGroup[],
  byPath: Map<string, string>,
  colorOf: (id: string) => string,
  allEdges: { source: string; target: string }[],
): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = groups.map((g) => ({
    id: g.id,
    type: "layer",
    position: { x: 0, y: 0 },
    data: { label: g.name, count: g.files.length, color: colorOf(g.id) },
    draggable: true,
    selectable: false,
  }))
  const pair = new Map<string, { a: string; b: string; ab: number; ba: number }>()
  allEdges.forEach((e) => {
    const gs = byPath.get(e.source)
    const gt = byPath.get(e.target)
    if (!gs || !gt || gs === gt) return
    const [a, b] = gs < gt ? [gs, gt] : [gt, gs]
    const key = `${a}|${b}`
    const rec = pair.get(key) ?? { a, b, ab: 0, ba: 0 }
    if (gs === a) rec.ab += 1
    else rec.ba += 1
    pair.set(key, rec)
  })
  // With many groups, drop one-off cross-group links so the backbone reads;
  // few groups (LLM layers) keep every edge.
  const minPair = groups.length > 24 ? 3 : 1
  const edges: Edge[] = [...pair.values()]
    .filter((r) => r.ab + r.ba >= minPair)
    .map((r, i) => {
    const forward = r.ab >= r.ba
    const source = forward ? r.a : r.b
    const target = forward ? r.b : r.a
    const total = r.ab + r.ba
    const mutual = r.ab > 0 && r.ba > 0
    const arrow = { type: MarkerType.ArrowClosed, color: "#64748b", width: 13, height: 13 }
    return {
      id: `${source}->${target}-${i}`,
      source,
      target,
      label: String(total),
      type: "smoothstep",
      pathOptions: { borderRadius: 12 },
      style: { stroke: "#64748b", strokeWidth: Math.min(1 + total / 8, 5) },
      labelStyle: { fill: "#94a3b8", fontSize: 9 },
      labelBgStyle: { fill: "#0b0f17" },
      markerEnd: arrow,
      markerStart: mutual ? arrow : undefined,
    }
  })
  // Sparse-edge fallback: dagre LR degenerates to a long single column when
  // many groups are mutually disconnected (the standard "many components, few
  // edges" failure mode). Lay them out as a compact grid instead — same fix
  // pattern as the architecture view's edgeless case.
  if (edges.length < nodes.length * 0.5) {
    const cols = Math.max(2, Math.ceil(Math.sqrt(nodes.length)))
    const xStep = LAYER_W + 60
    const yStep = LAYER_H + 30
    const positioned = nodes.map((n, i) => ({
      ...n,
      position: { x: (i % cols) * xStep, y: Math.floor(i / cols) * yStep },
    }))
    return { nodes: positioned, edges }
  }
  return { nodes: layout(nodes, edges, LAYER_W, LAYER_H), edges }
}

// ─── top-level ─────────────────────────────────────────────────────────

export function CodeGraphView() {
  useLang()
  const [graph, setGraph] = useState<CodeGraph | null>(null)
  const [query, setQuery] = useState("")
  const [selected, setSelected] = useState<string | null>(null)
  const [showTour, setShowTour] = useState(false)
  // view mode: layer overview vs files. Drilling a layer sets `activeLayer`.
  const [mode, setMode] = useState<"layers" | "modules" | "files">("modules")
  const [activeLayer, setActiveLayer] = useState<string | null>(null)
  // module drill-down: the directory-group id whose files are shown.
  const [activeModule, setActiveModule] = useState<string | null>(null)
  // directory-rollup depth (1–3); -1 sentinel = not yet auto-picked.
  const [dirDepth, setDirDepth] = useState(-1)
  // tour-driven highlight: file paths to spotlight + fit to.
  const [tourFocus, setTourFocus] = useState<string[] | null>(null)
  const [didInit, setDidInit] = useState(false)

  useEffect(() => {
    api
      .codegraph()
      .then((g) => {
        setGraph(g)
        if (!didInit) {
          // Layer overview if the graph ships LLM layers; otherwise the
          // directory-module rollup — both keep a big graph legible. The raw
          // files view is never the default (it's a 26k-node web).
          setMode(g.layers && g.layers.length > 0 ? "layers" : "modules")
          if (dirDepth < 0 && g.nodes.length) setDirDepth(pickDepth(g.nodes))
          setDidInit(true)
        }
      })
      .catch(() => setGraph({ nodes: [], edges: [], available: false }))
  }, [didInit, dirDepth])

  const layerColor = useMemo(() => {
    const m = new Map<string, string>()
    graph?.layers?.forEach((l, i) => m.set(l.id, LAYER_PALETTE[i % LAYER_PALETTE.length]))
    return m
  }, [graph])

  const layerByPath = useMemo(() => {
    const m = new Map<string, string>()
    graph?.layers?.forEach((l) => l.files.forEach((f) => m.set(f, l.id)))
    return m
  }, [graph])

  const layerName = useMemo(() => {
    const m = new Map<string, string>()
    graph?.layers?.forEach((l) => m.set(l.id, l.name))
    return m
  }, [graph])

  const nodeByPath = useMemo(() => {
    const m = new Map<string, CodeFileNode>()
    graph?.nodes.forEach((n) => m.set(n.path, n))
    return m
  }, [graph])

  // Directory-rollup grouping (used when there are no LLM layers, and always
  // available as the "Modules" view).
  const depth = dirDepth < 0 ? 2 : dirDepth
  const moduleGroups = useMemo(
    () => (graph ? groupByDir(graph.nodes, depth) : []),
    [graph, depth],
  )
  const moduleColor = useMemo(() => {
    const m = new Map<string, string>()
    moduleGroups.forEach((g, i) => m.set(g.id, LAYER_PALETTE[i % LAYER_PALETTE.length]))
    return m
  }, [moduleGroups])
  const moduleByPath = useMemo(() => {
    const m = new Map<string, string>()
    moduleGroups.forEach((g) => g.files.forEach((f) => m.set(f, g.id)))
    return m
  }, [moduleGroups])

  const hasLayers = !!graph?.layers && graph.layers.length > 0

  const openTourStep = (paths: string[]) => {
    setMode("files")
    setActiveLayer(null)
    setActiveModule(null)
    setTourFocus(paths)
    setShowTour(false)
  }

  if (graph && !graph.available) {
    return (
      <div className="flex-1 flex items-center justify-center p-8 text-center text-sm text-ink-faint">
        <p className="max-w-md leading-relaxed">{t("codegraph.empty")}</p>
      </div>
    )
  }

  const rich = graph?.source === "understand-anything"

  return (
    <ReactFlowProvider>
      <div className="flex-1 flex flex-col min-h-0">
        <header className="px-6 py-3 border-b border-line flex items-center gap-3 flex-wrap">
          <h1 className="flex items-center gap-2 text-base font-semibold">
            <FileCode size={16} className="text-blue-300" /> {t("tab.codegraph")}
          </h1>
          {graph && (
            <span
              className="hidden items-center gap-1 rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-faint sm:inline-flex"
              title={t(`codegraph.source.${graph.source ?? "native"}`)}
            >
              {rich && <Sparkles size={10} className="text-violet-300" />}
              {t(`codegraph.source.${graph.source ?? "native"}`)}
            </span>
          )}
          <EngineChooser onBuilt={() => api.codegraph().then(setGraph).catch(() => {})} />

          {/* view toggle: Layers (if any) · Modules (directory rollup) · Files */}
          <div className="flex items-center rounded-lg border border-line bg-bg-inset p-0.5">
            {hasLayers && (
              <button
                onClick={() => {
                  setMode("layers")
                  setActiveLayer(null)
                  setActiveModule(null)
                  setTourFocus(null)
                }}
                className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "layers" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
              >
                <Layers size={12} /> {t("codegraph.layersView")}
              </button>
            )}
            <button
              onClick={() => {
                setMode("modules")
                setActiveLayer(null)
                setActiveModule(null)
                setTourFocus(null)
              }}
              className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "modules" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              <Layers size={12} /> {t("codegraph.modulesView")}
            </button>
            <button
              onClick={() => {
                setMode("files")
                setActiveLayer(null)
                setActiveModule(null)
              }}
              className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "files" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              <FileCode size={12} /> {t("codegraph.filesView")}
            </button>
          </div>

          {/* depth stepper (modules overview only) */}
          {mode === "modules" && (
            <div className="flex items-center gap-1 rounded-md border border-line px-1.5 py-0.5 text-[11px] text-ink-faint">
              <span>{t("codegraph.depth")}</span>
              <button
                onClick={() => setDirDepth((d) => Math.max(1, (d < 0 ? 2 : d) - 1))}
                disabled={depth <= 1}
                className="px-1 text-ink-dim hover:text-ink disabled:opacity-30"
              >
                −
              </button>
              <span className="w-3 text-center font-mono text-ink-dim">{depth}</span>
              <button
                onClick={() => setDirDepth((d) => Math.min(4, (d < 0 ? 2 : d) + 1))}
                disabled={depth >= 4}
                className="px-1 text-ink-dim hover:text-ink disabled:opacity-30"
              >
                +
              </button>
              <span className="ml-1 text-ink-faint">· {moduleGroups.length}</span>
            </div>
          )}

          {/* breadcrumb when drilled into a layer */}
          {mode === "files" && activeLayer && (
            <button
              onClick={() => {
                setMode("layers")
                setActiveLayer(null)
              }}
              className="flex items-center gap-1 rounded-md border border-line px-2 py-1 text-[11px] text-ink-dim hover:bg-bg-hover"
            >
              <ChevronLeft size={11} /> {layerName.get(activeLayer) ?? t("codegraph.layersView")}
            </button>
          )}

          {/* breadcrumb when drilled into a module */}
          {mode === "files" && activeModule && (
            <button
              onClick={() => {
                setMode("modules")
                setActiveModule(null)
              }}
              className="flex items-center gap-1 rounded-md border border-line px-2 py-1 font-mono text-[11px] text-ink-dim hover:bg-bg-hover"
            >
              <ChevronLeft size={11} /> {activeModule}
            </button>
          )}

          <span className="hidden text-xs text-ink-faint lg:inline">
            {graph ? `${graph.nodes.length} files · ${graph.edges.length} edges` : ""}
          </span>

          {graph?.tour && graph.tour.length > 0 && (
            <button
              onClick={() => setShowTour((s) => !s)}
              className={`flex items-center gap-1 rounded-md border px-2 py-1 text-xs ${showTour ? "border-violet-500/50 bg-violet-500/10 text-violet-200" : "border-line text-ink-dim hover:bg-bg-hover"}`}
            >
              <MapIcon size={12} /> {t("codegraph.tour")} · {graph.tour.length}
            </button>
          )}

          {mode === "files" && (
            <div className="relative ml-auto">
              <Search size={12} className="absolute left-2 top-1/2 -translate-y-1/2 text-ink-faint" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t("codegraph.searchPlaceholder")}
                className="w-44 rounded-md border border-line bg-bg-inset pl-7 pr-2 py-1 text-xs focus:outline-none focus:border-blue-600"
              />
            </div>
          )}
        </header>

        {/* layer legend (files view) */}
        {mode === "files" && hasLayers && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-6 py-2">
            {!activeLayer && (
              <span className="text-[10px] text-ink-faint">{t("codegraph.focusHint")}</span>
            )}
            {graph!.layers!.map((l) => (
              <button
                key={l.id}
                onClick={() => {
                  setMode("files")
                  setActiveLayer(l.id)
                  setTourFocus(null)
                }}
                className={`flex items-center gap-1.5 text-[10px] ${activeLayer === l.id ? "text-ink" : "text-ink-dim hover:text-ink"}`}
                title={l.description ?? l.name}
              >
                <span className="h-2 w-2 rounded-sm" style={{ backgroundColor: layerColor.get(l.id) }} />
                {l.name}
              </button>
            ))}
          </div>
        )}

        {/* modules overview hint */}
        {mode === "modules" && (
          <div className="border-b border-line px-6 py-2 text-[10px] text-ink-faint">
            {t("codegraph.modulesHint")}
          </div>
        )}

        <div className="flex-1 min-h-0 arch-flow">
          {graph ? (
            <GraphCanvas
              graph={graph}
              mode={mode}
              activeLayer={activeLayer}
              activeModule={activeModule}
              query={query}
              tourFocus={tourFocus}
              layerColor={layerColor}
              layerByPath={layerByPath}
              moduleGroups={moduleGroups}
              moduleColor={moduleColor}
              moduleByPath={moduleByPath}
              onDrillLayer={(id) => {
                setMode("files")
                setActiveLayer(id)
                setTourFocus(null)
              }}
              onDrillModule={(id) => {
                setMode("files")
                setActiveModule(id)
                setTourFocus(null)
              }}
              onPickFile={(path) => setSelected(path)}
              onClearTourFocus={() => setTourFocus(null)}
            />
          ) : (
            // `graph` is null only while the first /api/codegraph/graph call is
            // in flight — that call scans the workspace and can take seconds. An
            // empty canvas there looks broken; show an explicit loading state.
            <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-sm text-ink-faint">
              <Loader2 size={24} className="animate-spin text-ink-faint" />
              <p className="text-ink-dim">{t("codegraph.loading")}</p>
            </div>
          )}
        </div>

        {showTour && graph?.tour && (
          <TourDrawer tour={graph.tour} onClose={() => setShowTour(false)} onPick={openTourStep} />
        )}

        {selected && (
          <SymbolPanel
            path={selected}
            node={nodeByPath.get(selected) ?? null}
            layerName={layerName.get(nodeByPath.get(selected)?.layer ?? "") ?? null}
            root={graph?.root ?? null}
            onClose={() => setSelected(null)}
          />
        )}
      </div>
    </ReactFlowProvider>
  )
}

// ─── the canvas (inside the provider, so it can fitView) ─────────────────

function GraphCanvas({
  graph,
  mode,
  activeLayer,
  activeModule,
  query,
  tourFocus,
  layerColor,
  layerByPath,
  moduleGroups,
  moduleColor,
  moduleByPath,
  onDrillLayer,
  onDrillModule,
  onPickFile,
  onClearTourFocus,
}: {
  graph: CodeGraph
  mode: "layers" | "modules" | "files"
  activeLayer: string | null
  activeModule: string | null
  query: string
  tourFocus: string[] | null
  layerColor: Map<string, string>
  layerByPath: Map<string, string>
  moduleGroups: DirGroup[]
  moduleColor: Map<string, string>
  moduleByPath: Map<string, string>
  onDrillLayer: (id: string) => void
  onDrillModule: (id: string) => void
  onPickFile: (path: string) => void
  onClearTourFocus: () => void
}) {
  const rf = useReactFlow()
  const [hovered, setHovered] = useState<string | null>(null)

  // adjacency for focus highlighting (files view)
  const adjacency = useMemo(() => {
    const m = new Map<string, Set<string>>()
    graph.edges.forEach((e) => {
      if (!m.has(e.source)) m.set(e.source, new Set())
      if (!m.has(e.target)) m.set(e.target, new Set())
      m.get(e.source)!.add(e.target)
      m.get(e.target)!.add(e.source)
    })
    return m
  }, [graph])

  // base layout (recomputed only when the structural inputs change, not on hover)
  const base = useMemo(() => {
    // Grouped overviews (LLM layers, or the directory-module rollup) share one
    // renderer: group nodes + aggregated cross-group edges.
    if (mode === "layers") {
      const groups: DirGroup[] = (graph.layers ?? []).map((l) => ({ id: l.id, name: l.name, files: l.files }))
      return { ...groupedView(groups, layerByPath, (id) => layerColor.get(id) ?? "#a3a3a3", graph.edges), hiddenCount: 0 }
    }
    if (mode === "modules") {
      return { ...groupedView(moduleGroups, moduleByPath, (id) => moduleColor.get(id) ?? "#a3a3a3", graph.edges), hiddenCount: 0 }
    }

    // files view
    const connected = new Set<string>()
    graph.edges.forEach((e) => {
      connected.add(e.source)
      connected.add(e.target)
    })
    const q = query.trim().toLowerCase()
    const matched = graph.nodes.filter(
      (n) =>
        connected.has(n.path) &&
        (!activeLayer || layerByPath.get(n.path) === activeLayer) &&
        (!activeModule || moduleByPath.get(n.path) === activeModule) &&
        (!q || n.path.toLowerCase().includes(q)),
    )
    // Cap to the most substantial files (by symbol count) so a big module
    // doesn't freeze the canvas; the rest are reachable via search.
    const total = matched.length
    const visible =
      total > MAX_FILE_NODES
        ? [...matched]
            .sort((a, b) => (b.symbols ?? 0) - (a.symbols ?? 0) || a.path.localeCompare(b.path))
            .slice(0, MAX_FILE_NODES)
        : matched
    const hiddenCount = total - visible.length
    const visibleSet = new Set(visible.map((n) => n.path))
    const nodes: Node[] = visible.map((n) => ({
      id: n.path,
      type: "file",
      position: { x: 0, y: 0 },
      data: {
        path: n.path,
        language: n.language,
        symbols: n.symbols,
        complexity: n.complexity,
        color: (n.layer && layerColor.get(n.layer)) || LANG_COLOR[n.language ?? ""] || "#a3a3a3",
        dim: false,
      },
      draggable: true,
      selectable: false,
    }))
    // In the all-files view the full edge set is an overwhelming web, so it's
    // de-emphasised (faint) — the overview is the place to read structure, and
    // drilling into a single layer/module brings its edges to full strength.
    const faint = !activeLayer && !activeModule
    const edges: Edge[] = graph.edges
      .filter((e) => visibleSet.has(e.source) && visibleSet.has(e.target))
      .map((e, i) => {
        const stroke = EDGE_COLOR[e.kind] ?? "#64748b"
        return {
          id: `${e.source}->${e.target}-${i}`,
          source: e.source,
          target: e.target,
          type: "smoothstep",
          pathOptions: { borderRadius: 8 },
          style: { stroke, strokeWidth: 1.4, opacity: faint ? 0.3 : 1 },
          markerEnd: { type: MarkerType.ArrowClosed, color: stroke, width: 14, height: 14 },
        }
      })
    return { nodes: layout(nodes, edges, NODE_W, NODE_H), edges, hiddenCount }
  }, [graph, mode, activeLayer, activeModule, query, layerColor, layerByPath, moduleGroups, moduleColor, moduleByPath])

  // The "lit" set: hovered node + neighbours, else the tour-focused paths —
  // but always intersected with what's actually on screen, so a stale tour
  // focus (or a focus on filtered-out files) can never dim the whole graph to
  // black. `null` ⇒ no focus, everything at full opacity.
  const lit = useMemo(() => {
    if (mode !== "files") return null
    const visibleIds = new Set(base.nodes.map((n) => n.id))
    let s: Set<string> | null = null
    if (hovered && visibleIds.has(hovered)) {
      s = new Set([hovered])
      adjacency.get(hovered)?.forEach((n) => visibleIds.has(n) && s!.add(n))
    } else if (tourFocus && tourFocus.length > 0) {
      const inView = tourFocus.filter((p) => visibleIds.has(p))
      if (inView.length > 0) s = new Set(inView)
    }
    return s && s.size > 0 ? s : null
  }, [mode, tourFocus, hovered, adjacency, base.nodes])

  // Dimming is applied to NODES only (opacity on the node box). Edges are left
  // untouched on hover — recreating edge objects re-draws their SVG paths and
  // markers every mouse-move, which reads as a flicker. Node focus alone is
  // enough to pull the eye, and the edges stay rock-steady.
  const nodes = useMemo(() => {
    if (!lit) return base.nodes
    return base.nodes.map((n) => ({ ...n, data: { ...n.data, dim: !lit.has(n.id) } }))
  }, [base.nodes, lit])

  const edges = base.edges

  // Fit the view ONLY when the structure changes (mode / drill / filter / load)
  // — never on hover, so hovering can't shift or shake the canvas.
  useEffect(() => {
    if (tourFocus && tourFocus.length > 0) return // tour effect handles framing
    const h = setTimeout(() => rf.fitView({ duration: 300, ...GRAPH_CANVAS.fitView }), 90)
    return () => clearTimeout(h)
  }, [mode, activeLayer, activeModule, query, graph, rf, tourFocus, base.nodes])

  // When a tour step is picked, frame just its (on-screen) nodes.
  useEffect(() => {
    if (mode === "files" && tourFocus && tourFocus.length > 0) {
      const ids = tourFocus.map((p) => ({ id: p }))
      const h = setTimeout(() => rf.fitView({ nodes: ids, duration: 500, padding: 0.4 }), 120)
      return () => clearTimeout(h)
    }
  }, [mode, tourFocus, rf])

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      minZoom={0.15}
      maxZoom={1.6}
      proOptions={{ hideAttribution: true }}
      panOnScroll
      zoomOnDoubleClick={false}
      onNodeMouseEnter={(_, n) => {
        if (mode === "files") {
          setHovered(n.id)
          onClearTourFocus() // browsing clears a stale tour spotlight
        }
      }}
      onNodeMouseLeave={() => setHovered(null)}
      onPaneClick={() => {
        setHovered(null)
        onClearTourFocus()
      }}
      onNodeClick={(_, n) => {
        if (mode === "layers") onDrillLayer(n.id)
        else if (mode === "modules") onDrillModule(n.id)
        else onPickFile(n.id)
      }}
    >
      <Background variant={BackgroundVariant.Dots} gap={20} size={1} color={GRAPH_CANVAS.grid} />
      {base.hiddenCount > 0 && (
        <Panel position="top-center">
          <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-1 text-[11px] text-amber-200 shadow">
            showing the {MAX_FILE_NODES} largest files · {base.hiddenCount} more hidden — type in search to narrow
          </div>
        </Panel>
      )}
      <Controls
        showInteractive={false}
        position="bottom-right"
        className="!bg-bg-panel !border-line !rounded !overflow-hidden"
      />
    </ReactFlow>
  )
}

// ─── side panels ─────────────────────────────────────────────────────────

function SymbolPanel({
  path,
  node,
  layerName,
  root,
  onClose,
}: {
  path: string
  node: CodeFileNode | null
  layerName: string | null
  root: string | null
  onClose: () => void
}) {
  const [symbols, setSymbols] = useState<CodeSymbol[] | null>(null)
  useEffect(() => {
    setSymbols(null)
    api.codegraphFile(path).then(setSymbols).catch(() => setSymbols([]))
  }, [path])
  const jump = (line?: number | null) =>
    root ? `vscode://file${root}/${path}${line != null ? `:${line}` : ""}` : undefined
  return (
    <div className="absolute right-0 top-0 bottom-0 z-20 flex w-80 flex-col border-l border-line bg-bg-panel shadow-2xl">
      <div className="flex items-start gap-2 border-b border-line px-3 py-2.5">
        <FileCode size={14} className="mt-0.5 shrink-0 text-blue-300" />
        <div className="min-w-0 flex-1">
          <div className="truncate font-mono text-xs text-ink" title={path}>
            {base(path)}
          </div>
          <div className="truncate font-mono text-[10px] text-ink-faint" title={path}>
            {path}
          </div>
        </div>
        <button onClick={onClose} className="rounded p-1 text-ink-faint hover:text-ink hover:bg-bg-hover">
          <X size={14} />
        </button>
      </div>

      {(node?.summary || (node?.tags && node.tags.length > 0) || layerName) && (
        <div className="border-b border-line px-3 py-2.5">
          {node?.summary && <p className="text-[11px] leading-relaxed text-ink-dim">{node.summary}</p>}
          <div className="mt-2 flex flex-wrap items-center gap-1">
            {layerName && (
              <span className="rounded border border-violet-500/30 bg-violet-500/10 px-1.5 py-0.5 text-[9px] text-violet-200">
                {layerName}
              </span>
            )}
            {node?.tags?.map((tag) => (
              <span key={tag} className="rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[9px] text-ink-mute">
                {tag}
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto p-2 scrollbar-thin">
        {!symbols ? (
          <div className="p-2 text-xs text-ink-faint">loading…</div>
        ) : symbols.length === 0 ? (
          <div className="p-2 text-xs text-ink-faint">no symbols</div>
        ) : (
          symbols.map((s, i) => (
            <a
              key={`${s.name}-${i}`}
              href={jump(s.line)}
              title={`${s.signature ?? s.name}${root ? " · open in editor" : ""}`}
              className="block rounded px-2 py-1.5 hover:bg-bg-hover"
            >
              <div className="flex items-center gap-2 text-xs">
                <span className="w-16 shrink-0 truncate text-[9px] uppercase tracking-wider text-ink-faint">
                  {s.kind}
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-ink-dim">{s.name}</span>
                {s.line != null && <span className="shrink-0 font-mono text-[10px] text-ink-faint">L{s.line}</span>}
              </div>
              {s.signature && (
                <div className="mt-0.5 pl-[72px] text-[10px] leading-snug text-ink-faint line-clamp-2">
                  {s.signature}
                </div>
              )}
            </a>
          ))
        )}
      </div>
    </div>
  )
}

function TourDrawer({
  tour,
  onClose,
  onPick,
}: {
  tour: CodeTourStep[]
  onClose: () => void
  onPick: (paths: string[]) => void
}) {
  return (
    <div className="absolute left-0 top-0 bottom-0 z-20 flex w-96 flex-col border-r border-line bg-bg-panel shadow-2xl">
      <div className="flex items-center gap-2 border-b border-line px-3 py-2.5">
        <MapIcon size={14} className="text-violet-300" />
        <span className="flex-1 text-sm font-semibold text-ink">{t("codegraph.tour")}</span>
        <button onClick={onClose} className="rounded p-1 text-ink-faint hover:text-ink hover:bg-bg-hover">
          <X size={14} />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3 scrollbar-thin space-y-3">
        {[...tour]
          .sort((a, b) => a.order - b.order)
          .map((step) => (
            <button
              key={step.order}
              onClick={() => step.files && step.files.length > 0 && onPick(step.files)}
              className="block w-full rounded-lg border border-line bg-bg-inset p-3 text-left transition-colors hover:border-violet-500/40 hover:bg-bg-hover"
            >
              <div className="mb-1 flex items-baseline gap-2">
                <span className="font-mono text-[10px] text-violet-300">{step.order}</span>
                <span className="text-xs font-medium text-ink">{step.title}</span>
              </div>
              <p className="text-[11px] leading-relaxed text-ink-mute">{step.description}</p>
              {step.language_lesson && (
                <p className="mt-1.5 border-l-2 border-violet-500/40 pl-2 text-[10px] leading-relaxed text-ink-faint">
                  💡 {step.language_lesson}
                </p>
              )}
              {step.files && step.files.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1">
                  {step.files.map((f) => (
                    <span
                      key={f}
                      className="rounded border border-line bg-bg-panel px-1.5 py-0.5 font-mono text-[9px] text-ink-dim"
                      title={f}
                    >
                      {base(f)}
                    </span>
                  ))}
                </div>
              )}
            </button>
          ))}
      </div>
    </div>
  )
}

/**
 * Code-graph engine chooser: shows the available engines (native / codegraph /
 * understand-anything) with cost + state, and lets the developer build a richer
 * one. codegraph builds server-side (free); understand shows the `/understand`
 * command to run in Claude Code (LLM — uses tokens). Explicit, never silent.
 */
function EngineChooser({ onBuilt }: { onBuilt: () => void }) {
  const [open, setOpen] = useState(false)
  const [engines, setEngines] = useState<CodeGraphEngine[] | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)

  const load = () => api.codegraphEngines().then(setEngines).catch(() => setEngines(null))
  useEffect(() => {
    if (open && !engines) load()
  }, [open, engines])

  const build = async (engine: string) => {
    setNote(null)
    setBusy(engine)
    try {
      const r = await api.codegraphBuild(engine)
      if (r.manual && r.command) {
        setNote(`${r.command}  —  ${r.note ?? ""}`)
      } else if (r.ok) {
        await load()
        onBuilt()
      }
    } catch (e) {
      setNote(String(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1 rounded border border-line px-2 py-0.5 text-[11px] text-ink-dim hover:bg-bg-hover hover:text-ink"
      >
        <Cpu size={11} /> {t("codegraph.engine")} <ChevronDown size={10} />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-20 mt-1 w-[22rem] rounded-lg border border-line bg-bg-panel p-2 shadow-xl">
          <div className="px-1 pb-1 text-[10px] uppercase tracking-wider text-ink-faint">
            {t("codegraph.engineTitle")}
          </div>
          {engines
            ?.filter((e) => e.engine !== "native")
            .map((e) => (
            <div key={e.engine} className="flex items-start gap-2 rounded px-1.5 py-1.5 hover:bg-bg-inset">
              <span className="mt-0.5 text-xs">{e.active ? "★" : "○"}</span>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5 text-xs text-ink">
                  <span className="font-medium">{e.engine}</span>
                  <span
                    className={`rounded px-1 text-[9px] ${
                      e.cost === "free"
                        ? "bg-emerald-500/15 text-emerald-300"
                        : "bg-amber-500/15 text-amber-300"
                    }`}
                  >
                    {e.cost}
                  </span>
                </div>
                <div className="truncate text-[10px] text-ink-faint" title={e.hint}>
                  {e.hint}
                </div>
              </div>
              {!e.active && (e.engine === "codegraph" || e.engine.startsWith("understand")) && (
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => build(e.engine === "codegraph" ? "codegraph" : "understand")}
                  className="mt-0.5 inline-flex items-center gap-1 rounded border border-line px-1.5 py-0.5 text-[10px] text-ink-dim hover:bg-bg-hover hover:text-ink disabled:opacity-50"
                >
                  {busy === e.engine ? <Loader2 size={10} className="animate-spin" /> : null}
                  {t("codegraph.build")}
                </button>
              )}
            </div>
          ))}
          {note && (
            <div className="mt-1 rounded border border-line bg-bg-inset px-2 py-1 font-mono text-[10px] text-ink-mute break-all">
              {note}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
