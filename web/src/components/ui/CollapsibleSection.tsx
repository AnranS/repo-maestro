// F-UI-003 Slice 2: a secondary Run-Inspector panel. The header (chevron + icon
// + title + an optional one-line summary shown while collapsed) lets the
// operator scan without expanding; the body renders only when open. Secondary
// panels are default-collapsed unless actionable (`defaultOpen`). One data
// responsibility, no card-in-card.

import { useState, type ReactNode } from "react"
import { ChevronDown, ChevronRight } from "lucide-react"

export function CollapsibleSection({
  title,
  icon,
  summary,
  defaultOpen = false,
  children,
}: {
  title: ReactNode
  icon?: ReactNode
  summary?: ReactNode
  defaultOpen?: boolean
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <section className="rounded-lg border border-line bg-bg-panel">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="focus-ring flex w-full items-center gap-2 px-4 py-2.5 text-left"
      >
        {open ? (
          <ChevronDown size={14} className="shrink-0 text-ink-faint" />
        ) : (
          <ChevronRight size={14} className="shrink-0 text-ink-faint" />
        )}
        {icon}
        <span className="text-xs uppercase tracking-wider text-ink-faint">{title}</span>
        {summary != null && (
          <span className="ml-auto truncate text-[11px] text-ink-faint">{summary}</span>
        )}
      </button>
      {open && <div className="border-t border-line">{children}</div>}
    </section>
  )
}
