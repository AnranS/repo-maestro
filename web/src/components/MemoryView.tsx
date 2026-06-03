import { useEffect, useState } from "react"
import { Brain, Search, X, Trash2, List, Network, Sparkles, ArrowUpRight } from "lucide-react"
import type { MemoryHit, MemoryItemDetail } from "../types"
import { api } from "../api"
import { t, useLang } from "../i18n"
import { MemoryGraphPanel } from "./MemoryGraphPanel"
import { StarMapPanel } from "./StarMapPanel"

/**
 * Recall-driven memory view. A search box queries maestro's TF-IDF index over
 * L1 facts / L2 decisions / replans and shows ranked hits; with no query it
 * shows the most recent learnings as a timeline. This surfaces the memory's
 * intelligence (relevance ranking) instead of presenting a file tree.
 */
export function MemoryView({ onJumpToRun }: { onJumpToRun?: (runId: string) => void }) {
  useLang()
  const [q, setQ] = useState("")
  const [hits, setHits] = useState<MemoryHit[]>([])
  const [loading, setLoading] = useState(true)
  const [selected, setSelected] = useState<MemoryHit | null>(null)
  const [refreshKey, setRefreshKey] = useState(0)
  const [agentUp, setAgentUp] = useState<boolean | null>(null)
  const [mode, setMode] = useState<"list" | "graph" | "starmap">("list")

  useEffect(() => {
    api.agentMemoryStatus().then((s) => setAgentUp(s.connected)).catch(() => setAgentUp(false))
  }, [])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    // Debounce typed queries; load recent immediately.
    const delay = q.trim() ? 200 : 0
    const timer = setTimeout(() => {
      api
        .memorySearch(q)
        .then((h) => {
          if (!cancelled) {
            setHits(h)
            setLoading(false)
          }
        })
        .catch(() => {
          if (!cancelled) setLoading(false)
        })
    }, delay)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [q, refreshKey])

  const searching = q.trim().length > 0
  const maxScore = Math.max(0.0001, ...hits.map((h) => h.score))

  return (
    <div className="flex-1 overflow-y-auto scrollbar-thin">
      <div className={`${mode === "list" ? "max-w-3xl" : "max-w-5xl"} mx-auto px-6 py-6`}>
        <h1 className="mb-1 flex items-center gap-2 text-base font-semibold text-ink">
          <Brain size={16} className="text-blue-300" /> {t("tab.memory")}
          {agentUp !== null && (
            <span
              className="ml-1 inline-flex items-center gap-1 rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] font-normal text-ink-faint"
              title={agentUp ? "agentmemory engine reachable on :3111" : "agentmemory not running on :3111"}
            >
              <span className={`h-1.5 w-1.5 rounded-full ${agentUp ? "bg-emerald-400" : "bg-ink-faint"}`} />
              agentmemory
            </span>
          )}
          <div className="ml-auto flex items-center rounded-lg border border-line bg-bg-inset p-0.5">
            <button
              onClick={() => setMode("list")}
              className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "list" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              <List size={12} /> {t("memory.mode.list")}
            </button>
            <button
              onClick={() => setMode("graph")}
              className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "graph" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              <Network size={12} /> {t("memory.mode.graph")}
            </button>
            <button
              onClick={() => setMode("starmap")}
              className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] ${mode === "starmap" ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              <Sparkles size={12} /> {t("memory.mode.starmap")}
            </button>
          </div>
        </h1>
        <p className="mb-4 text-xs text-ink-faint">
          {mode === "graph" ? t("memory.graph.subtitle") : mode === "starmap" ? t("starmap.subtitle") : t("memory.subtitle")}
        </p>

        {mode === "graph" ? (
          <MemoryGraphPanel />
        ) : mode === "starmap" ? (
          <StarMapPanel />
        ) : (
          <>
        <div className="relative mb-5">
          <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-ink-faint" />
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={t("memory.searchPlaceholder")}
            className="w-full rounded-lg border border-line bg-bg-inset pl-9 pr-3 py-2.5 text-sm focus:outline-none focus:border-blue-600"
          />
        </div>

        <div className="mb-2 text-[11px] uppercase tracking-wider text-ink-faint">
          {searching ? t("memory.results", { n: hits.length }) : t("memory.recent")}
        </div>

        {hits.length === 0 && !loading && (
          <div className="py-10 text-center text-sm text-ink-faint">{t("memory.empty")}</div>
        )}

        <div className="space-y-2">
          {hits.map((h) => (
            <MemoryCard
              key={h.id}
              hit={h}
              showScore={searching}
              maxScore={maxScore}
              onOpen={() => setSelected(h)}
            />
          ))}
        </div>
          </>
        )}
      </div>

      {selected && (
        <MemoryDetailModal
          hit={selected}
          onClose={() => setSelected(null)}
          onChanged={() => {
            setSelected(null)
            setRefreshKey((k) => k + 1)
          }}
          onJumpToRun={onJumpToRun}
        />
      )}
    </div>
  )
}

function MemoryDetailModal({
  hit,
  onClose,
  onChanged,
  onJumpToRun,
}: {
  hit: MemoryHit
  onClose: () => void
  onChanged: () => void
  onJumpToRun?: (runId: string) => void
}) {
  const [item, setItem] = useState<MemoryItemDetail | null>(null)
  const [draft, setDraft] = useState("")
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    api
      .memoryItem(hit.kind, hit.id)
      .then((it) => {
        if (!cancelled) {
          setItem(it)
          setDraft(it.content)
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [hit.kind, hit.id])

  // L1 fact ids are "<topic>/<name>".
  const factParts = () => {
    const i = hit.id.indexOf("/")
    return i < 0 ? null : { topic: hit.id.slice(0, i), name: hit.id.slice(i + 1) }
  }
  const save = async () => {
    const fp = factParts()
    if (!fp) return
    setBusy(true)
    try {
      await api.memorySave(fp.topic, fp.name, draft)
      onChanged()
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    const fp = factParts()
    if (!fp || !confirm(`Delete memory "${hit.title}"?`)) return
    setBusy(true)
    try {
      await api.memoryDelete(fp.topic, fp.name)
      onChanged()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-6"
      onClick={onClose}
    >
      <div
        className="flex max-h-[80vh] w-full max-w-2xl flex-col rounded-xl border border-line bg-bg-panel shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-start gap-3 border-b border-line px-4 py-3">
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold text-ink">{hit.title}</div>
            {item?.path && (
              <div className="mt-0.5 truncate font-mono text-[10px] text-ink-faint" title={item.path}>
                {item.path}
              </div>
            )}
            {(() => {
              const m = item?.content.match(/run_id:\s*(\S+)/)
              if (!m) return null
              return onJumpToRun ? (
                <button
                  onClick={() => onJumpToRun(m[1])}
                  className="mt-1 inline-flex items-center gap-1 rounded border border-blue-500/30 bg-blue-500/10 px-1.5 py-0.5 font-mono text-[10px] text-blue-300 hover:bg-blue-500/20"
                  title={t("memory.openRun")}
                >
                  <ArrowUpRight size={10} /> run {m[1]}
                </button>
              ) : (
                <div className="mt-1 inline-flex items-center gap-1 rounded bg-bg-inset px-1.5 py-0.5 font-mono text-[10px] text-ink-mute">
                  run {m[1]}
                </div>
              )
            })()}
          </div>
          {item?.editable && !editing && (
            <button
              onClick={() => setEditing(true)}
              className="rounded border border-line px-2 py-1 text-xs text-ink-dim hover:bg-bg-hover"
            >
              {t("memory.edit")}
            </button>
          )}
          {item?.editable && (
            <button
              onClick={remove}
              disabled={busy}
              className="rounded border border-red-500/30 bg-red-500/10 p-1 text-red-300 hover:bg-red-500/20 disabled:opacity-50"
              title={t("common.delete")}
            >
              <Trash2 size={13} />
            </button>
          )}
          <button onClick={onClose} className="rounded p-1 text-ink-faint hover:text-ink hover:bg-bg-hover">
            <X size={15} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-4 scrollbar-thin">
          {!item ? (
            <div className="text-sm text-ink-faint">loading…</div>
          ) : editing ? (
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              className="h-72 w-full resize-none rounded-md border border-line bg-bg-inset p-3 font-mono text-xs focus:outline-none focus:border-blue-600"
            />
          ) : (
            <pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-ink-dim">
              {item.content}
            </pre>
          )}
        </div>
        {item?.editable && editing && (
          <div className="flex justify-end gap-2 border-t border-line px-4 py-3">
            <button
              onClick={() => {
                setEditing(false)
                setDraft(item.content)
              }}
              className="rounded-md px-3 py-1.5 text-xs text-ink-dim hover:bg-bg-hover"
            >
              {t("common.cancel")}
            </button>
            <button
              onClick={save}
              disabled={busy}
              className="rounded-md bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-500 disabled:opacity-50"
            >
              {t("common.save")}
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

function MemoryCard({
  hit,
  showScore,
  maxScore,
  onOpen,
}: {
  hit: MemoryHit
  showScore: boolean
  maxScore: number
  onOpen: () => void
}) {
  const tone =
    hit.kind === "decision"
      ? "text-emerald-300 border-emerald-500/30 bg-emerald-500/10"
      : hit.kind === "fact"
        ? "text-blue-300 border-blue-500/30 bg-blue-500/10"
        : "text-amber-300 border-amber-500/30 bg-amber-500/10"
  const when = hit.updated_ms ? new Date(hit.updated_ms).toLocaleDateString() : ""
  return (
    <div
      onClick={onOpen}
      className="cursor-pointer rounded-lg border border-line bg-bg-panel p-3 transition-colors hover:border-line-soft"
    >
      <div className="mb-1 flex items-center gap-2">
        <span className={`rounded border px-1.5 py-0.5 text-[9px] uppercase tracking-wider ${tone}`}>
          {t(`memory.kind.${hit.kind}`)}
        </span>
        {hit.project && <span className="font-mono text-[10px] text-ink-mute">{hit.project}</span>}
        {when && <span className="ml-auto text-[10px] text-ink-faint">{when}</span>}
      </div>
      <div className="truncate text-sm text-ink">{hit.title}</div>
      {hit.excerpt && (
        <div className="mt-0.5 line-clamp-2 text-[11px] leading-relaxed text-ink-mute">
          {hit.excerpt}
        </div>
      )}
      {showScore && (
        <div className="mt-2 flex items-center gap-2">
          <div className="h-1 flex-1 overflow-hidden rounded-full bg-bg-inset">
            <div
              className="h-full rounded-full bg-blue-400"
              style={{ width: `${Math.round((hit.score / maxScore) * 100)}%` }}
            />
          </div>
          <span className="font-mono text-[9px] text-ink-faint">{hit.score.toFixed(2)}</span>
        </div>
      )}
    </div>
  )
}
