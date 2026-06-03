import { useState } from "react"
import { ChevronRight } from "lucide-react"

/**
 * Building blocks for the Context tab's left sidebar:
 *
 *   <Section>   — the two top-level groups ("memory · l1 facts", "skills · playbooks")
 *   <Group>     — collapsible scope/topic header inside a section
 *   <Item>      — clickable leaf row for a single fact or skill
 *
 * Kept here because `ContextView` doesn't need to know what their internals
 * look like — they're an implementation detail of the sidebar.
 */

export function Section({
  icon,
  label,
  right,
  children,
}: {
  icon?: React.ReactNode
  label: string
  right?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div className="border-b border-line/60 py-2">
      <div className="px-3 py-1 flex items-center gap-1.5 text-[10px] uppercase tracking-wider text-ink-faint">
        {icon}
        <span>{label}</span>
        <span className="ml-auto">{right}</span>
      </div>
      <div>{children}</div>
    </div>
  )
}

export function Group({
  label,
  icon,
  children,
}: {
  label: string
  icon?: React.ReactNode
  children: React.ReactNode
}) {
  const [open, setOpen] = useState(true)
  return (
    <div className="mt-1">
      <button
        onClick={() => setOpen(!open)}
        className="w-full px-3 py-0.5 flex items-center gap-1 text-[11px] text-ink-dim hover:text-ink"
      >
        <ChevronRight
          size={10}
          className={`transition-transform ${open ? "rotate-90" : ""}`}
        />
        {icon}
        <span className="font-medium">{label}</span>
      </button>
      {open && <div>{children}</div>}
    </div>
  )
}

export function Item({
  label,
  sub,
  selected,
  onClick,
}: {
  label: string
  sub?: string
  selected: boolean
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full text-left pl-7 pr-3 py-1 text-xs transition-colors ${
        selected
          ? "bg-bg-hover text-ink"
          : "text-ink-dim hover:text-ink hover:bg-bg-hover/50"
      }`}
    >
      <div className="truncate font-mono">{label}</div>
      {sub && (
        <div className="truncate text-[10px] text-ink-faint mt-0.5 leading-tight">
          {sub}
        </div>
      )}
    </button>
  )
}
