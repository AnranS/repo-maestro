import type { Dispatch, SetStateAction } from "react"
import {
  MessageSquare,
  Layers,
  Sparkles,
  Brain,
  Settings,
  Network,
  GitFork,
  BookOpen,
  ChevronDown,
} from "lucide-react"
import type { RunState, SessionMeta } from "../types"
import type { Tab } from "../hooks/useHashTab"
import type { RunTaskSummary } from "../hooks/useRunState"
import { t } from "../i18n"
import { taskStatusColor } from "./graph/tokens"

interface HeaderProps {
  tab: Tab
  setTab: (tab: Tab) => void
  sessions: SessionMeta[]
  runSummary: RunTaskSummary | null
  connected: boolean
  state: RunState | null
  goalOpen: boolean
  setGoalOpen: Dispatch<SetStateAction<boolean>>
  onOpenSettings: () => void
}

export function Header({
  tab,
  setTab,
  sessions,
  runSummary,
  connected,
  state,
  goalOpen,
  setGoalOpen,
  onOpenSettings,
}: HeaderProps) {
  const goalText = state?.spec || t("header.noActiveRun")

  return (
    <header className="shrink-0 border-b border-line bg-bg/95 backdrop-blur">
      <div className="flex min-h-12 items-center gap-3 px-3 py-2 sm:px-4">
        <div className="flex w-[210px] shrink-0 items-center gap-2">
          <img
            src="/icon-512.png"
            alt="maestro"
            className="h-7 w-7 rounded-md shadow-[0_0_18px_rgba(59,130,246,0.18)]"
          />
          <div className="leading-tight">
            <div className="font-semibold tracking-tight">maestro</div>
            <div className="hidden text-[10px] uppercase tracking-wider text-ink-faint md:block">
              {t("header.subtitle")}
            </div>
          </div>
        </div>

        <nav className="mx-auto flex min-w-0 max-w-full items-center gap-1 overflow-x-auto rounded-lg border border-line bg-bg-inset p-1 scrollbar-thin">
          <TabButton active={tab === "chat"} onClick={() => setTab("chat")} icon={<MessageSquare size={14} />}>
            {t("tab.chat")}
            {sessions.length > 0 && (
              <span className="ml-1 text-[10px] text-ink-faint">{sessions.length}</span>
            )}
          </TabButton>
          <TabButton active={tab === "tasks"} onClick={() => setTab("tasks")} icon={<Layers size={14} />}>
            {t("tab.tasks")}{" "}
            {runSummary && (runSummary.running > 0 || runSummary.failed > 0) && (
              <span
                className={`ml-1 text-[10px] px-1.5 rounded ${
                  runSummary.failed > 0
                    ? "bg-red-900/60 text-red-200"
                    : "bg-blue-900/60 text-blue-200 animate-pulse"
                }`}
              >
                {runSummary.failed > 0
                  ? `${runSummary.failed} ${t("header.failed")}`
                  : `${runSummary.running} ${t("header.running")}`}
              </span>
            )}
          </TabButton>
          <TabButton active={tab === "context"} onClick={() => setTab("context")} icon={<Sparkles size={14} />}>
            {t("tab.context")}
          </TabButton>
          <TabButton active={tab === "memory"} onClick={() => setTab("memory")} icon={<Brain size={14} />}>
            {t("tab.memory")}
          </TabButton>
          <TabButton active={tab === "architecture"} onClick={() => setTab("architecture")} icon={<Network size={14} />}>
            {t("tab.architecture")}
          </TabButton>
          <TabButton active={tab === "codegraph"} onClick={() => setTab("codegraph")} icon={<GitFork size={14} />}>
            {t("tab.codegraph")}
          </TabButton>
          <TabButton active={tab === "docs"} onClick={() => setTab("docs")} icon={<BookOpen size={14} />}>
            {t("tab.docs")}
          </TabButton>
        </nav>

        <div className="flex w-[210px] shrink-0 justify-end">
          <button
            onClick={onOpenSettings}
            className="p-1 rounded text-ink-faint hover:text-ink hover:bg-bg-hover"
            title={t("header.settings")}
          >
            <Settings size={14} />
          </button>
        </div>
      </div>

      <div className="relative flex min-h-10 items-center gap-3 border-t border-line/60 px-3 py-2 sm:px-4">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <span className="shrink-0 text-[10px] uppercase tracking-wider text-ink-faint">goal</span>
          <button
            type="button"
            onClick={() => setGoalOpen((value) => !value)}
            className="group flex min-w-0 flex-1 items-center gap-1.5 text-left"
            title={goalText}
          >
            <span className={`truncate text-sm ${state?.spec ? "text-ink-mute group-hover:text-ink-dim" : "text-ink-faint"}`}>
              {goalText}
            </span>
            <ChevronDown
              size={12}
              className={`shrink-0 text-ink-faint transition-transform ${goalOpen ? "rotate-180" : ""}`}
            />
          </button>
        </div>

        <div className="flex shrink-0 items-center gap-2 text-xs">
          <ConnectionChip connected={connected} />
          {state && state.acceptance_results && state.acceptance_results.length > 0 && (
            <VerifyBadge state={state} />
          )}
          {runSummary && (
            <div className="hidden items-center gap-1.5 sm:flex">
              <CountChip label={t("header.done")} value={runSummary.done} color={taskStatusColor("done")} />
              <CountChip label={t("header.running")} value={runSummary.running} color={taskStatusColor("running")} />
              <CountChip label={t("header.failed")} value={runSummary.failed} color={taskStatusColor("failed")} />
            </div>
          )}
          {state?.usage && <UsageBadge usage={state.usage} />}
        </div>

        {goalOpen && (
          <div className="absolute left-3 right-3 top-full z-30 mt-1 rounded-lg border border-line bg-bg-panel p-3 text-sm leading-relaxed text-ink-dim shadow-2xl sm:left-4 sm:right-4">
            {goalText}
          </div>
        )}
      </div>
    </header>
  )
}

function TabButton(props: {
  active: boolean
  onClick: () => void
  icon: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <button
      onClick={props.onClick}
      aria-pressed={props.active}
      className={`flex shrink-0 items-center gap-1.5 whitespace-nowrap px-2.5 py-1 rounded-md text-sm transition-colors ${
        props.active
          ? "bg-bg-hover text-ink"
          : "text-ink-dim hover:text-ink hover:bg-bg-hover/60"
      }`}
    >
      {props.icon}
      {props.children}
    </button>
  )
}

function CountChip({ label, value, color }: { label: string; value: number; color: string }) {
  return (
    <span className="inline-flex h-6 items-center gap-1.5 rounded-md border border-line bg-bg-inset px-2 text-ink-dim">
      <span className="h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} />
      <span className="hidden text-ink-faint lg:inline">{label}</span>
      <span className="font-semibold text-ink">{value}</span>
    </span>
  )
}

function ConnectionChip({ connected }: { connected: boolean }) {
  return (
    <span className="inline-flex h-6 items-center gap-1.5 rounded-md border border-line bg-bg-inset px-2 text-ink-dim">
      <span className={`h-1.5 w-1.5 rounded-full ${connected ? "bg-emerald-400" : "bg-amber-400"}`} />
      <span className="hidden md:inline">{connected ? t("header.live") : t("header.reconnecting")}</span>
    </span>
  )
}

function VerifyBadge({ state }: { state: RunState }) {
  const results = state.acceptance_results ?? []
  const total = results.length
  const passed = results.filter((r) => r.passed).length
  const ok = state.verified === true
  const cls = ok
    ? "border-emerald-900/70 bg-emerald-900/25 text-emerald-300"
    : "border-red-900/70 bg-red-900/25 text-red-300"
  const label = ok ? `✓ ${t("tasks.verified")}` : `${passed}/${total} ${t("tasks.acceptance")}`
  return (
    <span
      className={`inline-flex h-6 items-center gap-1.5 rounded-md border px-2 ${cls}`}
      title={ok ? t("tasks.allAcceptancePassed") : t("tasks.acceptanceFailing", { n: total - passed })}
    >
      {label}
    </span>
  )
}

function UsageBadge({ usage }: { usage: { input_tokens: number; output_tokens: number; cost_usd?: number | null } }) {
  const total = usage.input_tokens + usage.output_tokens
  if (total === 0 && !usage.cost_usd) return null
  const dollars = usage.cost_usd != null ? `$${usage.cost_usd.toFixed(4)}` : null
  const tokens = total > 1000 ? `${(total / 1000).toFixed(1)}k` : `${total}`
  return (
    <span
      className="inline-flex h-6 items-baseline gap-1 rounded-md border border-amber-900/60 bg-amber-900/20 px-1.5 text-amber-200"
      title={`input ${usage.input_tokens} + output ${usage.output_tokens} tokens`}
    >
      {dollars && <span className="font-medium">{dollars}</span>}
      <span className="text-ink-faint">·</span>
      <span>{tokens} tok</span>
    </span>
  )
}
