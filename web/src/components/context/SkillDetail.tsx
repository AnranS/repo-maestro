import { Suspense, lazy, useEffect, useState } from "react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import rehypeHighlight from "rehype-highlight"
import { Pencil, Save, Sparkles, Trash2 } from "lucide-react"
import { api } from "../../api"
import type { Skill } from "../../types"
import { markdownComponents } from "../markdown"

/**
 * Right-pane view for one Skill. Editing operates on the *full* file body
 * (frontmatter + content), so we reconstruct the frontmatter from the API
 * response. Save re-fetches so the rendered frontmatter stays in sync
 * with the freshly-saved file on disk.
 */
const Editor = lazy(() =>
  import("../Editor").then((m) => ({ default: m.Editor })),
)

export function SkillDetail({
  scope,
  name,
  onSaved,
  onDeleted,
}: {
  scope: string
  name: string
  onSaved: () => void
  onDeleted: () => void
}) {
  const [skill, setSkill] = useState<Skill | null>(null)
  const [edit, setEdit] = useState(false)
  const [draft, setDraft] = useState("")
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    setEdit(false)
    api
      .skill(scope, name)
      .then((s) => {
        setSkill(s)
        setDraft(reconstructSkillFile(s))
      })
      .catch(() => setSkill(null))
  }, [scope, name])

  const save = async () => {
    setBusy(true)
    try {
      await api.saveSkill(scope, name, draft)
      const fresh = await api.skill(scope, name)
      setSkill(fresh)
      setDraft(reconstructSkillFile(fresh))
      setEdit(false)
      onSaved()
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!confirm(`Delete skill "${name}" from ${scope}?`)) return
    await api.deleteSkill(scope, name)
    onDeleted()
  }

  if (!skill) {
    return <div className="p-6 text-sm text-ink-faint">loading skill…</div>
  }

  return (
    <div className="p-6">
      <header className="mb-4 flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <div className="text-[10px] text-ink-faint uppercase tracking-wider mb-1">
            skill · {scope === "_global" ? "global" : scope}
          </div>
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <Sparkles size={15} className="text-purple-300" />
            {skill.name}
          </h1>
          {skill.description && (
            <p className="mt-1 text-sm text-ink-dim">{skill.description}</p>
          )}
          {skill.trigger && (
            <p className="mt-1 text-[11px] text-ink-faint">
              trigger:{" "}
              <code className="bg-bg-inset px-1 rounded">{skill.trigger}</code>
            </p>
          )}
        </div>

        <div className="flex items-center gap-2">
          {edit ? (
            <>
              <button
                onClick={save}
                disabled={busy}
                className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded text-xs bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-200 border border-emerald-500/30"
              >
                <Save size={11} /> save
              </button>
              <button
                onClick={() => {
                  setEdit(false)
                  setDraft(reconstructSkillFile(skill))
                }}
                className="px-2.5 py-1 rounded text-xs text-ink-faint hover:text-ink border border-line"
              >
                cancel
              </button>
            </>
          ) : (
            <>
              <button
                onClick={() => setEdit(true)}
                className="inline-flex items-center gap-1 px-2.5 py-1 rounded text-xs text-ink-dim hover:text-ink border border-line"
              >
                <Pencil size={11} /> edit
              </button>
              <button
                onClick={remove}
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-xs text-red-300 hover:bg-red-500/10 border border-line hover:border-red-500/30"
              >
                <Trash2 size={11} />
              </button>
            </>
          )}
        </div>
      </header>

      <div className="bg-bg-panel border border-line rounded-lg overflow-hidden">
        {edit ? (
          <Suspense fallback={<EditorFallback height="62vh" />}>
            <Editor
              value={draft}
              onChange={setDraft}
              filename={`${name}.md`}
              height="62vh"
            />
          </Suspense>
        ) : (
          <div className="p-5 prose-chat">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              rehypePlugins={[rehypeHighlight]}
              components={markdownComponents}
            >
              {skill.content}
            </ReactMarkdown>
          </div>
        )}
      </div>

      <p className="mt-2 text-[11px] text-ink-faint font-mono break-all">
        {skill.path}
      </p>
    </div>
  )
}

/**
 * The API returns description/trigger as parsed fields plus the body
 * separately; the editor wants the *whole* file (frontmatter included),
 * so we stitch a synthetic one back together for that view.
 */
function reconstructSkillFile(s: Skill): string {
  const desc = s.description ?? ""
  const trig = s.trigger ?? ""
  if (!desc && !trig) return s.content
  return `---\ndescription: ${desc}\ntrigger: ${trig}\n---\n\n${s.content}`
}

function EditorFallback({ height }: { height: string }) {
  return (
    <div
      style={{ height }}
      className="p-5 text-sm text-ink-faint bg-bg-inset"
    >
      loading editor…
    </div>
  )
}
