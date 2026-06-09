import { useState, type ReactNode } from "react"
import {
  Activity,
  AlertTriangle,
  BarChart3,
  CheckCircle2,
  Coins,
  Link2,
  OctagonX,
  RotateCw,
  ShieldCheck,
} from "lucide-react"
import type { AutoAction, RunState, TaskState } from "../../types"
import { t } from "../../i18n"
import { CollapsibleSection } from "../ui/CollapsibleSection"

/** Derive the per-status tallies + headline labels the summary strip renders. */
export function taskSummary(tasks: TaskState[], state: RunState) {
  const count = (status: string) => tasks.filter((task) => task.status === status).length
  const terminal = tasks.filter((task) =>
    ["done", "failed", "skipped", "cancelled"].includes(task.status),
  ).length
  const total = tasks.length
  const percent = total === 0 ? 0 : Math.round((terminal / total) * 100)
  const acceptanceTotal = state.acceptance_results?.length ?? 0
  const acceptancePassed = state.acceptance_results?.filter((check) => check.passed).length ?? 0
  const active = tasks.filter((task) => task.status === "running").map((task) => task.id)
  const waiting = tasks.filter((task) => task.status === "awaiting_approval").map((task) => task.id)
  return {
    total,
    terminal,
    percent,
    done: count("done"),
    running: count("running"),
    failed: count("failed"),
    pending: count("pending"),
    skipped: count("skipped"),
    cancelled: count("cancelled"),
    awaitingApproval: count("awaiting_approval"),
    acceptanceTotal,
    acceptancePassed,
    activeLabel:
      active.length > 0
        ? t("tasks.runningList", { ids: active.join(", ") })
        : waiting.length > 0
          ? t("tasks.awaitingList", { ids: waiting.join(", ") })
          : t("tasks.settledList", { done: terminal, total }),
  }
}

export type TaskSummary = ReturnType<typeof taskSummary>

export function RunSummaryStrip({ summary }: { summary: TaskSummary }) {
  return (
    <section className="grid gap-3 md:grid-cols-4">
      <SummaryTile
        icon={<BarChart3 size={14} />}
        label={t("tasks.summaryProgress")}
        value={`${summary.terminal}/${summary.total}`}
        detail={t("tasks.settledPct", { n: summary.percent })}
        tone="blue"
        progress={summary.percent}
      />
      <SummaryTile
        icon={<Activity size={14} />}
        label={t("tasks.summaryActive")}
        value={summary.running + summary.awaitingApproval}
        detail={summary.activeLabel}
        tone={summary.awaitingApproval > 0 ? "amber" : "blue"}
      />
      <SummaryTile
        icon={<CheckCircle2 size={14} />}
        label={t("tasks.summaryDone")}
        value={summary.done}
        detail={t("tasks.doneDetail", {
          failed: summary.failed,
          cancelled: summary.cancelled,
        })}
        tone={summary.failed > 0 ? "red" : "emerald"}
      />
      <SummaryTile
        icon={<ShieldCheck size={14} />}
        label={t("tasks.summaryAcceptance")}
        value={summary.acceptanceTotal ? `${summary.acceptancePassed}/${summary.acceptanceTotal}` : t("tasks.none")}
        detail={summary.acceptanceTotal ? t("tasks.verificationChecks") : t("tasks.noGoalGate")}
        tone={summary.acceptanceTotal && summary.acceptancePassed < summary.acceptanceTotal ? "red" : "emerald"}
      />
    </section>
  )
}

function SummaryTile({
  icon,
  label,
  value,
  detail,
  tone,
  progress,
}: {
  icon: ReactNode
  label: string
  value: ReactNode
  detail: string
  tone: "blue" | "emerald" | "amber" | "red"
  progress?: number
}) {
  const toneClass = {
    blue: "text-status-info bg-blue-500/10 border-blue-500/25",
    emerald: "text-status-success bg-emerald-500/10 border-emerald-500/25",
    amber: "text-status-warning bg-amber-500/10 border-amber-500/25",
    red: "text-status-danger bg-red-500/10 border-red-500/25",
  }[tone]
  const barClass = {
    blue: "bg-blue-400",
    emerald: "bg-emerald-400",
    amber: "bg-amber-400",
    red: "bg-red-400",
  }[tone]
  return (
    <div className="rounded-lg border border-line bg-bg-panel p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[10px] uppercase tracking-wider text-ink-faint">{label}</span>
        <span className={`inline-flex h-6 w-6 items-center justify-center rounded border ${toneClass}`}>
          {icon}
        </span>
      </div>
      <div className="mt-2 text-xl font-semibold leading-none text-ink">{value}</div>
      <div className="mt-1 truncate text-[11px] text-ink-mute">{detail}</div>
      {typeof progress === "number" && (
        <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-bg-inset">
          <div className={`h-full rounded-full ${barClass}`} style={{ width: `${progress}%` }} />
        </div>
      )}
    </div>
  )
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return `${n}`
}

type CostRow = { key: string; tokens: number; cost: number }

function aggregate(tasks: TaskState[], by: "project" | "agent"): CostRow[] {
  const m = new Map<string, CostRow>()
  for (const tk of tasks) {
    if (!tk.usage) continue
    const tokens = (tk.usage.input_tokens || 0) + (tk.usage.output_tokens || 0)
    if (tokens === 0 && !tk.usage.cost_usd) continue
    const k = (by === "project" ? tk.project : tk.agent) || "—"
    const row = m.get(k) ?? { key: k, tokens: 0, cost: 0 }
    row.tokens += tokens
    row.cost += tk.usage.cost_usd || 0
    m.set(k, row)
  }
  return [...m.values()].sort((a, b) => b.tokens - a.tokens)
}

/**
 * Cost breakdown for the run — total tokens/$ with consumption against the
 * `--max-tokens` budget (if set), plus a per-project / per-agent split so you
 * can see *where* the spend goes, not just the headline number. Completes the
 * cost-control story the budget gate started (P4). Renders nothing for runs
 * that reported no usage (e.g. mock/shell-only).
 */
export function CostPanel({ state }: { state: RunState }) {
  const [by, setBy] = useState<"project" | "agent">("project")
  const tasks = Object.values(state.tasks)
  const rows = aggregate(tasks, by)
  const totalTokens =
    (state.usage?.input_tokens || 0) + (state.usage?.output_tokens || 0) ||
    rows.reduce((s, r) => s + r.tokens, 0)
  if (totalTokens === 0) return null
  const totalCost = state.usage?.cost_usd ?? rows.reduce((s, r) => s + r.cost, 0)
  const budget = state.budget_tokens ?? null
  const pct = budget ? Math.min(100, Math.round((totalTokens / budget) * 100)) : null
  const over = budget != null && totalTokens > budget
  const barColor = over
    ? "bg-red-400"
    : pct != null && pct >= 80
      ? "bg-amber-400"
      : "bg-blue-400"
  const max = Math.max(1, ...rows.map((r) => r.tokens))
  return (
    <CollapsibleSection
      title={t("cost.title")}
      icon={
        <span className="inline-flex h-5 w-5 items-center justify-center rounded border border-blue-500/25 bg-blue-500/10 text-status-info">
          <Coins size={12} />
        </span>
      }
      summary={`${fmtTokens(totalTokens)} ${t("cost.tokens")}${totalCost > 0 ? ` · $${totalCost.toFixed(2)}` : ""}`}
      defaultOpen={over || (pct != null && pct >= 80)}
    >
      <div className="p-3">
      <div className="mb-2 flex items-center">
        <div className="ml-auto flex items-center rounded-lg border border-line bg-bg-inset p-0.5">
          {(["project", "agent"] as const).map((k) => (
            <button
              key={k}
              onClick={() => setBy(k)}
              className={`rounded px-2 py-0.5 text-[10px] ${by === k ? "bg-bg-hover text-ink" : "text-ink-faint hover:text-ink-dim"}`}
            >
              {t(`cost.by_${k}`)}
            </button>
          ))}
        </div>
      </div>
      {budget != null && (
        <div className="mb-3">
          <div className="mb-1 flex items-center justify-between text-[11px]">
            <span className={over ? "text-status-danger" : "text-ink-mute"}>
              {t("cost.budget", { used: fmtTokens(totalTokens), budget: fmtTokens(budget) })}
            </span>
            <span className={over ? "text-status-danger font-medium" : "text-ink-faint"}>{pct}%</span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-bg-inset">
            <div className={`h-full rounded-full ${barColor}`} style={{ width: `${pct}%` }} />
          </div>
        </div>
      )}
      <ul className="space-y-1.5">
        {rows.map((r) => (
          <li key={r.key} className="flex items-center gap-2 text-[12px]">
            <span className="w-28 shrink-0 truncate text-ink-dim" title={r.key}>
              {r.key}
            </span>
            <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-bg-inset">
              <div
                className="h-full rounded-full bg-blue-400/70"
                style={{ width: `${Math.round((r.tokens / max) * 100)}%` }}
              />
            </div>
            <span className="w-12 shrink-0 text-right font-mono text-[11px] text-ink-mute">
              {fmtTokens(r.tokens)}
            </span>
            {totalCost > 0 && (
              <span className="w-14 shrink-0 text-right font-mono text-[11px] text-ink-faint">
                ${r.cost.toFixed(2)}
              </span>
            )}
          </li>
        ))}
      </ul>
      </div>
    </CollapsibleSection>
  )
}

/** Visual treatment per auto-action kind. */
const AUTO_ACTION_META: Record<
  string,
  { icon: ReactNode; label: string; tone: "blue" | "amber" | "red" }
> = {
  contract_wired: { icon: <Link2 size={13} />, label: t("tasks.autoWired"), tone: "blue" },
  retry: { icon: <RotateCw size={13} />, label: t("tasks.autoRetry"), tone: "amber" },
  circuit_break: { icon: <OctagonX size={13} />, label: t("tasks.autoCircuitBreak"), tone: "red" },
  integration_conflict: {
    icon: <AlertTriangle size={13} />,
    label: t("tasks.autoConflict"),
    tone: "red",
  },
}

/**
 * "What maestro did for you" — the run's auto-actions ledger (contract edges
 * wired, retries, circuit-breaker stops, integration conflicts), grouped by
 * kind so escalations stand out. Renders nothing when the run made none.
 */
export function AutoActionsPanel({ actions }: { actions?: AutoAction[] }) {
  if (!actions || actions.length === 0) return null
  const order = ["circuit_break", "integration_conflict", "retry", "contract_wired"]
  const groups = new Map<string, AutoAction[]>()
  for (const a of actions) {
    const arr = groups.get(a.kind) ?? []
    arr.push(a)
    groups.set(a.kind, arr)
  }
  const kinds = [...groups.keys()].sort(
    (a, b) => (order.indexOf(a) + 1 || 99) - (order.indexOf(b) + 1 || 99),
  )
  const toneClass = {
    blue: "text-status-info bg-blue-500/10 border-blue-500/25",
    amber: "text-status-warning bg-amber-500/10 border-amber-500/25",
    red: "text-status-danger bg-red-500/10 border-red-500/25",
  }
  return (
    <CollapsibleSection
      title={t("tasks.autoActionsTitle")}
      summary={t("tasks.autoActionsHint")}
    >
      <div className="p-3">
      <ul className="space-y-1.5">
        {kinds.flatMap((kind) => {
          const meta = AUTO_ACTION_META[kind] ?? {
            icon: <Activity size={13} />,
            label: kind,
            tone: "blue" as const,
          }
          return groups.get(kind)!.map((a, i) => (
            <li key={`${kind}-${i}`} className="flex items-start gap-2 text-[12px]">
              <span
                className={`mt-0.5 inline-flex h-5 items-center gap-1 rounded border px-1.5 text-[10px] font-medium ${toneClass[meta.tone]}`}
              >
                {meta.icon}
                {meta.label}
              </span>
              <span className="flex-1 text-ink-mute">
                {a.task && <code className="text-ink-dim">{a.task}</code>} {a.detail}
              </span>
            </li>
          ))
        })}
      </ul>
      </div>
    </CollapsibleSection>
  )
}

export function StatusPill({ status }: { status: string }) {
  const cls = {
    running: "border-blue-500/30 bg-blue-500/10 text-status-info",
    done: "border-emerald-500/30 bg-emerald-500/10 text-status-success",
    failed: "border-red-500/30 bg-red-500/10 text-status-danger",
    cancelled: "border-line bg-bg-inset text-ink-dim",
  }[status] ?? "border-line bg-bg-inset text-ink-dim"
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs font-medium ${cls}`}>
      <span className="h-1.5 w-1.5 rounded-full bg-current" />
      {statusText(status)}
    </span>
  )
}

function statusText(status: string) {
  const key = `status.${status}`
  const translated = t(key)
  return translated === key ? status : translated
}
