import { useState } from "react"
import { X, Save, FolderPlus, FolderOpen } from "lucide-react"
import { api } from "../api"
import type { AddProjectBody } from "../types"
import { PathPicker } from "./PathPicker"
import { t } from "../i18n"

const TYPES = ["", "backend", "frontend", "mobile", "tool", "library"] as const
const AGENTS = ["", "codex", "cursor", "shell", "mock"] as const

interface Props {
  onClose: () => void
  onCreated: (name: string) => void
  /** Names of existing projects so we can warn on collision before POST. */
  existing: string[]
}

export function AddProjectModal({ onClose, onCreated, existing }: Props) {
  const [form, setForm] = useState<AddProjectBody>({
    name: "",
    path: "",
    type: "",
    stack: [],
    agent: "",
    memory_scope: [],
    provides: "",
    consumes: "",
  })
  const [stackText, setStackText] = useState("")
  const [memoryText, setMemoryText] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [pickerOpen, setPickerOpen] = useState(false)

  const set = <K extends keyof AddProjectBody>(k: K, v: AddProjectBody[K]) =>
    setForm((prev) => ({ ...prev, [k]: v }))

  const nameTaken =
    form.name.trim().length > 0 && existing.includes(form.name.trim())

  const canSubmit =
    !submitting && form.name.trim().length > 0 && form.path.trim().length > 0 && !nameTaken

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!canSubmit) return
    setSubmitting(true)
    setErr(null)
    try {
      const payload: AddProjectBody = {
        name: form.name.trim(),
        path: form.path.trim(),
        type: form.type?.trim() || undefined,
        stack: tokenize(stackText),
        agent: form.agent?.trim() || undefined,
        memory_scope: tokenize(memoryText),
        provides: form.provides?.trim() || undefined,
        consumes: form.consumes?.trim() || undefined,
      }
      await api.addProject(payload)
      onCreated(form.name.trim())
    } catch (e: any) {
      setErr(String(e.message ?? e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <form
        className="bg-bg-panel border border-line rounded-xl w-full max-w-xl shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        onSubmit={submit}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-line">
          <div className="flex items-center gap-2">
            <FolderPlus size={14} className="text-blue-300" />
            <h2 className="text-sm font-semibold">Register a project</h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-ink-faint hover:text-ink"
          >
            <X size={14} />
          </button>
        </div>

        <div className="p-5 space-y-3.5">
          <Field label="name *" hint="kebab-case; becomes the project id in projects.yaml">
            <input
              autoFocus
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
              placeholder="notes-api"
              className={`w-full bg-bg-inset border rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none ${
                nameTaken
                  ? "border-red-500/60 focus:border-red-500"
                  : "border-line focus:border-blue-600"
              }`}
            />
            {nameTaken && (
              <p className="mt-1 text-[11px] text-red-300">
                a project named `{form.name.trim()}` already exists
              </p>
            )}
          </Field>

          <Field
            label="path *"
            hint="absolute, ~/path, or relative to the workspace; must already exist"
          >
            <div className="flex items-stretch gap-1.5">
              <input
                value={form.path}
                onChange={(e) => set("path", e.target.value)}
                placeholder="~/work/notes-api"
                className="flex-1 min-w-0 bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
              />
              <button
                type="button"
                onClick={() => setPickerOpen(true)}
                className="shrink-0 inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded text-xs bg-bg-inset border border-line hover:bg-bg-hover hover:text-ink"
                title={t("picker.browse")}
              >
                <FolderOpen size={11} />
                {t("picker.browse")}
              </button>
            </div>
          </Field>

          <div className="grid grid-cols-2 gap-3">
            <Field label="type">
              <select
                value={form.type ?? ""}
                onChange={(e) => set("type", e.target.value)}
                className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm focus:outline-none focus:border-blue-600"
              >
                {TYPES.map((t) => (
                  <option key={t || "_"} value={t}>
                    {t || "(unset)"}
                  </option>
                ))}
              </select>
            </Field>

            <Field label="agent">
              <select
                value={form.agent ?? ""}
                onChange={(e) => set("agent", e.target.value)}
                className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm focus:outline-none focus:border-blue-600"
              >
                {AGENTS.map((a) => (
                  <option key={a || "_"} value={a}>
                    {a || "(use defaults)"}
                  </option>
                ))}
              </select>
            </Field>
          </div>

          <Field label="stack" hint="comma- or space-separated tags">
            <input
              value={stackText}
              onChange={(e) => setStackText(e.target.value)}
              placeholder="python fastapi"
              className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
            />
          </Field>

          <Field
            label="memory_scope"
            hint="which L1 memory topics this project's tasks should auto-inject"
          >
            <input
              value={memoryText}
              onChange={(e) => setMemoryText(e.target.value)}
              placeholder="api schema"
              className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
            />
          </Field>

          <div className="grid grid-cols-2 gap-3">
            <Field
              label="provides"
              hint="contract file this project authors (e.g. schemas/openapi.yaml)"
            >
              <input
                value={form.provides ?? ""}
                onChange={(e) => set("provides", e.target.value)}
                placeholder="schemas/openapi.yaml"
                className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
              />
            </Field>

            <Field
              label="consumes"
              hint="contract file this project depends on"
            >
              <input
                value={form.consumes ?? ""}
                onChange={(e) => set("consumes", e.target.value)}
                placeholder="schemas/openapi.yaml"
                className="w-full bg-bg-inset border border-line rounded px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
              />
            </Field>
          </div>

          {err && (
            <p className="text-xs text-red-300 bg-red-500/10 border border-red-500/30 rounded px-2.5 py-1.5">
              {err}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-line">
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 rounded text-sm text-ink-dim hover:text-ink"
          >
            cancel
          </button>
          <button
            type="submit"
            disabled={!canSubmit}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded text-sm bg-blue-600 hover:bg-blue-500 disabled:bg-bg-hover disabled:text-ink-faint"
          >
            <Save size={12} />
            {submitting ? "adding…" : "add project"}
          </button>
        </div>
      </form>

      {pickerOpen && (
        <PathPicker
          initialPath={form.path || undefined}
          title={t("picker.title")}
          onSelect={(p) => {
            set("path", p)
            setPickerOpen(false)
          }}
          onClose={() => setPickerOpen(false)}
        />
      )}
    </div>
  )
}

function tokenize(raw: string): string[] {
  return raw
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
}

function Field({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div>
      <label className="block text-[10px] uppercase tracking-wider text-ink-faint mb-1">
        {label}
      </label>
      {children}
      {hint && <p className="mt-1 text-[10px] text-ink-faint">{hint}</p>}
    </div>
  )
}
