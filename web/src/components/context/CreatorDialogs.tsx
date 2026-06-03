import { useState } from "react"
import { api } from "../../api"

/**
 * "+ new skill" and "+ new memory" dialogs, plus the tiny `<Modal>` /
 * `<ModalActions>` / `<Field>` primitives they share. Kept in one file
 * because they're co-used and never imported separately.
 */

export function CreateSkillDialog({
  existingScopes,
  projects,
  onClose,
  onCreated,
}: {
  existingScopes: string[]
  projects: string[]
  onClose: () => void
  onCreated: (scope: string, name: string) => void
}) {
  const allScopes = Array.from(
    new Set(["_global", ...projects, ...existingScopes.filter((s) => s !== "_global")]),
  )
  const [scope, setScope] = useState(allScopes[0] || "_global")
  const [name, setName] = useState("")
  const [description, setDescription] = useState("")
  const [trigger, setTrigger] = useState("")
  const [busy, setBusy] = useState(false)

  const submit = async () => {
    if (!name.trim()) return
    setBusy(true)
    const safe = name.trim().replace(/[^a-zA-Z0-9_-]/g, "-")
    const template = `---\ndescription: ${description.trim()}\ntrigger: ${trigger.trim()}\n---\n\n# ${safe}\n\nWhen to use it / preconditions:\n\n- …\n\nSteps:\n\n1. …\n2. …\n\nOutputs:\n\n- …\n`
    try {
      await api.saveSkill(scope, safe, template)
      onCreated(scope, safe)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal onClose={onClose} title="New skill">
      <Field label="scope">
        <select
          value={scope}
          onChange={(e) => setScope(e.target.value)}
          className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600"
        >
          {allScopes.map((s) => (
            <option key={s} value={s}>
              {s === "_global" ? "global (all agents)" : s}
            </option>
          ))}
        </select>
        <p className="mt-1 text-[10px] text-ink-faint">
          global skills are visible to every agent; project-scoped skills only
          appear when that project's adapter runs (or in chat).
        </p>
      </Field>
      <Field label="name">
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="write-test"
          autoFocus
          className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
        />
      </Field>
      <Field label="description">
        <input
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="One-line summary the agent will see"
          className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600"
        />
      </Field>
      <Field label="trigger (optional)">
        <input
          value={trigger}
          onChange={(e) => setTrigger(e.target.value)}
          placeholder="phrase | another phrase | …"
          className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600"
        />
        <p className="mt-1 text-[10px] text-ink-faint">
          When any phrase appears in a chat message, this skill's full body is
          auto-injected for that turn.
        </p>
      </Field>
      <ModalActions onClose={onClose}>
        <button
          onClick={submit}
          disabled={busy || !name.trim()}
          className="px-3 py-1.5 rounded text-sm bg-blue-600 hover:bg-blue-500 disabled:bg-bg-hover disabled:text-ink-faint"
        >
          create
        </button>
      </ModalActions>
    </Modal>
  )
}

export function CreateMemoryDialog({
  existingTopics,
  onClose,
  onCreated,
}: {
  existingTopics: string[]
  onClose: () => void
  onCreated: (topic: string, name: string) => void
}) {
  const [topicMode, setTopicMode] = useState<"existing" | "new">(
    existingTopics.length > 0 ? "existing" : "new",
  )
  const [topic, setTopic] = useState(existingTopics[0] || "")
  const [newTopic, setNewTopic] = useState("")
  const [filename, setFilename] = useState("")
  const [busy, setBusy] = useState(false)

  const submit = async () => {
    const finalTopic = topicMode === "new" ? newTopic.trim() : topic.trim()
    const finalName = filename.trim()
    if (!finalTopic || !finalName) return
    setBusy(true)
    try {
      await api.memorySave(finalTopic, finalName, defaultMemoryTemplate(finalName))
      onCreated(finalTopic, finalName)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal onClose={onClose} title="New memory fact">
      <Field label="topic">
        <div className="flex gap-2">
          {existingTopics.length > 0 && (
            <button
              onClick={() => setTopicMode("existing")}
              className={`px-2 py-1 rounded text-xs ${
                topicMode === "existing"
                  ? "bg-bg-hover text-ink border border-line"
                  : "text-ink-faint border border-line/40"
              }`}
            >
              existing
            </button>
          )}
          <button
            onClick={() => setTopicMode("new")}
            className={`px-2 py-1 rounded text-xs ${
              topicMode === "new"
                ? "bg-bg-hover text-ink border border-line"
                : "text-ink-faint border border-line/40"
            }`}
          >
            new
          </button>
        </div>
        {topicMode === "existing" ? (
          <select
            value={topic}
            onChange={(e) => setTopic(e.target.value)}
            className="mt-2 w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm focus:outline-none focus:border-blue-600"
          >
            {existingTopics.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        ) : (
          <input
            value={newTopic}
            onChange={(e) => setNewTopic(e.target.value)}
            placeholder="api, schema, ux, ..."
            autoFocus
            className="mt-2 w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
          />
        )}
      </Field>
      <Field label="filename">
        <input
          value={filename}
          onChange={(e) => setFilename(e.target.value)}
          placeholder="rate-limit.md, schema.sql, conventions.yaml, …"
          className="w-full bg-bg-inset border border-line rounded px-2 py-1.5 text-sm font-mono focus:outline-none focus:border-blue-600"
        />
        <p className="mt-1 text-[10px] text-ink-faint">
          Use `.md` / `.yaml` / `.sql` etc. The extension picks the syntax
          highlighting.
        </p>
      </Field>
      <ModalActions onClose={onClose}>
        <button
          onClick={submit}
          disabled={busy}
          className="px-3 py-1.5 rounded text-sm bg-blue-600 hover:bg-blue-500 disabled:bg-bg-hover disabled:text-ink-faint"
        >
          create
        </button>
      </ModalActions>
    </Modal>
  )
}

function defaultMemoryTemplate(filename: string): string {
  if (filename.endsWith(".md")) {
    return `# ${filename.replace(/\.md$/, "")}\n\nDescribe the fact here. Bullets keep best when agents read it.\n\n- ...\n- ...\n`
  }
  if (filename.endsWith(".yaml") || filename.endsWith(".yml")) {
    return "# Replace with concrete facts\nkey: value\n"
  }
  return ""
}

function Modal({
  title,
  children,
  onClose,
}: {
  title: string
  children: React.ReactNode
  onClose: () => void
}) {
  return (
    <div
      className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-bg-panel border border-line rounded-xl w-full max-w-md p-5 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-base font-semibold mb-3">{title}</h2>
        <div className="space-y-3">{children}</div>
      </div>
    </div>
  )
}

function ModalActions({
  onClose,
  children,
}: {
  onClose: () => void
  children: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-end gap-2 pt-2">
      <button
        onClick={onClose}
        className="px-3 py-1.5 rounded text-sm text-ink-dim hover:text-ink"
      >
        cancel
      </button>
      {children}
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="block text-[10px] uppercase tracking-wider text-ink-faint mb-1">
        {label}
      </label>
      {children}
    </div>
  )
}
