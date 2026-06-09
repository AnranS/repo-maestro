// F-UI-003 Slice 1: shared empty / loading / error states so every surface
// speaks the same calm language. Empty and 404 are calm (ink-faint), never red;
// an error is a soft neutral "unavailable" line that NEVER echoes a raw
// error/path/body; loading is quiet. These replace the ad-hoc per-component
// `<div className="text-ink-faint">loading…</div>` patterns.

import type { ReactNode } from "react"
import { Loader2 } from "lucide-react"

export function LoadingState({ label = "loading…" }: { label?: string }) {
  return (
    <div className="inline-flex items-center gap-1.5 text-[11px] text-ink-faint">
      <Loader2 size={11} className="animate-spin-slow" />
      {label}
    </div>
  )
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <div className="text-[11px] text-ink-faint">{children}</div>
}

/** A soft fault: an unavailable read is amber (not a hard red error), and the
 *  label is fixed copy — it must never carry the raw error / path / body. */
export function ErrorState({ label = "unavailable" }: { label?: string }) {
  return <div className="text-[11px] text-status-warning/80">{label}</div>
}
