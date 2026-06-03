export const TASK_STATUS_COLORS = {
  pending: "#3f3f46",
  running: "#3b82f6",
  done: "#10b981",
  failed: "#ef4444",
  awaiting_approval: "#f59e0b",
  skipped: "#525252",
  cancelled: "#525252",
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
  grid: "#1f1f1f",
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
