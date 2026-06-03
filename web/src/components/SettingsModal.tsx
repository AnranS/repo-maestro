import { useEffect, useMemo, useRef, useState } from "react"
import { X, Save, RefreshCw, ChevronDown, Check, Search, Globe } from "lucide-react"
import { api } from "../api"
import type { DefaultsConfig, ModelInfo } from "../types"
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
  const [saving, setSaving] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    api.defaults().then(setD).catch((e) => setErr(String(e)))
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
      className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-bg-panel border border-line rounded-xl w-full max-w-lg shadow-2xl"
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
          <div className="p-5 space-y-4">
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
              <p className="mt-1 text-[10px] text-ink-faint">
                {t("settings.languageHelp")}
              </p>
            </Field>

            <Field label={t("settings.defaultAgent")}>
              <select
                value={d.agent}
                onChange={(e) => setD({ ...d, agent: e.target.value })}
                className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600"
              >
                <option value="codex">codex</option>
                <option value="cursor">cursor</option>
                <option value="shell">shell</option>
                <option value="mock">mock</option>
              </select>
              <p className="mt-1 text-[10px] text-ink-faint">
                codex/cursor run agent tasks; shell is for verify commands; mock is for dry simulations.
              </p>
            </Field>

            <Field label={t("settings.agentModel")}>
              <ModelInput
                value={d.agent_model ?? d.cursor_model ?? ""}
                models={models}
                onChange={(v) => setD({ ...d, agent_model: v })}
              />
              <p className="mt-1 text-[10px] text-ink-faint">
                {t("settings.agentModelHelp")}
              </p>
              <p className="mt-1 text-[10px] text-ink-faint">
                {t("settings.taskModelHint")}
              </p>
            </Field>

            <Field label={t("settings.taggerModel")}>
              <ModelInput
                value={d.tagger_model ?? ""}
                models={models}
                onChange={(v) => setD({ ...d, tagger_model: v })}
              />
              <p className="mt-1 text-[10px] text-ink-faint">
                {t("settings.taggerModelHelp")}
              </p>
            </Field>

            <Field label={t("settings.branchPrefix")}>
              <input
                value={d.branch_prefix}
                onChange={(e) => setD({ ...d, branch_prefix: e.target.value })}
                className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
              />
            </Field>

            <Field label={t("settings.maxParallel")}>
              <input
                type="number"
                min={1}
                max={32}
                value={d.max_parallel}
                onChange={(e) =>
                  setD({ ...d, max_parallel: parseInt(e.target.value || "1", 10) })
                }
                className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
              />
            </Field>

            <div className="pt-2 border-t border-line/60">
              <button
                onClick={refresh}
                disabled={refreshing}
                className="inline-flex items-center gap-1.5 text-[11px] text-ink-mute hover:text-ink"
              >
                <RefreshCw
                  size={11}
                  className={refreshing ? "animate-spin" : ""}
                />
                {t("settings.refreshModels")}
                <span className="text-ink-faint ml-1">
                  {t("settings.modelsCached", { n: models.length })}
                </span>
              </button>
            </div>

            {err && (
              <p className="text-xs text-red-300 bg-red-500/10 border border-red-500/30 rounded px-2 py-1">
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
        className="w-full flex items-center gap-2 bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600 hover:border-line-soft"
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
        <div className="absolute z-50 mt-1 left-0 right-0 bg-bg-panel border border-line rounded-md shadow-2xl overflow-hidden">
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
        isActive ? "bg-blue-600/10 text-blue-300" : "text-ink-dim"
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
