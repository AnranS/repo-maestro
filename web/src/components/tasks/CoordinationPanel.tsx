import { useEffect, useState } from "react"
import { ArrowRight, CheckCircle2, Circle, HelpCircle, Inbox, MessagesSquare } from "lucide-react"
import type { MailMessage } from "../../types"
import { api } from "../../api"
import { t } from "../../i18n"
import { DiscussionThread, isDiscussion } from "./DiscussionThread"

/**
 * Surfaces the agent-to-agent coordination mailbox in the run view: who told
 * whom what, and whether it's been resolved. The scheduler delivers these same
 * messages into recipient agents' prompts (see
 * docs/design/multi-agent-collaboration.md) — this makes that loop visible.
 *
 * `refreshKey` re-fetches when it changes (e.g. on run-state ticks) so messages
 * an agent posts mid-run show up without a manual reload. Renders nothing when
 * the mailbox is empty, to stay out of the way on solo runs.
 */
export function CoordinationPanel({ refreshKey }: { refreshKey?: string | number | null }) {
  const [messages, setMessages] = useState<MailMessage[]>([])
  const [localBump, setLocalBump] = useState(0)

  useEffect(() => {
    let cancelled = false
    api
      .mailbox()
      .then((m) => {
        if (!cancelled) setMessages(m)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [refreshKey, localBump])

  if (messages.length === 0) return null
  const open = messages.filter((m) => m.status === "open").length
  // Deliberation messages render as a chat conversation; everything else stays
  // the compact coordination row list.
  const discussion = messages.filter(isDiscussion)
  const coordination = messages.filter((m) => !isDiscussion(m))

  return (
    <section data-pane="coordination" className="bg-bg-panel border border-line rounded-xl">
      <div className="px-4 py-2.5 border-b border-line flex items-center justify-between">
        <span className="text-xs uppercase tracking-wider text-ink-faint flex items-center gap-1.5">
          {discussion.length > 0 ? <MessagesSquare size={12} /> : <Inbox size={12} />}
          {discussion.length > 0 ? t("discussion.title") : t("tasks.coordination")}
        </span>
        <span className="text-xs text-ink-faint">
          {t("tasks.coordinationCount", { open, total: messages.length })}
        </span>
      </div>
      {discussion.length > 0 && <DiscussionThread messages={discussion} />}
      {coordination.length > 0 && (
        <div className="divide-y divide-line/40 border-t border-line/40">
          {coordination.slice(0, 12).map((m) => (
            <MailRow key={m.id} message={m} onAnswered={() => setLocalBump((n) => n + 1)} />
          ))}
        </div>
      )}
    </section>
  )
}

function MailRow({ message, onAnswered }: { message: MailMessage; onAnswered: () => void }) {
  const resolved = message.status === "resolved"
  const askable = !!message.ask && !resolved
  return (
    <div className="px-4 py-2.5 flex items-start gap-3">
      <span className="mt-0.5 shrink-0" title={message.status}>
        {resolved ? (
          <CheckCircle2 size={13} className="text-emerald-400" />
        ) : message.ask ? (
          <HelpCircle size={13} className="text-blue-400" />
        ) : (
          <Circle size={13} className="text-amber-400" />
        )}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 text-xs">
          {message.blocking && !resolved && (
            <span className="shrink-0 rounded border border-red-500/40 bg-red-500/15 px-1 text-[9px] font-semibold uppercase tracking-wider text-red-300">
              blocking
            </span>
          )}
          <span className="font-mono text-ink-dim shrink-0">{message.from}</span>
          <ArrowRight size={11} className="text-ink-faint shrink-0" />
          <span className="font-mono text-ink-dim shrink-0">{message.to}</span>
          <span className="text-ink-faint">·</span>
          <span className="truncate text-ink">{message.subject}</span>
        </div>
        <div className="mt-0.5 text-[11px] text-ink-mute" title={message.body}>
          {message.body}
        </div>
        {askable && <AskButtons message={message} onAnswered={onAnswered} />}
        {resolved && message.answer && message.answer.length > 0 && (
          <div className="mt-1 text-[10px] text-emerald-300/90">
            ✓ {message.ask
              ? message.answer
                  .map((k) => message.ask!.options.find((o) => o.key === k)?.label ?? k)
                  .join(", ")
              : message.answer.join(", ")}
          </div>
        )}
      </div>
      {resolved && (
        <span className="shrink-0 self-center text-[10px] text-ink-faint">{t("tasks.resolved")}</span>
      )}
    </div>
  )
}

function AskButtons({ message, onAnswered }: { message: MailMessage; onAnswered: () => void }) {
  const ask = message.ask!
  const [picked, setPicked] = useState<string[]>(ask.default ?? [])
  const [busy, setBusy] = useState(false)

  const submit = async (keys: string[]) => {
    if (busy || keys.length === 0) return
    setBusy(true)
    try {
      await api.mailboxAnswer(message.id, keys)
      onAnswered()
    } finally {
      setBusy(false)
    }
  }

  if (!ask.multi_select) {
    // single-select: each option answers immediately
    return (
      <div className="mt-1.5 flex flex-wrap gap-1.5">
        {ask.options.map((o) => (
          <button
            key={o.key}
            disabled={busy}
            onClick={() => submit([o.key])}
            className={`rounded-md border px-2 py-1 text-[11px] disabled:opacity-50 ${
              ask.default?.includes(o.key)
                ? "border-blue-500/50 bg-blue-500/10 text-blue-200"
                : "border-line text-ink-dim hover:bg-bg-hover"
            }`}
          >
            {o.label}
          </button>
        ))}
      </div>
    )
  }

  // multi-select: toggle then submit
  const toggle = (key: string) =>
    setPicked((p) => (p.includes(key) ? p.filter((k) => k !== key) : [...p, key]))
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
      {ask.options.map((o) => (
        <button
          key={o.key}
          disabled={busy}
          onClick={() => toggle(o.key)}
          className={`rounded-md border px-2 py-1 text-[11px] disabled:opacity-50 ${
            picked.includes(o.key)
              ? "border-blue-500/50 bg-blue-500/15 text-blue-200"
              : "border-line text-ink-dim hover:bg-bg-hover"
          }`}
        >
          {picked.includes(o.key) ? "✓ " : ""}
          {o.label}
        </button>
      ))}
      <button
        disabled={busy || picked.length === 0}
        onClick={() => submit(picked)}
        className="rounded-md bg-blue-600 px-2.5 py-1 text-[11px] font-medium text-white hover:bg-blue-500 disabled:opacity-40"
      >
        {t("ask.submit")}
      </button>
    </div>
  )
}
