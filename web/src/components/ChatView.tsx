import { useCallback, useEffect, useRef, useState } from "react"
import { ChevronDown, Server } from "lucide-react"
import type { Action, Message, Session } from "../types"
import { api, streamMessage } from "../api"
import { MessageBubble } from "./MessageBubble"
import { ChatInput } from "./ChatInput"
import { ThinkingIndicator } from "./ThinkingIndicator"

const CHAT_PROVIDERS = ["cursor", "codex", "claude"] as const

interface Props {
  session: Session | null
  onSessionUpdated: () => Promise<void>
  onCreateIfMissing: () => Promise<Session | null>
}

export function ChatView({ session, onSessionUpdated, onCreateIfMissing }: Props) {
  const [draftReply, setDraftReply] = useState<string>("")
  const [streaming, setStreaming] = useState(false)
  const [streamingId, setStreamingId] = useState<string | null>(null)
  /**
   * Accumulated reasoning-mode tokens for the current turn (Claude
   * `-thinking-*` etc.). Reset every send. Rendered separately from the
   * answer so the user gets a "model is thinking" affordance instead of
   * staring at an empty bubble.
   */
  const [thinkingTrace, setThinkingTrace] = useState<string>("")
  /** When the current turn started — used to display elapsed seconds. */
  const [streamStartedAt, setStreamStartedAt] = useState<number | null>(null)
  /**
   * Optimistic user message for the in-flight turn. Rendered alongside the
   * persisted session messages so the user's text shows immediately, then
   * cleared once the refreshed session carries the real (persisted) copy.
   */
  const [pendingUser, setPendingUser] = useState<Message | null>(null)
  const abortRef = useRef<(() => void) | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const messages = session?.messages ?? []
  const currentProvider = session?.chat_provider ?? null
  // The model isn't user-settable from this view (each agent CLI owns its own
  // model config), but the session struct may still carry one from an older
  // run or a CLI-side pin — honour it when present.
  const currentModel = session?.cursor_model ?? null

  const setProvider = async (provider: string | null) => {
    if (provider === currentProvider) return
    let target = session
    if (!target) {
      target = await onCreateIfMissing()
      if (!target) return
    }
    await api.setSessionProvider(target.id, provider)
    // A previously-pinned model may not even exist under the new provider;
    // drop it so it doesn't silently shadow the new agent's default.
    if (target.cursor_model) {
      await api.setSessionModel(target.id, null)
    }
    await onSessionUpdated()
  }

  // Auto-scroll on new content
  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    })
  }, [messages.length, draftReply])

  const send = useCallback(
    async (
      text: string,
      oneShotModel?: string | null,
      mode?: "plan" | "exec",
    ) => {
      if (!text.trim() || streaming) return

      let s = session
      if (!s) {
        s = await onCreateIfMissing()
        if (!s) return
      }

      // Slash commands intercept BEFORE we hit the streaming endpoint —
      // they're operations on the session, not messages to the agent.
      const slash = text.trim().toLowerCase()
      if (slash === "/compact") {
        const ok = confirm(
          "Compact this session?\n\nOlder messages will be folded into a single summary. The next message starts a fresh provider chat. Recent messages are preserved.",
        )
        if (!ok) return
        try {
          await api.compactSession(s.id)
          await onSessionUpdated()
        } catch (e) {
          console.error("compact failed:", e)
        }
        return
      }

      setStreaming(true)
      setDraftReply("")
      setStreamingId(null)
      setThinkingTrace("")
      setStreamStartedAt(Date.now())

      const optimisticUser: Message = {
        id: `temp-${Date.now()}`,
        role: "user",
        content: text,
        timestamp: new Date().toISOString(),
      }
      setPendingUser(optimisticUser)

      const effectiveModel =
        oneShotModel !== undefined && oneShotModel !== null
          ? oneShotModel
          : currentModel

      abortRef.current = streamMessage(
        text,
        s.id,
        {
          onMeta: ({ message_id }) => setStreamingId(message_id),
          onDelta: (chunk) => setDraftReply((prev) => prev + chunk),
          onThinking: (chunk) => setThinkingTrace((prev) => prev + chunk),
          onDone: async () => {
            await onSessionUpdated()
            setStreaming(false)
            setDraftReply("")
            setStreamingId(null)
            setThinkingTrace("")
            setStreamStartedAt(null)
            setPendingUser(null)
          },
          onError: async (err) => {
            console.error("chat stream error:", err)
            await onSessionUpdated()
            setStreaming(false)
            setDraftReply("")
            setStreamingId(null)
            setThinkingTrace("")
            setStreamStartedAt(null)
            setPendingUser(null)
          },
        },
        effectiveModel,
        mode,
        currentProvider,
      )
    },
    [
      session,
      streaming,
      messages,
      onCreateIfMissing,
      onSessionUpdated,
      currentModel,
      currentProvider,
    ],
  )

  const stopStream = useCallback(() => {
    abortRef.current?.()
    setStreaming(false)
  }, [])

  const handleAction = useCallback(
    async (action: Action, decision: "approve" | "reject") => {
      if (!session) return
      try {
        await api.runAction(session.id, action.id, decision)
      } catch (e) {
        console.error(e)
      }
      await onSessionUpdated()
    },
    [session, onSessionUpdated],
  )

  return (
    <div className="flex-1 flex flex-col min-w-0 bg-bg">
      <header className="px-6 py-3 border-b border-line flex items-center gap-3">
        <h1 className="text-base font-semibold truncate flex-1">
          {session?.title ?? "Orchestrator"}
        </h1>
        {session?.cursor_chat_id && (
          <span className="text-[11px] font-mono text-ink-faint">
            cursor session · {session.cursor_chat_id.slice(0, 8)}…
          </span>
        )}
        {currentProvider && (
          <span
            className="text-[11px] font-mono text-ink-faint"
            title={`Provider for this session: ${currentProvider}`}
          >
            provider · {currentProvider}
          </span>
        )}
      </header>

      <div ref={scrollRef} className="flex-1 overflow-y-auto scrollbar-thin">
        <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
          {messages.length === 0 && !streaming && (
            <EmptyState onSuggest={(t) => send(t)} />
          )}

          {[...messages, pendingUser]
            .filter((m): m is Message => Boolean(m))
            .map((m) => (
              <MessageBubble
                key={m.id}
                message={m}
                onAction={handleAction}
              />
            ))}

          {streaming && (
            <div className="space-y-2">
              <ThinkingIndicator
                streaming={streaming}
                startedAt={streamStartedAt}
                thinkingText={thinkingTrace}
                draftText={draftReply}
                provider={currentProvider}
              />
              <MessageBubble
                message={{
                  id: streamingId ?? "streaming",
                  role: "assistant",
                  content: draftReply,
                  timestamp: new Date().toISOString(),
                }}
                streaming
              />
            </div>
          )}
        </div>
      </div>

      <div className="border-t border-line bg-bg">
        <div className="max-w-3xl mx-auto p-4">
          <div className="flex items-center gap-2 mb-2 px-1">
            <ProviderPicker value={currentProvider} onChange={setProvider} />
            {/* The model is whatever the picked agent's own CLI is configured
                to use (cursor-agent / codex / claude all have their own model
                setting); we don't second-guess that here. */}
            <span className="text-[11px] text-ink-faint">
              model · set in <span className="font-mono">{currentProvider ?? "the agent"}</span>'s CLI
            </span>
          </div>
          <ChatInput
            onSubmit={send}
            disabled={streaming}
            onStop={stopStream}
            streaming={streaming}
          />
          <p className="mt-2 text-[11px] text-ink-faint text-center">
            Provider pin is saved on this session.
            Enter to send · Shift+Enter for newline.
          </p>
        </div>
      </div>
    </div>
  )
}

/**
 * Custom provider dropdown that mirrors ModelPicker's button + popup styling
 * (same chrome, same selected-dot rows, opens upward) so the two controls in
 * the composer bar read as one consistent set instead of a native <select>
 * sitting next to a bespoke menu.
 */
function ProviderPicker({
  value,
  onChange,
}: {
  value: string | null
  onChange: (provider: string | null) => void | Promise<void>
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener("mousedown", handler)
    return () => document.removeEventListener("mousedown", handler)
  }, [open])

  const options: { id: string | null; label: string }[] = [
    { id: null, label: "default" },
    ...CHAT_PROVIDERS.map((provider) => ({ id: provider, label: provider })),
  ]

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="inline-flex items-center gap-1.5 px-2 py-1 rounded-md text-xs border border-line hover:bg-bg-hover transition-colors"
        title={value ? `chat provider: ${value}` : "chat provider — using the workspace default"}
      >
        <Server size={11} className="text-ink-mute shrink-0" />
        <span className="text-ink-faint">provider</span>
        {value && <span className="font-mono">· {value}</span>}
        <ChevronDown size={11} className="text-ink-faint shrink-0" />
      </button>

      {open && (
        <div className="absolute z-20 left-0 bottom-full mb-1 w-44 rounded-lg bg-bg-panel border border-line shadow-xl overflow-hidden py-1">
          {options.map((opt) => {
            const selected = (opt.id ?? null) === (value ?? null)
            return (
              <button
                key={opt.id ?? "__default__"}
                type="button"
                onClick={() => {
                  onChange(opt.id)
                  setOpen(false)
                }}
                className={`w-full text-left px-3 py-1.5 hover:bg-bg-hover flex items-center gap-2 text-xs ${
                  selected ? "bg-blue-500/10" : ""
                }`}
              >
                <span className={`text-[10px] shrink-0 ${selected ? "text-blue-300" : "text-ink-faint"}`}>
                  {selected ? "●" : "○"}
                </span>
                <span className="font-mono text-ink-dim">{opt.label}</span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

function EmptyState({ onSuggest }: { onSuggest: (text: string) => void }) {
  const suggestions = [
    "Walk me through the projects and how they depend on each other",
    "Summarize the most recent run and tell me whether anything failed",
    "Which projects have the largest blast radius (most downstream consumers)?",
    "What approval gates are still pending?",
  ]
  return (
    <div className="pt-10 pb-6 text-center">
      <div className="inline-flex items-center justify-center w-12 h-12 rounded-2xl bg-gradient-to-br from-blue-500/20 to-purple-500/20 border border-blue-500/30 mb-4">
        <span className="text-xl">🪢</span>
      </div>
      <h2 className="text-xl font-semibold mb-1">How can I orchestrate today?</h2>
      <p className="text-sm text-ink-mute">
        I have visibility into all registered projects, your memory facts, and the active run.
      </p>
      <div className="mt-6 grid grid-cols-1 sm:grid-cols-2 gap-2 text-left">
        {suggestions.map((s) => (
          <button
            key={s}
            onClick={() => onSuggest(s)}
            className="px-3 py-2.5 rounded-lg border border-line text-sm text-ink-dim hover:text-ink hover:border-line-soft hover:bg-bg-hover transition-colors"
          >
            {s}
          </button>
        ))}
      </div>
    </div>
  )
}
