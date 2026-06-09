// F-135b: a tone-keyed companion to StatusChip (which is status-string-keyed).
// Many surfaces carry a SEMANTIC tone (danger/warning/success/info/neutral) that
// isn't one of the named run/task statuses — error panels, "blocked on", bypass
// stage, destructive buttons. They used raw `text-{red,amber,emerald,blue}-300`
// foregrounds: fine on dark, too pale on light. `statusToneClasses` is the single
// source for the translucent chip triad (bg + theme-aware `text-status-*` FG +
// border); the FG follows the F-135a status tokens so it stays legible in BOTH
// themes. Use `statusToneClasses(tone)` when the call site owns its own layout
// (keeps existing padding/size), or `<StatusBadge>` for a canonical small chip.

import type { ReactNode } from "react"

export type StatusTone = "success" | "warning" | "danger" | "info" | "neutral"

// bg + border stay translucent (theme-neutral over any background); only the FG
// is a theme-aware token. Opacities match the StatusChip token set.
const TONE: Record<StatusTone, string> = {
  success: "border-emerald-500/25 bg-emerald-500/15 text-status-success",
  warning: "border-amber-500/25 bg-amber-500/15 text-status-warning",
  danger: "border-red-500/25 bg-red-500/15 text-status-danger",
  info: "border-blue-500/20 bg-blue-500/15 text-status-info",
  neutral: "border-line bg-bg-inset text-ink-dim",
}

/** The translucent color triad (border + bg + theme-aware FG token) for a tone.
 *  For sites that compose their own layout (ternary chip strings, custom padding). */
export function statusToneClasses(tone: StatusTone): string {
  return TONE[tone]
}

/** A canonical small tone-colored chip. Pass `className` for layout-only tweaks
 *  (e.g. `mt-1`); the size/padding stay fixed so chips read consistently. */
export function StatusBadge({
  tone,
  children,
  className = "",
}: {
  tone: StatusTone
  children: ReactNode
  className?: string
}) {
  return (
    <span
      className={`inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 text-[11px] font-medium ${TONE[tone]} ${className}`}
    >
      {children}
    </span>
  )
}
