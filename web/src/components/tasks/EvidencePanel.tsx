import { useState } from "react"
import type { ReactNode } from "react"
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Clock,
  Copy,
  FileText,
  GitBranch,
  Monitor,
  XCircle,
} from "lucide-react"
import type { ParallelWindow, RunEvidence, RunReplay } from "../../types"
import { t } from "../../i18n"
import { CollapsibleSection } from "../ui/CollapsibleSection"

export function EvidencePanel({
  evidence,
  replay,
  prBody,
}: {
  evidence: RunEvidence
  replay: RunReplay | null
  prBody: string
}) {
  const [tab, setTab] = useState<"overview" | "timeline" | "browser" | "pr">("overview")
  const worktreeCount = evidence.tasks.filter((task) => task.worktree_path).length
  const acceptancePassed = evidence.acceptance.filter((check) => check.passed).length
  const browser = evidence.browser ?? { present: false, artifact_count: 0 }
  return (
    <CollapsibleSection
      title={t("tasks.evidence")}
      summary={`${acceptancePassed}/${evidence.acceptance.length}`}
    >
      <div className="px-4 py-2.5 border-b border-line flex items-center justify-end">
        <div className="flex flex-wrap items-center justify-end gap-2">
          {(["overview", "timeline", "browser", "pr"] as const).map((name) => (
            <button
              key={name}
              onClick={() => setTab(name)}
              className={`px-2 py-1 rounded text-[11px] capitalize ${
                tab === name
                  ? "bg-accent/20 text-accent border border-accent/30"
                  : "text-ink-faint hover:text-ink-dim"
              }`}
            >
              {t(`tasks.evidenceTab.${name}`)}
            </button>
          ))}
        </div>
      </div>
      <div className="p-4">
        {tab === "overview" && (
          <div className="grid gap-3 lg:grid-cols-[0.9fr_1.1fr]">
            <div className="grid grid-cols-2 gap-2 text-xs">
              <EvidenceMetric
                icon={<Activity size={13} />}
                label={t("tasks.observed")}
                value={`${evidence.max_observed_parallelism}/${evidence.max_parallel}`}
              />
              <EvidenceMetric
                icon={<GitBranch size={13} />}
                label={t("tasks.worktrees")}
                value={`${worktreeCount}/${evidence.task_count}`}
              />
              <EvidenceMetric
                icon={<Monitor size={13} />}
                label={t("tasks.browser")}
                value={
                  browser.present
                    ? t("tasks.fileCount", { n: browser.artifact_count })
                    : t("tasks.none")
                }
              />
              <EvidenceMetric
                label={t("tasks.acceptance")}
                value={
                  evidence.acceptance.length
                    ? `${acceptancePassed}/${evidence.acceptance.length}`
                    : t("tasks.none")
                }
              />
            </div>
            <ParallelWindows windows={evidence.parallel_windows} />
          </div>
        )}
        {tab === "timeline" && <ReplayTimeline replay={replay} />}
        {tab === "browser" && <BrowserEvidencePanel evidence={evidence} />}
        {tab === "pr" && <PrBodyPanel body={prBody} />}
      </div>
    </CollapsibleSection>
  )
}

function ParallelWindows({ windows }: { windows: ParallelWindow[] }) {
  if (windows.length === 0) {
    return <p className="text-xs text-ink-faint">{t("tasks.noOverlap")}</p>
  }
  return (
    <div className="space-y-1.5 min-w-0">
      {windows.slice(0, 6).map((window, idx) => (
        <ParallelWindowRow key={`${window.started_at}-${idx}`} window={window} />
      ))}
    </div>
  )
}

function ReplayTimeline({ replay }: { replay: RunReplay | null }) {
  if (!replay) {
    return <p className="text-xs text-ink-faint">{t("tasks.replayUnavailable")}</p>
  }
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-4 text-xs text-ink-faint">
        <span className="inline-flex items-center gap-1">
          <Clock size={13} /> {t("tasks.eventCount", { n: replay.event_count })}
        </span>
        <span>{t("tasks.observedParallel", { n: replay.max_observed_parallelism })}</span>
      </div>
      <div className="divide-y divide-line/40 border border-line/60 rounded-md overflow-hidden">
        {replay.events.slice(-14).map((event) => (
          <div
            key={event.seq}
            className="grid grid-cols-[56px_96px_1fr] gap-2 px-3 py-2 text-xs"
          >
            <span className="font-mono text-ink-faint">#{event.seq}</span>
            <span className="font-mono text-ink-dim">{event.kind}</span>
            <span className="min-w-0 truncate">
              {event.task_id && (
                <span className="font-mono text-accent mr-2">{event.task_id}</span>
              )}
              <span className="text-ink-faint">{event.message}</span>
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}

function BrowserEvidencePanel({ evidence }: { evidence: RunEvidence }) {
  const browser = evidence.browser
  if (!browser?.present) {
    return <p className="text-xs text-ink-faint">{t("tasks.noBrowserQa")}</p>
  }
  const rows = [
    ["screenshots", browser.screenshots ?? []],
    ["traces", browser.traces ?? []],
    ["DOM/report", browser.dom_snapshots ?? []],
    ["videos", browser.videos ?? []],
  ] as const
  return (
    <div className="grid gap-4 lg:grid-cols-[0.85fr_1.15fr]">
      <div className="grid grid-cols-2 gap-2 text-xs">
        <EvidenceMetric
          icon={<Monitor size={13} />}
          label={t("tasks.artifacts")}
          value={String(browser.artifact_count)}
        />
        <EvidenceMetric label={t("tasks.checks")} value={String(browser.checks?.length ?? 0)} />
        <EvidenceMetric label={t("tasks.network")} value={String(browser.network_failures?.length ?? 0)} />
        <EvidenceMetric label={t("tasks.console")} value={String(browser.console_errors?.length ?? 0)} />
      </div>
      <div className="space-y-3 min-w-0">
        {browser.checks?.map((check) => (
          <div key={check.evidence_path ?? check.check} className="flex items-start gap-2 text-xs">
            {check.passed ? (
              <CheckCircle2 size={14} className="mt-0.5 text-green-300 shrink-0" />
            ) : (
              <XCircle size={14} className="mt-0.5 text-status-danger shrink-0" />
            )}
            <div className="min-w-0">
              <div className="text-ink-dim truncate">{check.describe}</div>
              {check.evidence_path && (
                <div className="font-mono text-ink-faint truncate">{check.evidence_path}</div>
              )}
            </div>
          </div>
        ))}
        {rows.map(([label, paths]) =>
          paths.length > 0 ? (
            <div key={label} className="text-xs min-w-0">
              <div className="text-ink-faint mb-1">{t(`tasks.browserArtifact.${label}`)}</div>
              {paths.slice(0, 4).map((path) => (
                <div key={path} className="font-mono text-ink-dim truncate">{path}</div>
              ))}
            </div>
          ) : null,
        )}
        <FailureList
          icon={<AlertTriangle size={13} />}
          label={t("tasks.network")}
          lines={browser.network_failures ?? []}
        />
        <FailureList
          icon={<AlertTriangle size={13} />}
          label={t("tasks.console")}
          lines={browser.console_errors ?? []}
        />
      </div>
    </div>
  )
}

function EvidenceMetric({
  icon,
  label,
  value,
}: {
  icon?: ReactNode
  label: string
  value: string
}) {
  return (
    <div className="border-l border-line/70 px-3 py-1.5 min-w-0">
      <div className="flex items-center gap-1.5 text-ink-faint">
        {icon}
        <span className="truncate">{label}</span>
      </div>
      <div className="mt-1 text-sm text-ink font-mono truncate">{value}</div>
    </div>
  )
}

function ParallelWindowRow({ window }: { window: ParallelWindow }) {
  return (
    <div className="flex items-center gap-2 text-xs min-w-0">
      <span className="font-mono text-ink-faint w-16 shrink-0">
        x{window.concurrency}
      </span>
      <span className="font-mono text-ink-dim truncate">
        {window.task_ids.join(", ")}
      </span>
      <span className="text-ink-faint truncate">
        {window.projects.join(" / ")}
      </span>
    </div>
  )
}

function FailureList({
  icon,
  label,
  lines,
}: {
  icon: ReactNode
  label: string
  lines: string[]
}) {
  if (lines.length === 0) return null
  return (
    <div className="text-xs min-w-0">
      <div className="flex items-center gap-1 text-ink-faint mb-1">
        {icon}
        <span>{label}</span>
      </div>
      {lines.slice(0, 4).map((line) => (
        <div key={line} className="font-mono text-status-danger/80 truncate">{line}</div>
      ))}
    </div>
  )
}

function PrBodyPanel({ body }: { body: string }) {
  const [copied, setCopied] = useState(false)
  if (!body) {
    return <p className="text-xs text-ink-faint">{t("tasks.prUnavailable")}</p>
  }
  const copy = async () => {
    await navigator.clipboard.writeText(body)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1200)
  }
  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between text-xs text-ink-faint">
        <span className="inline-flex items-center gap-1">
          <FileText size={13} /> {t("tasks.prDraft")}
        </span>
        <button
          onClick={copy}
          className="inline-flex items-center gap-1.5 px-2 py-1 rounded border border-line hover:border-accent/50 hover:text-ink-dim"
        >
          <Copy size={12} />
          {copied ? t("common.copied") : t("common.copy")}
        </button>
      </div>
      <pre className="max-h-72 overflow-auto rounded-md border border-line/60 bg-bg/70 p-3 text-[11px] leading-relaxed text-ink-dim whitespace-pre-wrap">
        {body}
      </pre>
    </div>
  )
}
