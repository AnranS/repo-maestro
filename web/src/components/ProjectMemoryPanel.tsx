import { useEffect, useState } from "react"
import { X, Brain, Cpu, GitBranch, History, ArrowDownLeft, ArrowUpRight, FileText, Check, AlertCircle } from "lucide-react"
import { api } from "../api"
import type { ProjectMemoryView } from "../types"

interface Props {
  projectName: string
  onClose: () => void
}

/**
 * Side panel rendered alongside the Architecture graph. Shows everything
 * maestro "remembers" about one project: identity (type / stack / role),
 * contracts in and out, L1 facts auto-injected via memory_scope, L2
 * decisions archived from successful runs, and recent runs that touched
 * this project.
 *
 * Driven by the `GET /api/projects/:name/memory` aggregate endpoint —
 * one request, everything we need to populate the panel.
 */
export function ProjectMemoryPanel({ projectName, onClose }: Props) {
  const [view, setView] = useState<ProjectMemoryView | null>(null)
  const [loading, setLoading] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setErr(null)
    setView(null)
    api
      .projectMemory(projectName)
      .then((v) => {
        if (!cancelled) setView(v)
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e.message ?? e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [projectName])

  return (
    <aside className="absolute right-0 top-0 bottom-0 w-[26rem] max-w-[90vw] bg-bg-panel border-l border-line shadow-overlay flex flex-col z-drawer">
      <header className="flex items-center justify-between px-4 py-3 border-b border-line">
        <div className="flex items-center gap-2 min-w-0">
          <Brain size={14} className="text-accent shrink-0" />
          <h2 className="text-sm font-semibold truncate" title={projectName}>
            {projectName}
          </h2>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="text-ink-faint hover:text-ink shrink-0"
          aria-label="close"
        >
          <X size={14} />
        </button>
      </header>

      <div className="flex-1 overflow-y-auto scrollbar-thin">
        {loading && (
          <p className="p-4 text-xs text-ink-faint">loading…</p>
        )}
        {err && (
          <p className="m-4 text-xs text-status-danger bg-red-500/10 border border-red-500/30 rounded px-2.5 py-2 inline-flex items-start gap-2">
            <AlertCircle size={12} className="mt-0.5 shrink-0" />
            {err}
          </p>
        )}
        {view && (
          <>
            <Identity view={view} />
            <Section icon={<GitBranch size={11} />} label="contracts">
              <Contracts contracts={view.contracts} />
            </Section>
            <Section
              icon={<Brain size={11} />}
              label={`L1 facts · ${view.l1_facts.length} via memory_scope`}
            >
              {view.l1_facts.length === 0 ? (
                <Empty>
                  no facts in scope.{" "}
                  {view.memory_scope.length === 0
                    ? "set memory_scope on the project to opt into topics."
                    : `topics: ${view.memory_scope.join(", ")} (none populated)`}
                </Empty>
              ) : (
                view.l1_facts.map((f) => (
                  <Slice
                    key={`${f.topic}/${f.file}`}
                    title={`${f.topic} / ${f.file}`}
                    body={f.preview}
                  />
                ))
              )}
            </Section>
            <Section
              icon={<History size={11} />}
              label={`L2 decisions · ${view.l2_decisions.length} archived`}
            >
              {view.l2_decisions.length === 0 ? (
                <Empty>
                  no archived decisions yet — successful runs touching this
                  project will land here automatically.
                </Empty>
              ) : (
                view.l2_decisions.map((d) => (
                  <Slice key={d.file} title={d.file} body={d.preview} />
                ))
              )}
            </Section>
            <Section
              icon={<Cpu size={11} />}
              label={`recent runs · ${view.recent_runs.length}`}
            >
              {view.recent_runs.length === 0 ? (
                <Empty>no past runs touched this project.</Empty>
              ) : (
                view.recent_runs.map((r) => <RunRow key={r.run_id} run={r} />)
              )}
            </Section>
          </>
        )}
      </div>
    </aside>
  )
}

function Identity({ view }: { view: ProjectMemoryView }) {
  return (
    <div className="px-4 py-3 border-b border-line/60 space-y-1.5 text-xs">
      <div className="flex items-center gap-2">
        {view.role && (
          <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-blue-500/15 text-status-info border border-blue-500/30 text-[10px]">
            {view.role}
          </span>
        )}
        {view.type && (
          <span className="text-ink-faint">{view.type}</span>
        )}
      </div>
      {view.stack.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {view.stack.map((s) => (
            <span
              key={s}
              className="px-1.5 py-0.5 rounded bg-bg-inset border border-line text-[10px] text-ink-dim font-mono"
            >
              {s}
            </span>
          ))}
        </div>
      )}
      <code className="block text-[10px] text-ink-faint truncate" title={view.path}>
        {view.path}
      </code>
    </div>
  )
}

function Section({
  icon,
  label,
  children,
}: {
  icon: React.ReactNode
  label: string
  children: React.ReactNode
}) {
  return (
    <section className="border-b border-line/60">
      <header className="flex items-center gap-1.5 px-4 py-2 text-[10px] uppercase tracking-wider text-ink-faint sticky top-0 bg-bg-panel z-sticky">
        {icon}
        <span>{label}</span>
      </header>
      <div className="px-4 pb-3 space-y-2">{children}</div>
    </section>
  )
}

function Contracts({
  contracts,
}: {
  contracts: ProjectMemoryView["contracts"]
}) {
  if (!contracts.provides && !contracts.consumes) {
    return (
      <Empty>
        no contracts declared — without provides/consumes the architecture
        view can't draw edges and contract-aware memory fan-in can't fire.
      </Empty>
    )
  }
  return (
    <div className="space-y-1.5 text-[11px]">
      {contracts.provides && (
        <div className="flex items-start gap-1.5">
          <ArrowUpRight size={10} className="mt-0.5 text-status-success shrink-0" />
          <div>
            <span className="text-ink-faint">provides </span>
            <code className="text-status-success/90 font-mono">
              {contracts.provides}
            </code>
          </div>
        </div>
      )}
      {contracts.consumes && (
        <div className="flex items-start gap-1.5">
          <ArrowDownLeft size={10} className="mt-0.5 text-cyan-300 shrink-0" />
          <div>
            <span className="text-ink-faint">consumes </span>
            <code className="text-cyan-300/90 font-mono">
              {contracts.consumes}
            </code>
          </div>
        </div>
      )}
    </div>
  )
}

function Slice({ title, body }: { title: string; body: string }) {
  return (
    <details className="bg-bg-inset border border-line/60 rounded">
      <summary className="cursor-pointer px-2 py-1.5 text-[11px] font-mono text-ink-dim hover:text-ink flex items-center gap-1.5">
        <FileText size={10} className="shrink-0 text-ink-faint" />
        <span className="truncate">{title}</span>
      </summary>
      <pre className="px-2.5 pb-2 pt-1 text-[10.5px] whitespace-pre-wrap break-words text-ink-faint leading-relaxed max-h-72 overflow-y-auto scrollbar-thin">
        {body}
      </pre>
    </details>
  )
}

function RunRow({ run }: { run: ProjectMemoryView["recent_runs"][number] }) {
  const status = run.status
  const cls =
    status === "done" && run.verified
      ? "text-status-success"
      : status === "done"
        ? "text-status-warning"
        : status === "failed"
          ? "text-status-danger"
          : "text-ink-dim"
  return (
    <div className="px-2 py-1.5 text-[11px] bg-bg-inset border border-line/60 rounded">
      <div className="flex items-center gap-1.5 mb-0.5">
        <span className={`text-[10px] font-mono ${cls}`}>{status}</span>
        {run.verified && (
          <Check size={9} className="text-status-success" />
        )}
        <code className="text-[10px] text-ink-faint font-mono ml-auto">
          {run.run_id.slice(0, 14)}
        </code>
      </div>
      <p className="text-ink-dim truncate" title={run.spec}>
        {run.spec}
      </p>
      <p className="text-[10px] text-ink-faint mt-0.5">
        tasks: {run.task_ids_in_project.join(", ")}
      </p>
    </div>
  )
}

function Empty({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-[11px] text-ink-faint italic leading-relaxed">{children}</p>
  )
}
