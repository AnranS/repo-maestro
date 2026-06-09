import { useEffect, useMemo, useState } from "react"
import {
  Plus,
  Trash2,
  MessageSquare,
  Pencil,
  Check,
  X,
  Tag,
  Sparkles,
  History,
  Loader2,
  Cpu,
} from "lucide-react"
import type { ExternalListing, ExternalSession, SessionMeta } from "../types"
import { api } from "../api"

export function SessionSidebar(props: {
  sessions: SessionMeta[]
  currentId: string | null
  onOpen: (id: string) => void
  onNew: () => void
  onDelete: (id: string) => void
  onRename: (id: string, title: string) => void
  onReload: () => void
}) {
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editTitle, setEditTitle] = useState("")
  const [tagEditingId, setTagEditingId] = useState<string | null>(null)
  const [tagDraft, setTagDraft] = useState("")
  const [busyTagId, setBusyTagId] = useState<string | null>(null)
  const [filter, setFilter] = useState<Set<string>>(new Set())
  const [external, setExternal] = useState<ExternalListing | null>(null)
  const [externalOpen, setExternalOpen] = useState(false)

  useEffect(() => {
    api.externalSessions().then(setExternal).catch(() => {})
  }, [])

  const allTags = useMemo(() => {
    const set = new Set<string>()
    props.sessions.forEach((s) => s.tags.forEach((t) => set.add(t)))
    return Array.from(set).sort()
  }, [props.sessions])

  const filtered = useMemo(() => {
    if (filter.size === 0) return props.sessions
    return props.sessions.filter((s) =>
      Array.from(filter).every((t) => s.tags.includes(t)),
    )
  }, [props.sessions, filter])

  const toggleFilter = (t: string) =>
    setFilter((prev) => {
      const next = new Set(prev)
      if (next.has(t)) next.delete(t)
      else next.add(t)
      return next
    })

  const startEdit = (s: SessionMeta) => {
    setEditingId(s.id)
    setEditTitle(s.title)
  }

  const commitEdit = () => {
    if (editingId && editTitle.trim()) {
      props.onRename(editingId, editTitle.trim())
    }
    setEditingId(null)
  }

  const submitTags = async () => {
    if (!tagEditingId) return
    const cleaned = tagDraft
      .split(/[\s,]+/)
      .map((t) => t.trim().toLowerCase().replace(/[^a-z0-9-_]/g, ""))
      .filter((t) => t.length > 0 && t.length <= 32)
    try {
      await api.setSessionTags(tagEditingId, Array.from(new Set(cleaned)))
      props.onReload()
    } finally {
      setTagEditingId(null)
      setTagDraft("")
    }
  }

  const autoTag = async (id: string) => {
    setBusyTagId(id)
    try {
      await api.autoTagSession(id)
      props.onReload()
    } finally {
      setBusyTagId(null)
    }
  }

  return (
    <aside className="flex max-h-72 w-full shrink-0 flex-col border-b border-line bg-bg-soft min-h-0 md:max-h-none md:w-72 md:border-b-0 md:border-r">
      <div className="p-3">
        <button
          onClick={props.onNew}
          className="w-full flex items-center justify-center gap-2 py-2 px-3 rounded-lg text-sm font-medium border border-line hover:bg-bg-hover transition-colors"
        >
          <Plus size={14} /> New chat
        </button>
      </div>

      {allTags.length > 0 && (
        <div className="px-2 pb-2 border-b border-line/40">
          <div className="px-1 pb-1 text-[10px] uppercase tracking-wider text-ink-faint">
            tags · {filter.size > 0 ? `${filter.size} active` : "all"}
          </div>
          <div className="flex flex-wrap gap-1">
            {allTags.map((t) => (
              <button
                key={t}
                onClick={() => toggleFilter(t)}
                className={`px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors ${
                  filter.has(t)
                    ? "bg-blue-500/30 text-accent border border-blue-500/50"
                    : "bg-bg-inset text-ink-mute border border-line hover:text-ink"
                }`}
              >
                #{t}
              </button>
            ))}
            {filter.size > 0 && (
              <button
                onClick={() => setFilter(new Set())}
                className="text-[10px] text-ink-faint hover:text-ink px-1"
              >
                clear
              </button>
            )}
          </div>
        </div>
      )}

      <div className="px-2 py-1 text-[11px] uppercase tracking-wider text-ink-faint flex items-center">
        <span>sessions</span>
        {props.sessions.length > 0 && (
          <span
            className="ml-auto text-ink-faint normal-case"
            title={filter.size > 0 ? "matching / total" : undefined}
          >
            {filter.size > 0
              ? `${filtered.length}/${props.sessions.length}`
              : props.sessions.length}
          </span>
        )}
      </div>

      <nav className="flex-1 overflow-y-auto scrollbar-thin px-2 pb-3 min-h-0">
        {filtered.length === 0 ? (
          <p className="px-2 py-2 text-xs text-ink-faint">
            {filter.size > 0
              ? "No sessions match the filter."
              : 'No sessions yet — click "+ New chat" above to start one.'}
          </p>
        ) : (
          filtered.map((s) => {
            const isCurrent = s.id === props.currentId
            const isEditing = s.id === editingId
            const isTagEditing = s.id === tagEditingId
            return (
              <div
                key={s.id}
                onClick={() => !isEditing && !isTagEditing && props.onOpen(s.id)}
                className={`group rounded-md px-2 py-1.5 mb-0.5 cursor-pointer text-sm transition-colors ${
                  isCurrent ? "bg-bg-hover" : "hover:bg-bg-hover/50"
                }`}
              >
                <div className="flex items-center gap-1.5">
                  <MessageSquare
                    size={12}
                    className={isCurrent ? "text-blue-400" : "text-ink-faint"}
                  />
                  {isEditing ? (
                    <input
                      value={editTitle}
                      onChange={(e) => setEditTitle(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitEdit()
                        if (e.key === "Escape") setEditingId(null)
                      }}
                      onBlur={commitEdit}
                      autoFocus
                      className="flex-1 bg-bg-inset border border-line rounded px-1.5 py-0.5 text-xs outline-none focus:border-accent"
                    />
                  ) : (
                    <span
                      className={`flex-1 truncate ${isCurrent ? "text-ink" : "text-ink-dim"}`}
                    >
                      {s.title}
                    </span>
                  )}

                  {!isEditing && !isTagEditing && (
                    <span className="opacity-0 group-hover:opacity-100 flex items-center gap-1">
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          autoTag(s.id)
                        }}
                        disabled={busyTagId === s.id}
                        className="p-0.5 text-purple-300/80 hover:text-purple-200 disabled:opacity-50"
                        title="auto-tag (AI, user messages only)"
                      >
                        {busyTagId === s.id ? (
                          <Loader2 size={10} className="animate-spin" />
                        ) : (
                          <Sparkles size={10} />
                        )}
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          setTagEditingId(s.id)
                          setTagDraft(s.tags.join(" "))
                        }}
                        className="p-0.5 text-ink-faint hover:text-ink"
                        title="edit tags"
                      >
                        <Tag size={10} />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          startEdit(s)
                        }}
                        className="p-0.5 text-ink-faint hover:text-ink"
                        title="rename"
                      >
                        <Pencil size={10} />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          if (confirm(`Delete "${s.title}"?`)) props.onDelete(s.id)
                        }}
                        className="p-0.5 text-ink-faint hover:text-red-400"
                        title="delete"
                      >
                        <Trash2 size={10} />
                      </button>
                    </span>
                  )}

                  {isEditing && (
                    <span className="flex items-center gap-0.5">
                      <button
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={commitEdit}
                        className="p-0.5 text-status-success hover:opacity-80"
                      >
                        <Check size={10} />
                      </button>
                      <button
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => setEditingId(null)}
                        className="p-0.5 text-ink-faint hover:text-ink"
                      >
                        <X size={10} />
                      </button>
                    </span>
                  )}
                </div>

                {isTagEditing ? (
                  <div className="ml-[18px] mt-1 flex items-center gap-1">
                    <input
                      value={tagDraft}
                      onChange={(e) => setTagDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") submitTags()
                        if (e.key === "Escape") setTagEditingId(null)
                      }}
                      onBlur={submitTags}
                      placeholder="space or comma separated"
                      autoFocus
                      className="flex-1 bg-bg-inset border border-line rounded px-1.5 py-0.5 text-[11px] font-mono outline-none focus:border-accent"
                    />
                    <button
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={submitTags}
                      className="p-0.5 text-emerald-400"
                    >
                      <Check size={10} />
                    </button>
                  </div>
                ) : s.tags.length > 0 ? (
                  <div className="ml-[18px] mt-1 flex flex-wrap gap-1">
                    {s.tags.map((t) => (
                      <span
                        key={t}
                        className="text-[9px] font-mono px-1 rounded bg-bg-inset text-ink-mute border border-line/60"
                      >
                        #{t}
                      </span>
                    ))}
                  </div>
                ) : null}

                <div className="ml-[18px] mt-0.5 text-[10px] text-ink-faint">
                  {s.message_count} msg ·{" "}
                  {new Date(s.updated_at).toLocaleString(undefined, {
                    month: "short",
                    day: "numeric",
                    hour: "2-digit",
                    minute: "2-digit",
                  })}
                </div>
              </div>
            )
          })
        )}
      </nav>

      {/* External history — read-only, collapsed by default */}
      {external && external.summaries.length > 0 && (
        <div className="border-t border-line/60 shrink-0 max-h-[40vh] overflow-y-auto scrollbar-thin">
          <button
            onClick={() => setExternalOpen(!externalOpen)}
            title={`External agent sessions on disk (read-only): ${external.summaries
              .map((s) => `${s.session_count} from ${s.source}`)
              .join(", ")}`}
            className="w-full flex items-center gap-2 px-3 py-2 text-[10px] uppercase tracking-wider text-ink-faint hover:text-ink-dim"
          >
            <History size={11} />
            <span>external history</span>
            <span className="ml-auto text-ink-faint normal-case">
              {external.summaries
                .map((s) => `${s.session_count} ${s.source}`)
                .join(" · ")}
            </span>
          </button>
          {externalOpen && (
            <div className="px-2 pb-2">
              {external.sessions.slice(0, 30).map((es) => (
                <ExternalRow key={es.path} session={es} />
              ))}
              {external.sessions.length > 30 && (
                <div className="px-2 py-1 text-[10px] text-ink-faint">
                  +{external.sessions.length - 30} more (read-only)
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </aside>
  )
}

function ExternalRow({ session }: { session: ExternalSession }) {
  const when = session.modified_at
    ? new Date(session.modified_at).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      })
    : "—"
  return (
    <div
      className="px-2 py-1 rounded hover:bg-bg-hover/40 cursor-default"
      title={session.path}
    >
      <div className="flex items-center gap-1.5 text-[11px]">
        <Cpu size={9} className="text-ink-faint" />
        <span className="font-medium text-ink-dim w-12 truncate">{session.source}</span>
        <span className="font-mono text-[10px] text-ink-mute truncate flex-1">
          {session.id.slice(0, 16)}
        </span>
      </div>
      <div className="ml-[20px] text-[9.5px] text-ink-faint truncate">
        ws {session.workspace_id.slice(0, 20)} · {when}
      </div>
    </div>
  )
}
