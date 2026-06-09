import { useCallback, useEffect, useRef, useState } from "react"
import {
  ChevronDown,
  Compass,
  PanelRightClose,
  PanelRightOpen,
  Server,
} from "lucide-react"
import type { Action, Message, RunState, Session } from "../types"
import { api, streamMessage } from "../api"
import { MessageBubble } from "./MessageBubble"
import { ChatInput } from "./ChatInput"
import { ThinkingIndicator } from "./ThinkingIndicator"
import { StatusChip } from "./ui/StatusChip"
import {
  ChatActivityPane,
  activityCount,
  linkedRunFor,
} from "./ChatActivityPane"

const CHAT_PROVIDERS = ["cursor", "codex", "claude"] as const

interface Props {
  session: Session | null
  onSessionUpdated: () => Promise<void>
  onCreateIfMissing: () => Promise<Session | null>
  /** The live run, for an explicit session→run Activity link (F-UI-002 Slice 2). */
  liveRun?: RunState | null
  onOpenRun?: (runId: string) => void
  /** Onboarding launcher to the Dashboard (F-UI-002 Slice 4). */
  onOpenDashboard?: () => void
}

export function ChatView({
  session,
  onSessionUpdated,
  onCreateIfMissing,
  liveRun,
  onOpenRun,
  onOpenDashboard,
}: Props) {
  const [activityOpen, setActivityOpen] = useState(false)
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

  // F-UI-002 Slice 3: one place to drop ALL transient turn state (and the stale
  // abort closure). Called on stream done/error/stop and on session switch, so a
  // completed/stopped turn never leaves a half-pending reply or an old abort ref.
  const clearTransientTurnState = useCallback(() => {
    setStreaming(false)
    setStreamingId(null)
    setDraftReply("")
    setThinkingTrace("")
    setStreamStartedAt(null)
    setPendingUser(null)
    abortRef.current = null
  }, [])

  // Switching session aborts any in-flight stream and clears all transient turn
  // state, so one conversation never bleeds into another.
  useEffect(() => {
    abortRef.current?.()
    clearTransientTurnState()
  }, [session?.id, clearTransientTurnState])

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

      // F-119: one idempotency key per submission, so an accidental double-submit
      // (or a network re-delivery) is deduped server-side rather than appending the
      // user message + first-turn prelude twice.
      const turnId = crypto.randomUUID()

      abortRef.current = streamMessage(
        text,
        s.id,
        {
          onMeta: ({ message_id }) => setStreamingId(message_id),
          onDelta: (chunk) => setDraftReply((prev) => prev + chunk),
          onThinking: (chunk) => setThinkingTrace((prev) => prev + chunk),
          onDone: async () => {
            await onSessionUpdated()
            clearTransientTurnState()
          },
          onError: async (err) => {
            console.error("chat stream error:", err)
            await onSessionUpdated()
            clearTransientTurnState()
          },
        },
        effectiveModel,
        mode,
        currentProvider,
        turnId,
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
      clearTransientTurnState,
    ],
  )

  const stopStream = useCallback(() => {
    abortRef.current?.()
    clearTransientTurnState()
  }, [clearTransientTurnState])

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

  const linkedRun = linkedRunFor(session, liveRun)
  const inFlight = { streaming, thinking: thinkingTrace.length > 0 }
  const actCount = activityCount(session, linkedRun, inFlight)

  return (
    <div className="flex-1 flex flex-col min-w-0 bg-bg">
      {/* F-UI-002 Slice 1: the Chat status band — identity + engine + a single
          shared-token runtime state. Streaming/loading is reflected here (and in
          the message group), never as a floating overlay. */}
      <header className="flex min-w-0 items-center gap-3 border-b border-line px-6 py-3">
        {/* identity — shrinks + truncates so it can never push out the status */}
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <h1 className="min-w-0 truncate text-base font-semibold">
            {session?.title ?? "Orchestrator"}
          </h1>
          {session?.id && (
            <span
              className="shrink-0 font-mono text-[11px] text-ink-faint"
              title={`session · ${session.id}`}
            >
              {session.id.slice(0, 8)}
            </span>
          )}
        </div>
        {/* engine + runtime state — never shrinks; the status chip always stays */}
        <div className="flex shrink-0 items-center gap-2">
          {currentProvider && (
            <span
              className="rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] font-mono text-ink-faint"
              title={`provider · ${currentProvider} (model is set in the agent's CLI)`}
            >
              {currentProvider}
            </span>
          )}
          {session?.cursor_chat_id && (
            <span
              className="hidden font-mono text-[10px] text-ink-faint sm:inline"
              title={`cursor session · ${session.cursor_chat_id}`}
            >
              cursor·{session.cursor_chat_id.slice(0, 6)}
            </span>
          )}
          <StatusChip status={streaming ? "streaming" : "idle"} />
          {/* Activity toggle — visible at all widths; on narrow screens the pane
              opens as a sheet (md:hidden), never a hard third column. */}
          <button
            onClick={() => setActivityOpen((o) => !o)}
            title="toggle activity"
            aria-label="toggle activity"
            className="inline-flex items-center gap-1 rounded border border-line bg-bg-inset px-1.5 py-0.5 text-[10px] text-ink-faint hover:text-ink-dim"
          >
            {activityOpen ? (
              <PanelRightClose size={12} />
            ) : (
              <PanelRightOpen size={12} />
            )}
            Activity{actCount > 0 ? ` ${actCount}` : ""}
          </button>
        </div>
      </header>

      {/* body row: center stream+composer | collapsible right Activity pane.
          The center owns scrolling; the Activity pane never squeezes it. */}
      <div className="flex min-h-0 flex-1">
      <div className="flex min-w-0 flex-1 flex-col">
      <div ref={scrollRef} className="flex-1 overflow-y-auto scrollbar-thin">
        <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
          {/* F-UI-002 Slice 4: executable work shows a run/task LINK card (explicit
              session→run link only) — id + status + open; no parallel status. */}
          {linkedRun && onOpenRun && (
            <RunLinkCard run={linkedRun} onOpen={onOpenRun} />
          )}
          {messages.length === 0 && !streaming && (
            <EmptyState
              onSuggest={(t) => send(t)}
              onOpenDashboard={onOpenDashboard}
            />
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

      {/* Sticky composer: pinned at the bottom of the center column (shrink-0 so
          it never collapses); control positions stay fixed across streaming. */}
      <div className="shrink-0 border-t border-line bg-bg">
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
            key={session?.id ?? "none"}
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
      <ChatActivityPane
        session={session}
        linkedRun={linkedRun}
        onOpenRun={onOpenRun}
        inFlight={inFlight}
        open={activityOpen}
        onClose={() => setActivityOpen(false)}
      />
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
        <div className="absolute z-dropdown left-0 bottom-full mb-1 w-44 rounded-lg bg-bg-panel border border-line shadow-overlay overflow-hidden py-1">
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
                <span className={`text-[10px] shrink-0 ${selected ? "text-accent" : "text-ink-faint"}`}>
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

/** F-UI-002 Slice 4: a run/task LINK card for a run explicitly linked to this
 *  session — short id + status chip + open. Chat links the task system, it does
 *  not re-implement run status; no raw body/path here. */
function RunLinkCard({
  run,
  onOpen,
}: {
  run: RunState
  // Required: the card is only rendered when the run can be opened (no dead-click).
  onOpen: (runId: string) => void
}) {
  const done = Object.values(run.tasks).filter((t) => t.status === "done").length
  const total = Object.keys(run.tasks).length
  return (
    <button
      type="button"
      onClick={() => onOpen(run.run_id)}
      className="flex w-full items-center gap-2 rounded-lg border border-line bg-bg-panel px-3 py-2 text-left hover:bg-bg-hover/40"
    >
      <StatusChip status={run.status} />
      <span className="text-[12px] text-ink-mute">linked run</span>
      <span className="flex-1 truncate font-mono text-[12px] text-ink-dim">
        {run.run_id.slice(0, 16)}
      </span>
      <span className="shrink-0 font-mono tabular-nums text-[11px] text-ink-faint">
        {done}/{total}
      </span>
      <span className="shrink-0 text-[11px] text-ink-faint">open →</span>
    </button>
  )
}

/** An onboarding launcher row: a real link when its surface exists, otherwise a
 *  non-interactive "not wired yet" chip (no dead-clicks). */
function Launcher({
  label,
  hint,
  onClick,
  wired = true,
}: {
  label: string
  hint?: string
  onClick?: () => void
  wired?: boolean
}) {
  if (!wired || !onClick) {
    return (
      <span
        aria-disabled="true"
        className="flex cursor-default items-center gap-2 rounded-lg border border-line bg-bg-inset px-3 py-2 text-[12px] text-ink-faint opacity-60"
      >
        <span>{label}</span>
        <span className="ml-auto rounded border border-line bg-bg-panel px-1.5 py-0.5 text-[10px]">
          not wired yet
        </span>
      </span>
    )
  }
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded-lg border border-line bg-bg-panel px-3 py-2 text-left text-[12px] text-ink-dim hover:border-line-soft hover:bg-bg-hover"
    >
      <span>{label}</span>
      {hint && <span className="ml-auto text-[11px] text-ink-faint">{hint} →</span>}
    </button>
  )
}

function EmptyState({
  onSuggest,
  onOpenDashboard,
}: {
  onSuggest: (text: string) => void
  onOpenDashboard?: () => void
}) {
  const focusComposer = () =>
    document.getElementById("chat-composer")?.focus()
  const suggestions = [
    "Walk me through the projects and how they depend on each other",
    "Summarize the most recent run and tell me whether anything failed",
    "Which projects have the largest blast radius (most downstream consumers)?",
    "What approval gates are still pending?",
  ]
  return (
    <div className="pb-6 pt-8">
      <div className="mb-4 flex items-center gap-3">
        <div className="inline-flex h-10 w-10 items-center justify-center rounded-lg border border-line bg-bg-inset text-ink-mute">
          <Compass size={18} />
        </div>
        <div>
          <h2 className="text-base font-semibold">Start here</h2>
          <p className="text-[12px] text-ink-mute">
            orchestrate across your registered projects — or jump to a surface.
          </p>
        </div>
      </div>
      {/* onboarding launchers — real links where the surface exists, the rest
          honestly "not wired yet" (no dead-clicks); start a goal focuses below. */}
      <div className="space-y-1.5">
        <Launcher label="Start a goal" hint="type below" onClick={focusComposer} />
        <Launcher
          label="Check readiness"
          hint="Dashboard"
          onClick={onOpenDashboard}
          wired={!!onOpenDashboard}
        />
        <Launcher label="Pick a profile" wired={false} />
        <Launcher label="Run a plan preview" wired={false} />
      </div>
      <div className="mt-5 grid grid-cols-1 gap-2 text-left sm:grid-cols-2">
        {suggestions.map((s) => (
          <button
            key={s}
            onClick={() => onSuggest(s)}
            className="rounded-lg border border-line px-3 py-2.5 text-sm text-ink-dim transition-colors hover:border-line-soft hover:bg-bg-hover hover:text-ink"
          >
            {s}
          </button>
        ))}
      </div>
    </div>
  )
}
