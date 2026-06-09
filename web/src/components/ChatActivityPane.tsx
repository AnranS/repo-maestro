// F-UI-002 Slice 2: the collapsible right Activity pane. It consumes ONLY real,
// existing data — the in-flight streaming/thinking affordance, the message
// actions already in the session, and an EXPLICIT session→run link (the live run
// whose session_id matches). It never synthesizes "events" from assistant
// markdown / trajectory / logs / message text. Rows show type / status / short
// id only — never a raw prompt/body/secret/absolute path or an action label
// (which can embed the user's goal text). With no run link the linked-run
// section is an honest empty state.

import { X } from "lucide-react"
import type { RunState, Session } from "../types"
import { StatusChip } from "./ui/StatusChip"

export interface ActivityInFlight {
  streaming: boolean
  thinking: boolean
}

function actionToken(s: string): string {
  switch (s) {
    case "approved":
    case "done":
      return "done"
    case "rejected":
    case "failed":
      return "failed"
    case "running":
      return "running"
    case "pending":
      return "awaiting_approval"
    default:
      return "pending"
  }
}

/** The live run only if it is explicitly linked to this session. */
export function linkedRunFor(
  session: Session | null,
  liveRun: RunState | null | undefined,
): RunState | null {
  if (!session || !liveRun) return null
  return liveRun.session_id === session.id ? liveRun : null
}

/** Count of real activity items (in-flight turn + actions + an explicit run link). */
export function activityCount(
  session: Session | null,
  linkedRun: RunState | null,
  inFlight: ActivityInFlight,
): number {
  const actions = (session?.messages ?? []).reduce(
    (n, m) => n + (m.actions?.length ?? 0),
    0,
  )
  return actions + (linkedRun ? 1 : 0) + (inFlight.streaming ? 1 : 0)
}

function ActivityBody({
  session,
  linkedRun,
  onOpenRun,
  inFlight,
}: {
  session: Session | null
  linkedRun: RunState | null
  onOpenRun?: (runId: string) => void
  inFlight: ActivityInFlight
}) {
  const actions = (session?.messages ?? []).flatMap((m) => m.actions ?? [])
  const tasksDone = linkedRun
    ? Object.values(linkedRun.tasks).filter((t) => t.status === "done").length
    : 0
  const tasksTotal = linkedRun ? Object.keys(linkedRun.tasks).length : 0

  return (
    <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3 text-[11px]">
      {/* in-flight turn — status/count only, never the thinking/draft text */}
      {inFlight.streaming && (
        <section>
          <div className="mb-1 text-ink-faint">in flight</div>
          <div className="flex items-center gap-2">
            <StatusChip status="streaming" />
            <span className="text-ink-dim">
              assistant turn{inFlight.thinking ? " · thinking" : ""}
            </span>
          </div>
        </section>
      )}
      <section>
        <div className="mb-1 text-ink-faint">linked run</div>
        {linkedRun ? (
          <button
            onClick={() => onOpenRun?.(linkedRun.run_id)}
            className="flex w-full items-center gap-2 rounded border border-line bg-bg-inset px-2 py-1.5 text-left hover:bg-bg-hover/40"
          >
            <StatusChip status={linkedRun.status} />
            <span className="flex-1 truncate font-mono text-ink-dim">
              {linkedRun.run_id.slice(0, 16)}
            </span>
            <span className="shrink-0 font-mono tabular-nums text-ink-faint">
              {tasksDone}/{tasksTotal}
            </span>
          </button>
        ) : (
          <div className="text-ink-faint">no run linked to this session</div>
        )}
      </section>
      <section>
        <div className="mb-1 text-ink-faint">actions ({actions.length})</div>
        {actions.length === 0 ? (
          <div className="text-ink-faint">none</div>
        ) : (
          <ul className="space-y-1">
            {/* verb + status + short id only — no `label` (it can embed the goal).
                Status defaults to "pending" so every row keeps the type/status/id
                contract. */}
            {actions.map((a) => {
              const status = a.status ?? "pending"
              return (
                <li key={a.id} className="flex items-center gap-2">
                  <StatusChip status={actionToken(status)} label={status} />
                  <span className="font-mono text-ink-mute">{a.verb}</span>
                  <span className="ml-auto shrink-0 font-mono text-[10px] text-ink-faint">
                    {a.id.slice(0, 8)}
                  </span>
                </li>
              )
            })}
          </ul>
        )}
      </section>
    </div>
  )
}

export function ChatActivityPane({
  session,
  linkedRun,
  onOpenRun,
  inFlight,
  open,
  onClose,
}: {
  session: Session | null
  linkedRun: RunState | null
  onOpenRun?: (runId: string) => void
  inFlight: ActivityInFlight
  open: boolean
  onClose: () => void
}) {
  if (!open) return null
  const body = (
    <ActivityBody
      session={session}
      linkedRun={linkedRun}
      onOpenRun={onOpenRun}
      inFlight={inFlight}
    />
  )
  return (
    <>
      {/* desktop: a fixed-width side column that never squeezes the center */}
      <aside className="hidden w-72 shrink-0 flex-col border-l border-line bg-bg-panel md:flex">
        <div className="border-b border-line px-3 py-2 text-xs font-medium text-ink-mute">
          Activity
        </div>
        {body}
      </aside>
      {/* mobile: a sheet/drawer overlay (no hard third column, no h-scroll) */}
      <div
        className="fixed inset-0 z-scrim bg-black/40 md:hidden"
        onClick={onClose}
        aria-hidden="true"
      />
      <aside className="fixed inset-y-0 right-0 z-drawer flex w-72 max-w-[85vw] flex-col border-l border-line bg-bg-panel shadow-overlay md:hidden">
        <div className="flex items-center border-b border-line px-3 py-2 text-xs font-medium text-ink-mute">
          Activity
          <button
            onClick={onClose}
            className="ml-auto text-ink-faint hover:text-ink-dim"
            title="close activity"
            aria-label="close activity"
          >
            <X size={13} />
          </button>
        </div>
        {body}
      </aside>
    </>
  )
}
