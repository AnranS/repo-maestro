// F-UI-001: the single shared status-token set. Every operator surface (health
// strip, dashboard, task list, inspector tabs) renders status through this, so
// color answers a status question consistently and never decoratively. `blocked`
// is a needs-action state (not muted); only skipped/cancelled are faint.

export type StatusGroup = "needs-action" | "active" | "ok" | "idle"

export interface StatusToken {
  label: string
  /** chip classes (bg + text + border) */
  chip: string
  /** dot color class */
  dot: string
  group: StatusGroup
}

const TOKENS: Record<string, StatusToken> = {
  running: {
    label: "running",
    chip: "bg-blue-500/15 text-status-info border border-blue-500/20",
    dot: "bg-blue-400",
    group: "active",
  },
  done: {
    label: "done",
    chip: "bg-emerald-500/15 text-status-success border border-emerald-500/20",
    dot: "bg-emerald-400",
    group: "ok",
  },
  failed: {
    label: "failed",
    chip: "bg-red-500/15 text-status-danger border border-red-500/25",
    dot: "bg-red-400",
    group: "needs-action",
  },
  // needs-action — NOT muted: a blocked task wants diagnosis/unblocking.
  blocked: {
    label: "blocked",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/25",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  awaiting_approval: {
    label: "awaiting approval",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/20",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  resume_unsafe: {
    label: "resume unsafe",
    chip: "bg-red-500/15 text-status-danger border border-red-500/25",
    dot: "bg-red-400",
    group: "needs-action",
  },
  risk_high: {
    label: "high risk",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/25",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  // F-UI-003 Slice 0: semantic aliases so findings severity / gate render through
  // the SAME set (mapping locked in F-UI-003 §8.4) — business components must not
  // hand-mix colors. critical→failed-tone, high/medium→amber, low/info→muted,
  // gate→awaiting_approval tone. Additive: nothing consumes these until Slice 1.
  severity_critical: {
    label: "critical",
    chip: "bg-red-500/15 text-status-danger border border-red-500/25",
    dot: "bg-red-400",
    group: "needs-action",
  },
  severity_high: {
    label: "high",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/25",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  severity_medium: {
    label: "medium",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/25",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  severity_low: {
    label: "low",
    chip: "bg-bg-inset text-ink-faint border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
  severity_info: {
    label: "info",
    chip: "bg-bg-inset text-ink-faint border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
  gate: {
    label: "gate",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/20",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  pending: {
    label: "pending",
    chip: "bg-bg-inset text-ink-dim border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
  // chat / runtime states (F-UI-002)
  idle: {
    label: "idle",
    chip: "bg-bg-inset text-ink-faint border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
  streaming: {
    label: "streaming",
    chip: "bg-blue-500/15 text-status-info border border-blue-500/20",
    dot: "bg-blue-400",
    group: "active",
  },
  // a healthy live connection (distinct from a "done" task, same ok tone).
  live: {
    label: "live",
    chip: "bg-emerald-500/15 text-status-success border border-emerald-500/20",
    dot: "bg-emerald-400",
    group: "ok",
  },
  reconnecting: {
    label: "reconnecting",
    chip: "bg-amber-500/15 text-status-warning border border-amber-500/25",
    dot: "bg-amber-400",
    group: "needs-action",
  },
  error: {
    label: "error",
    chip: "bg-red-500/15 text-status-danger border border-red-500/25",
    dot: "bg-red-400",
    group: "needs-action",
  },
  skipped: {
    label: "skipped",
    chip: "bg-bg-inset text-ink-faint border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
  cancelled: {
    label: "cancelled",
    chip: "bg-bg-inset text-ink-faint border border-line",
    dot: "bg-ink-faint",
    group: "idle",
  },
}

const FALLBACK: StatusToken = {
  label: "—",
  chip: "bg-bg-inset text-ink-dim border border-line",
  dot: "bg-ink-faint",
  group: "idle",
}

export function statusToken(status: string): StatusToken {
  return TOKENS[status] ?? { ...FALLBACK, label: status || "—" }
}

/** Map a finding severity (critical/high/medium/low/info) onto the shared token
 *  set via its `severity_*` alias (F-UI-003 §8.4). Unknown values fall back. */
export function severityToken(severity: string): StatusToken {
  return statusToken(`severity_${severity.toLowerCase()}`)
}

// Text-color variants of the status dots — kept as literals so Tailwind keeps
// them, but derived from the single token source via `.dot`. Lets semantic
// icons (mailbox status, pass/fail) share the status language without inline
// per-component colors.
const STATUS_TEXT: Record<string, string> = {
  "bg-blue-400": "text-blue-400",
  "bg-emerald-400": "text-emerald-400",
  "bg-red-400": "text-red-400",
  "bg-amber-400": "text-amber-400",
  "bg-ink-faint": "text-ink-faint",
}

/** The text color for a status, for icons that carry status meaning. */
export function statusTextColor(status: string): string {
  return STATUS_TEXT[statusToken(status).dot] ?? "text-ink-faint"
}

/** A compact status chip. Pass an override `label` for run-status wording. */
export function StatusChip({
  status,
  label,
}: {
  status: string
  label?: string
}) {
  const tok = statusToken(status)
  return (
    <span
      className={`inline-flex shrink-0 items-center rounded px-1.5 py-0.5 text-[10px] font-medium ${tok.chip}`}
    >
      {label ?? tok.label}
    </span>
  )
}

/** A status dot + label, for the health strip's compact metric form. */
export function StatusDot({
  status,
  children,
}: {
  status: string
  children: React.ReactNode
}) {
  const tok = statusToken(status)
  return (
    <span className="inline-flex items-center gap-1.5">
      <span className={`h-1.5 w-1.5 rounded-full ${tok.dot}`} />
      {children}
    </span>
  )
}
