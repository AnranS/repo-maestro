import { Fragment, useEffect, useMemo, useRef, useState } from "react"
import { ChevronDown, Cpu, RefreshCw, Search } from "lucide-react"
import type { ModelInfo } from "../types"

interface Props {
  models: ModelInfo[]
  /** Current pinned model id; `null` means "let the active agent decide". */
  value: string | null
  onChange: (model: string | null) => void | Promise<void>
  onRefresh?: () => Promise<void>
}

/**
 * Agent model picker.
 *
 * Cursor accounts often have 100+ models (Anthropic + OpenAI + Grok +
 * variants), so the picker must:
 *   1. Be wide enough to show full ids like `claude-opus-4.7-1m-low`.
 *   2. Show id AND human label per row without horizontal truncation
 *      fighting (we stack them vertically).
 *   3. Provide a search box because scrolling 107 items is hostile.
 *   4. Show the currently-pinned model unmistakably.
 */
export function ModelPicker({ models, value, onChange, onRefresh }: Props) {
  const [open, setOpen] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [query, setQuery] = useState("")
  const containerRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  // Close on outside click (mousedown so it fires before option's click).
  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setOpen(false)
      }
    }
    document.addEventListener("mousedown", handler)
    return () => document.removeEventListener("mousedown", handler)
  }, [open])

  // Auto-focus the search box and reset query when the dropdown opens.
  useEffect(() => {
    if (open) {
      setQuery("")
      // Defer to next frame so the input exists in the DOM.
      requestAnimationFrame(() => searchRef.current?.focus())
    }
  }, [open])

  const label = value ?? "agent default"

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return models
    return models.filter((m) => {
      const id = m.id.toLowerCase()
      const lab = (m.label ?? "").toLowerCase()
      const alias = (m.aliases ?? []).join(" ").toLowerCase()
      return id.includes(q) || lab.includes(q) || alias.includes(q)
    })
  }, [models, query])

  const grouped = useMemo(() => {
    const map = new Map<string, ModelInfo[]>()
    for (const model of filtered) {
      const provider = model.provider ?? "cursor"
      map.set(provider, [...(map.get(provider) ?? []), model])
    }
    return Array.from(map.entries())
  }, [filtered])

  const showProviderGroups = useMemo(() => {
    return new Set(models.map((m) => m.provider ?? "cursor")).size > 1
  }, [models])

  const refresh = async () => {
    if (!onRefresh) return
    setRefreshing(true)
    try {
      await onRefresh()
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <div className="relative" ref={containerRef}>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="inline-flex items-center gap-1.5 px-2 py-1 rounded-md text-xs border border-line hover:bg-bg-hover transition-colors max-w-[16rem]"
        title={value ? `model: ${value}` : "model — using the agent's default"}
      >
        <Cpu size={11} className="text-ink-mute shrink-0" />
        <span className="text-ink-faint">model</span>
        {value && <span className="font-mono truncate">· {value}</span>}
        <ChevronDown size={11} className="text-ink-faint shrink-0" />
      </button>

      {open && (
        <div className="absolute z-dropdown left-0 bottom-full mb-1 w-[26rem] max-w-[90vw] rounded-lg bg-bg-panel border border-line shadow-overlay flex flex-col overflow-hidden">
          {/* search */}
          <div className="p-2 border-b border-line/60 shrink-0">
            <div className="relative">
              <Search
                size={11}
                className="absolute left-2 top-1/2 -translate-y-1/2 text-ink-faint pointer-events-none"
              />
              <input
                ref={searchRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={`filter ${models.length} models…`}
                className="w-full bg-bg-inset border border-line rounded pl-7 pr-2 py-1 text-xs font-mono focus:outline-none focus:border-accent"
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.preventDefault()
                    setOpen(false)
                  }
                }}
              />
            </div>
          </div>

          <div className="overflow-y-auto scrollbar-thin py-1" style={{ maxHeight: 320 }}>
            {/* "agent default" is always available, even when filter doesn't match */}
            {query.trim() === "" && (
              <>
                <ModelOption
                  id="agent default"
                  label="omit --model and let the active agent pick"
                  selected={value === null}
                  onClick={() => {
                    onChange(null)
                    setOpen(false)
                  }}
                />
                <div className="my-1 border-t border-line/60" />
              </>
            )}

            {models.length === 0 ? (
              <div className="px-3 py-2 text-xs text-ink-faint">
                no models cached — click ↻ below
              </div>
            ) : filtered.length === 0 ? (
              <div className="px-3 py-2 text-xs text-ink-faint">
                no matches for <span className="font-mono">{query}</span>
              </div>
            ) : (
              grouped.map(([provider, group]) => (
                <Fragment key={provider}>
                  {showProviderGroups && (
                    <div className="px-3 py-1 text-[10px] uppercase text-ink-faint bg-bg-inset/60">
                      {provider}
                    </div>
                  )}
                  {group.map((m) => (
                    <ModelOption
                      key={`${provider}:${m.id}`}
                      id={m.id}
                      label={m.label ?? undefined}
                      selected={value === m.id}
                      onClick={() => {
                        onChange(m.id)
                        setOpen(false)
                      }}
                    />
                  ))}
                </Fragment>
              ))
            )}
          </div>

          {onRefresh && (
            <div className="border-t border-line/60 shrink-0">
              <button
                type="button"
                onClick={refresh}
                disabled={refreshing}
                className="w-full px-3 py-1.5 flex items-center gap-2 text-xs text-ink-mute hover:text-ink hover:bg-bg-hover disabled:opacity-50"
              >
                <RefreshCw
                  size={10}
                  className={refreshing ? "animate-spin" : ""}
                />
                {refreshing ? "refreshing…" : "refresh model list"}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * One row in the dropdown. ID on top in monospaced font; the human label
 * (if present) stacks below in faint text. No horizontal truncation —
 * if a label is monstrously long, the row wraps. Hover shows the full
 * id via title for accessibility.
 */
function ModelOption({
  id,
  label,
  selected,
  onClick,
}: {
  id: string
  label?: string
  selected: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={id}
      className={`w-full text-left px-3 py-1.5 hover:bg-bg-hover flex items-start gap-2 ${
        selected ? "bg-blue-500/10" : ""
      }`}
    >
      <span
        className={`mt-0.5 text-[10px] shrink-0 ${
          selected ? "text-accent" : "text-ink-faint"
        }`}
      >
        {selected ? "●" : "○"}
      </span>
      <div className="flex-1 min-w-0">
        <div className="font-mono text-xs text-ink-dim break-all leading-snug">
          {id}
        </div>
        {label && label !== id && (
          <div className="text-[10px] text-ink-faint mt-0.5 break-words leading-snug">
            {label}
          </div>
        )}
      </div>
    </button>
  )
}
