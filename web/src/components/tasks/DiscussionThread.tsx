import { Crown } from "lucide-react"
import type { MailMessage } from "../../types"
import { t } from "../../i18n"

/** A deliberation message — agents stating positions + the lead synthesis —
 *  identified by its subject prefix. */
export function isDiscussion(m: MailMessage): boolean {
  const s = m.subject || ""
  return s.startsWith("[discuss") || s.startsWith("[deliberation")
}

// Stable per-speaker avatar color so each "person" reads consistently.
const PALETTE = [
  "bg-sky-500/20 text-sky-200 border-sky-500/30",
  "bg-violet-500/20 text-violet-200 border-violet-500/30",
  "bg-emerald-500/20 text-emerald-200 border-emerald-500/30",
  "bg-rose-500/20 text-rose-200 border-rose-500/30",
  "bg-cyan-500/20 text-cyan-200 border-cyan-500/30",
  "bg-orange-500/20 text-orange-200 border-orange-500/30",
]
function colorFor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return PALETTE[h % PALETTE.length]
}

/** Display name from the sender id (`web-app-agent` → `web-app`). */
function speaker(from: string): string {
  return from.replace(/-agent$/, "")
}
function initials(name: string): string {
  const parts = name.split(/[-_\s]/).filter(Boolean)
  return ((parts[0]?.[0] ?? "") + (parts[1]?.[0] ?? parts[0]?.[1] ?? "")).toUpperCase()
}
/** Role tag parsed from the subject: "... position (top-level)" or "...: role". */
function roleOf(subject: string): string | null {
  const paren = subject.match(/\(([^)]+)\)\s*$/)
  if (paren) return paren[1]
  const colon = subject.match(/position:\s*(\S+)/)
  return colon ? colon[1] : null
}
function fmtTime(iso: string): string {
  const d = new Date(iso)
  return isNaN(d.getTime())
    ? ""
    : d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
}

// The local agent backend that produced a take (carried on the message's
// `task` field by discuss mode), so the avatar shows codex / claude / cursor.
const BACKEND: Record<string, { label: string; dot: string; chip: string }> = {
  claude: { label: "claude", dot: "bg-orange-400", chip: "border-orange-500/40 bg-orange-500/15 text-orange-200" },
  codex: { label: "codex", dot: "bg-teal-400", chip: "border-teal-500/40 bg-teal-500/15 text-teal-200" },
  cursor: { label: "cursor", dot: "bg-indigo-400", chip: "border-indigo-500/40 bg-indigo-500/15 text-indigo-200" },
}
function backendOf(m: MailMessage) {
  return BACKEND[(m.task || "").toLowerCase()] ?? null
}

/**
 * Renders the deliberation as a chat conversation — one avatar per project
 * agent, message bubbles in time order, and the lead's synthesis as a
 * highlighted facilitator turn. Reads much more like people talking than the
 * flat mailbox list.
 */
export function DiscussionThread({ messages }: { messages: MailMessage[] }) {
  if (messages.length === 0) return null
  // Mailbox is newest-first; a conversation reads oldest-first.
  const ordered = [...messages].reverse()

  return (
    <div className="space-y-3 p-4">
      {ordered.map((m) => {
        const name = speaker(m.from)
        const isLead = name === "lead"
        const role = roleOf(m.subject || "")
        const be = backendOf(m)
        if (isLead) {
          return (
            <div
              key={m.id}
              className="rounded-lg border border-amber-500/30 bg-amber-500/[0.07] p-3"
            >
              <div className="mb-1 flex items-center gap-1.5 text-[12px]">
                <Crown size={13} className="text-amber-300" />
                <span className="font-semibold text-amber-200">lead</span>
                <span className="text-ink-faint">·</span>
                <span className="text-ink-faint">{t("discussion.synthesis")}</span>
                {be && (
                  <span className={`rounded border px-1 py-px text-[9px] ${be.chip}`}>{be.label}</span>
                )}
                <span className="ml-auto text-[10px] text-ink-faint">{fmtTime(m.created_at)}</span>
              </div>
              <p className="whitespace-pre-wrap text-[12px] leading-relaxed text-ink-dim">
                {m.body}
              </p>
            </div>
          )
        }
        return (
          <div key={m.id} className="flex items-start gap-2.5">
            <span className="relative mt-0.5 shrink-0" title={be ? `${name} · ${be.label}` : name}>
              <span
                className={`flex h-7 w-7 items-center justify-center rounded-full border text-[10px] font-semibold ${colorFor(name)}`}
              >
                {initials(name)}
              </span>
              {be && (
                <span
                  className={`absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border border-bg-panel ${be.dot}`}
                />
              )}
            </span>
            <div className="min-w-0 flex-1">
              <div className="mb-0.5 flex items-center gap-1.5 text-[12px]">
                <span className="font-medium text-ink">{name}</span>
                {role && (
                  <span className="rounded border border-line bg-bg-inset px-1 py-px text-[9px] text-ink-faint">
                    {role}
                  </span>
                )}
                {be && (
                  <span className={`rounded border px-1 py-px text-[9px] ${be.chip}`}>{be.label}</span>
                )}
                <span className="ml-auto text-[10px] text-ink-faint">{fmtTime(m.created_at)}</span>
              </div>
              <div className="rounded-lg rounded-tl-sm border border-line bg-bg-inset px-3 py-2">
                <p className="whitespace-pre-wrap text-[12px] leading-relaxed text-ink-dim">
                  {m.body}
                </p>
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}
