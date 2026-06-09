import { useEffect, useMemo, useRef, useState } from "react"
import {
  X,
  Save,
  RefreshCw,
  ChevronDown,
  Check,
  Search,
  Globe,
  Cpu,
  Network,
  ShieldCheck,
  Server,
  FileText,
} from "lucide-react"
import { api } from "../api"
import { CollapsibleSection } from "./ui/CollapsibleSection"
import type {
  DefaultsConfig,
  Enforcement,
  ModelInfo,
  ProviderEnforcementProfile,
} from "../types"
import { LANG_LABELS, SUPPORTED_LANGS, getLang, setLang, t, useLang } from "../i18n"

export function SettingsModal({
  models,
  onClose,
  onRefreshModels,
}: {
  models: ModelInfo[]
  onClose: () => void
  onRefreshModels: () => Promise<void>
}) {
  // Subscribe so the modal re-renders when the user flips language inside
  // the modal itself.
  useLang()
  const [d, setD] = useState<DefaultsConfig | null>(null)
  const [profiles, setProfiles] = useState<ProviderEnforcementProfile[]>([])
  const [profilesLoading, setProfilesLoading] = useState(true)
  const [profilesErr, setProfilesErr] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    api.defaults().then(setD).catch((e) => setErr(String(e)))
    // Read-only provider enforcement matrix (F-136a2). It is a SECURITY info surface —
    // a fetch failure must surface as "unavailable", NEVER silently hide the matrix
    // (F-136a2 B1). Doesn't block saving defaults.
    api
      .providerProfiles()
      .then((p) => {
        setProfiles(p)
        setProfilesErr(null)
      })
      .catch((e) => setProfilesErr(e instanceof Error ? e.message : String(e)))
      .finally(() => setProfilesLoading(false))
  }, [])

  const save = async () => {
    if (!d) return
    setSaving(true)
    try {
      const updated = await api.updateDefaults({
        agent: d.agent,
        branch_prefix: d.branch_prefix,
        max_parallel: d.max_parallel,
        agent_model: d.agent_model ?? d.cursor_model ?? "",
        tagger_model: d.tagger_model ?? "",
      })
      setD(updated)
      onClose()
    } catch (e) {
      setErr(String(e))
    } finally {
      setSaving(false)
    }
  }

  const refresh = async () => {
    setRefreshing(true)
    try {
      await onRefreshModels()
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-modal bg-black/60 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-bg-panel border border-line rounded-lg w-full max-w-2xl shadow-overlay"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-line">
          <h2 className="text-sm font-semibold">{t("settings.title")}</h2>
          <button onClick={onClose} className="text-ink-faint hover:text-ink">
            <X size={14} />
          </button>
        </div>

        {!d ? (
          <div className="p-5 text-sm text-ink-faint">{t("common.loading")}</div>
        ) : (
          <div className="max-h-[70vh] space-y-3 overflow-y-auto scrollbar-thin p-4">
            {/* Language is a UI preference, not a security setting — above the blocks. */}
            <Field label={t("settings.language")}>
              <div className="flex items-center gap-1 bg-bg-inset border border-line rounded p-0.5 w-fit">
                {SUPPORTED_LANGS.map((code) => {
                  const active = getLang() === code
                  return (
                    <button
                      key={code}
                      onClick={() => setLang(code)}
                      className={`inline-flex items-center gap-1.5 px-3 py-1 rounded text-xs transition-colors ${
                        active
                          ? "bg-blue-600 text-white"
                          : "text-ink-dim hover:text-ink hover:bg-bg-hover"
                      }`}
                    >
                      <Globe size={11} />
                      {LANG_LABELS[code]}
                    </button>
                  )
                })}
              </div>
              <p className="mt-1 text-[10px] text-ink-faint">{t("settings.languageHelp")}</p>
            </Field>

            {/* ── Runtime & Isolation ── */}
            <CollapsibleSection
              title={t("settings.blkRuntime")}
              icon={<Cpu size={13} className="shrink-0 text-ink-faint" />}
              defaultOpen
            >
              <div className="space-y-3 px-4 py-3">
                <Field label={t("settings.maxParallel")}>
                  <input
                    type="number"
                    min={1}
                    max={32}
                    value={d.max_parallel}
                    onChange={(e) =>
                      setD({ ...d, max_parallel: parseInt(e.target.value || "1", 10) })
                    }
                    className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-accent"
                  />
                  <p className="mt-1 text-[10px] text-ink-faint">{t("settings.maxParallelHelp")}</p>
                </Field>
                <Field label={t("settings.maxTotalTasks")}>
                  <div className="text-sm font-mono text-ink-dim">
                    {d.max_total_tasks ?? "—"}
                    <span className="ml-2 text-[10px] uppercase tracking-wider text-ink-faint">
                      {t("settings.readOnly")}
                    </span>
                  </div>
                  <p className="mt-1 text-[10px] text-ink-faint">
                    {t("settings.maxTotalTasksHelp")}
                  </p>
                </Field>
                <Field label={t("settings.branchPrefix")}>
                  <input
                    value={d.branch_prefix}
                    onChange={(e) => setD({ ...d, branch_prefix: e.target.value })}
                    className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-accent"
                  />
                </Field>
                <SectionNote>{t("settings.isolationNote")}</SectionNote>
              </div>
            </CollapsibleSection>

            {/* ── Network & Secrets (no settings yet — honest) ── */}
            <CollapsibleSection
              title={t("settings.blkNetwork")}
              icon={<Network size={13} className="shrink-0 text-ink-faint" />}
            >
              <div className="px-4 py-3">
                <SectionNote>{t("settings.networkNote")}</SectionNote>
              </div>
            </CollapsibleSection>

            {/* ── Review Gates (read-only badges, NOT toggles) ── */}
            <CollapsibleSection
              title={t("settings.blkGates")}
              icon={<ShieldCheck size={13} className="shrink-0 text-ink-faint" />}
            >
              <div className="space-y-1 px-4 py-3">
                <GateRow
                  label={t("settings.gatePolicy")}
                  help={t("settings.gatePolicyHelp")}
                  on={!!d.gate_on_policy_violation}
                />
                <GateRow
                  label={t("settings.gateHighRisk")}
                  help={t("settings.gateHighRiskHelp")}
                  on={!!d.gate_on_high_risk}
                />
                <GateRow
                  label={t("settings.gateRefute")}
                  help={t("settings.gateRefuteHelp")}
                  on={!!d.refute_on_high_risk}
                />
                <GateRow
                  label={t("settings.autoPr")}
                  help={t("settings.autoPrHelp")}
                  on={!!d.auto_pr}
                />
                <SectionNote>{t("settings.gatesNote")}</SectionNote>
              </div>
            </CollapsibleSection>

            {/* ── Providers (agent + models editable; enforcement matrix read-only) ── */}
            <CollapsibleSection
              title={t("settings.blkProviders")}
              icon={<Server size={13} className="shrink-0 text-ink-faint" />}
              defaultOpen
            >
              <div className="space-y-3 px-4 py-3">
                <Field label={t("settings.defaultAgent")}>
                  <select
                    value={d.agent}
                    onChange={(e) => setD({ ...d, agent: e.target.value })}
                    className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-accent"
                  >
                    <option value="codex">codex</option>
                    <option value="cursor">cursor</option>
                    <option value="shell">shell</option>
                    <option value="mock">mock</option>
                  </select>
                  <p className="mt-1 text-[10px] text-ink-faint">{t("settings.defaultAgentHelp")}</p>
                </Field>
                <Field label={t("settings.agentModel")}>
                  <ModelInput
                    value={d.agent_model ?? d.cursor_model ?? ""}
                    models={models}
                    onChange={(v) => setD({ ...d, agent_model: v })}
                  />
                  <p className="mt-1 text-[10px] text-ink-faint">{t("settings.agentModelHelp")}</p>
                  <p className="mt-1 text-[10px] text-ink-faint">{t("settings.taskModelHint")}</p>
                </Field>
                <Field label={t("settings.taggerModel")}>
                  <ModelInput
                    value={d.tagger_model ?? ""}
                    models={models}
                    onChange={(v) => setD({ ...d, tagger_model: v })}
                  />
                  <p className="mt-1 text-[10px] text-ink-faint">{t("settings.taggerModelHelp")}</p>
                </Field>
                <button
                  onClick={refresh}
                  disabled={refreshing}
                  className="inline-flex items-center gap-1.5 text-[11px] text-ink-mute hover:text-ink"
                >
                  <RefreshCw size={11} className={refreshing ? "animate-spin" : ""} />
                  {t("settings.refreshModels")}
                  <span className="text-ink-faint ml-1">
                    {t("settings.modelsCached", { n: models.length })}
                  </span>
                </button>
                <div className="pt-1">
                  <div className="mb-1.5 text-[10px] uppercase tracking-wider text-ink-faint">
                    {t("settings.enforcementMatrix")}
                  </div>
                  {profilesLoading ? (
                    <p className="text-[11px] text-ink-faint">{t("common.loading")}</p>
                  ) : profilesErr ? (
                    // Never silently hide a security info surface (B1).
                    <p className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-[11px] text-status-warning">
                      {t("settings.matrixUnavailable")}
                    </p>
                  ) : (
                    <ProvidersMatrix profiles={profiles} />
                  )}
                </div>
              </div>
            </CollapsibleSection>

            {/* ── Evidence & Audit (read-only pointer) ── */}
            <CollapsibleSection
              title={t("settings.blkEvidence")}
              icon={<FileText size={13} className="shrink-0 text-ink-faint" />}
            >
              <div className="px-4 py-3">
                <SectionNote>{t("settings.evidenceNote")}</SectionNote>
              </div>
            </CollapsibleSection>

            {err && (
              <p className="text-xs text-status-danger bg-red-500/10 border border-red-500/30 rounded px-2 py-1">
                {err}
              </p>
            )}
          </div>
        )}

        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-line">
          <button
            onClick={onClose}
            className="px-3 py-1.5 rounded text-sm text-ink-dim hover:text-ink"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={save}
            disabled={!d || saving}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded text-sm bg-blue-600 hover:bg-blue-500 disabled:bg-bg-hover disabled:text-ink-faint"
          >
            <Save size={12} /> {saving ? t("common.saving") : t("common.save")}
          </button>
        </div>
      </div>
    </div>
  )
}

function Field({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div>
      <label className="block text-[10px] uppercase tracking-wider text-ink-faint mb-1">
        {label}
      </label>
      {children}
    </div>
  )
}

/** Honest explanatory text for blocks that have no (or future) settings. */
function SectionNote({ children }: { children: React.ReactNode }) {
  return <p className="text-[11px] leading-relaxed text-ink-faint">{children}</p>
}

/** Read-only gate status (F-136a2): shows enabled/disabled — NOT a toggle. The flags
 *  live in projects.yaml; web editing is a later cut. */
function GateRow({ label, help, on }: { label: string; help?: string; on: boolean }) {
  return (
    <div className="flex items-start justify-between gap-3 py-1">
      <div className="min-w-0">
        <div className="text-xs text-ink-dim">{label}</div>
        {help && <div className="text-[10px] text-ink-faint">{help}</div>}
      </div>
      <span
        className={`mt-0.5 inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 text-[10px] ${
          on
            ? "border-emerald-500/30 bg-emerald-500/10 text-status-success"
            : "border-line bg-bg-inset text-ink-faint"
        }`}
      >
        {on ? t("settings.enabled") : t("settings.disabled")}
      </span>
    </div>
  )
}

// F-136a2: enforcement tones — `soft` is AMBER (advisory), never a green "safe".
const ENF_TONE: Record<string, string> = {
  hard: "border-blue-500/30 bg-blue-500/10 text-status-info",
  soft: "border-amber-500/30 bg-amber-500/10 text-status-warning",
  unsupported: "border-red-500/30 bg-red-500/10 text-status-danger",
  not_applicable: "border-line bg-bg-inset text-ink-faint",
}
const ENF_LABEL: Record<string, string> = {
  hard: "hard",
  soft: "advisory",
  unsupported: "unsupported",
  not_applicable: "n/a",
}

function EnforcementBadge({ e }: { e: Enforcement }) {
  return (
    <span
      className={`inline-flex items-center rounded border px-1.5 py-0.5 text-[10px] ${
        ENF_TONE[e] ?? ENF_TONE.not_applicable
      }`}
    >
      {ENF_LABEL[e] ?? e}
    </span>
  )
}

const MATRIX_CAPS: (keyof ProviderEnforcementProfile)[] = [
  "shell",
  "git_write",
  "network",
  "fs_write",
  "external_dir",
  "mcp",
]

/** Per-provider hard/soft/advisory matrix (read-only). Soft is labelled "advisory";
 *  the post-run policy gate is the real check — never disguised as hard enforcement. */
function ProvidersMatrix({ profiles }: { profiles: ProviderEnforcementProfile[] }) {
  return (
    <div className="overflow-x-auto rounded border border-line">
      <table className="w-full border-collapse text-[10px]">
        <thead>
          <tr className="bg-bg-inset">
            <th className="px-1.5 py-1 text-left font-medium text-ink-faint">provider</th>
            {MATRIX_CAPS.map((c) => (
              <th key={c} className="px-1.5 py-1 text-left font-medium text-ink-faint">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {profiles.map((p) => (
            <tr key={p.provider_id} className="border-t border-line/60">
              <td className="px-1.5 py-1 font-mono text-ink-dim">{p.provider_id}</td>
              {MATRIX_CAPS.map((c) => (
                <td key={c} className="px-1.5 py-1">
                  <EnforcementBadge e={p[c] as Enforcement} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <p className="px-1.5 py-1.5 text-[10px] leading-relaxed text-ink-faint">
        {t("settings.matrixLegend")}
      </p>
    </div>
  )
}

/**
 * Custom model dropdown — the native `<datalist>` we used before can't be
 * height-capped, so a 100-entry account scrolled off the bottom of the
 * viewport. This is a filterable combo with a fixed-height scroll panel.
 */
function ModelInput({
  value,
  models,
  onChange,
}: {
  value: string
  models: ModelInfo[]
  onChange: (v: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState("")
  const wrapRef = useRef<HTMLDivElement>(null)

  // Close on outside click. We listen on `mousedown` so the panel can capture
  // its own clicks before the document handler fires.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener("mousedown", onDown)
    return () => document.removeEventListener("mousedown", onDown)
  }, [open])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return models
    return models.filter(
      (m) =>
        m.id.toLowerCase().includes(q) ||
        (m.label ?? "").toLowerCase().includes(q),
    )
  }, [models, query])

  const customModel = query.trim()
  const hasCustomModel = customModel.length > 0 && !models.some((m) => m.id === customModel)

  const selected = value
    ? models.find((m) => m.id === value)
    : undefined

  const display = value
    ? selected?.label
      ? `${value} · ${selected.label}`
      : value
    : t("common.empty")

  return (
    <div ref={wrapRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-accent hover:border-line-soft"
        aria-label={t("common.search")}
      >
        <span
          className={`flex-1 text-left truncate ${
            value ? "font-mono text-ink" : "text-ink-faint"
          }`}
        >
          {display}
        </span>
        <ChevronDown
          size={14}
          className={`text-ink-faint transition-transform ${
            open ? "rotate-180" : ""
          }`}
        />
      </button>

      {open && (
        <div className="absolute z-dropdown mt-1 left-0 right-0 bg-bg-panel border border-line rounded-md shadow-overlay overflow-hidden">
          <div className="flex items-center gap-2 px-2 py-1.5 border-b border-line/70">
            <Search size={12} className="text-ink-faint" />
      <input
        autoFocus
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={t("common.filterN", { n: models.length })}
        className="flex-1 bg-transparent text-xs focus:outline-none placeholder:text-ink-faint"
      />
          </div>

          <div className="max-h-72 overflow-y-auto scrollbar-thin py-1">
            <ModelRow
              isActive={value === ""}
              onPick={() => {
                onChange("")
                setOpen(false)
                setQuery("")
              }}
              id=""
              label={t("common.empty")}
              mono={false}
            />

            {filtered.length === 0 && (
              <div className="px-3 py-2 text-xs text-ink-faint">{t("docs.noMatches")}</div>
            )}

            {hasCustomModel && (
              <ModelRow
                isActive={value === customModel}
                onPick={() => {
                  onChange(customModel)
                  setOpen(false)
                  setQuery("")
                }}
                id={customModel}
                label={t("common.useCustomModel")}
                mono
              />
            )}

            {filtered.map((m) => (
              <ModelRow
                key={m.id}
                isActive={value === m.id}
                onPick={() => {
                  onChange(m.id)
                  setOpen(false)
                  setQuery("")
                }}
                id={m.id}
                label={m.label ?? ""}
                mono
              />
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

function ModelRow({
  isActive,
  onPick,
  id,
  label,
  mono,
}: {
  isActive: boolean
  onPick: () => void
  id: string
  label: string
  mono: boolean
}) {
  return (
    <button
      type="button"
      onClick={onPick}
      className={`w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left hover:bg-bg-hover ${
        isActive ? "bg-accent/10 text-accent" : "text-ink-dim"
      }`}
    >
      <Check
        size={11}
        className={isActive ? "text-blue-400" : "text-transparent"}
      />
      <span className={`flex-1 truncate ${mono ? "font-mono" : ""}`}>
        {id || label}
      </span>
      {id && label && (
        <span className="text-ink-faint text-[10px] truncate max-w-[40%]">
          {label}
        </span>
      )}
    </button>
  )
}
