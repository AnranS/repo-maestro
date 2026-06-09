export const TASK_STATUS_COLORS = {
  pending: "#64748b",
  running: "#2563eb",
  done: "#059669",
  failed: "#dc2626",
  awaiting_approval: "#d97706",
  skipped: "#94a3b8",
  cancelled: "#94a3b8",
} as const

export const MODULE_TYPE_COLORS = {
  backend: "#6ee7b7",
  frontend: "#93c5fd",
  mobile: "#c4b5fd",
  tool: "#fcd34d",
  library: "#f9a8d4",
  default: "#a3a3a3",
} as const

export const ARCH_EDGE_COLORS = {
  dependency: "#f59e0b",
  dependencyAndContract: "#8b5cf6",
  // Code-graph-rolled import edges: a calm slate-blue so a dense backbone
  // recedes behind declared dependencies.
  import: "#3f6493",
  // Contract edges — both declared (provides/consumes) and code-derived
  // cross-stack RPC contracts (generated-client naming). Warm amber so they
  // read distinctly from the slate import backbone.
  contract: "#c08550",
  // Both signals on the same pair — a richer hue that says "doubly coupled".
  importContract: "#9a6cb0",
} as const

export const GRAPH_CANVAS = {
  grid: "rgb(var(--graph-grid))",
  nodeBg: "rgb(var(--graph-node-bg))",
  labelBg: "rgb(var(--graph-edge-label-bg))",
  labelFg: "rgb(var(--graph-edge-label-fg))",
  muted: "rgb(var(--graph-node-muted))",
  fitView: { padding: 0.16, minZoom: 0.4, maxZoom: 1.4 },
} as const

export function taskStatusColor(status?: string | null): string {
  return TASK_STATUS_COLORS[status as keyof typeof TASK_STATUS_COLORS] ?? TASK_STATUS_COLORS.pending
}

export function moduleTypeColor(type?: string | null): string {
  return MODULE_TYPE_COLORS[type as keyof typeof MODULE_TYPE_COLORS] ?? MODULE_TYPE_COLORS.default
}

export function withAlpha(hex: string, alpha: number): string {
  const raw = hex.replace("#", "")
  const r = Number.parseInt(raw.slice(0, 2), 16)
  const g = Number.parseInt(raw.slice(2, 4), 16)
  const b = Number.parseInt(raw.slice(4, 6), 16)
  return `rgba(${r}, ${g}, ${b}, ${alpha})`
}
