import { Suspense, lazy, useEffect, useState } from "react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import rehypeHighlight from "rehype-highlight"
import { BookOpen, Pencil, Save, Trash2 } from "lucide-react"
import { api } from "../../api"
import { markdownComponents } from "../markdown"

/**
 * Right-pane view for one L1 fact file. Three modes:
 *   - loading: just spinner text
 *   - read:    rendered markdown (or raw mono for non-md), with edit/delete
 *   - edit:    CodeMirror via <Editor/>, with save/cancel
 *
 * Owns its own draft state; the parent only sees commit/delete signals.
 */
const Editor = lazy(() =>
  import("../Editor").then((m) => ({ default: m.Editor })),
)

export function MemoryDetail({
  topic,
  file,
  onSaved,
  onDeleted,
}: {
  topic: string
  file: string
  onSaved: () => void
  onDeleted: () => void
}) {
  const [original, setOriginal] = useState<string>("")
  const [draft, setDraft] = useState<string>("")
  const [edit, setEdit] = useState(false)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const isMarkdown = file.endsWith(".md")

  useEffect(() => {
    setLoading(true)
    api
      .memoryRead(topic, file)
      .then((content) => {
        setOriginal(content)
        setDraft(content)
      })
      .catch(() => {
        setOriginal("(failed to load)")
        setDraft("")
      })
      .finally(() => setLoading(false))
  }, [topic, file])

  const save = async () => {
    setBusy(true)
    try {
      await api.memorySave(topic, file, draft)
      setOriginal(draft)
      setEdit(false)
      onSaved()
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!confirm(`Delete ${topic}/${file}?`)) return
    await api.memoryDelete(topic, file)
    onDeleted()
  }

  return (
    <div className="p-6">
      <header className="mb-4 flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <div className="text-[10px] text-ink-faint uppercase tracking-wider mb-1">
            memory · {topic}
          </div>
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <BookOpen size={15} className="text-ink-dim" />
            {file}
          </h1>
          <p className="mt-1 text-xs text-ink-faint">
            Injected to any task whose project has{" "}
            <code className="bg-bg-inset px-1 rounded">
              memory_scope: [{topic}]
            </code>
            .
          </p>
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
                  setDraft(original)
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
        {loading ? (
          <div className="p-5 text-sm text-ink-faint">loading…</div>
        ) : edit ? (
          <Suspense fallback={<EditorFallback height="60vh" />}>
            <Editor value={draft} onChange={setDraft} filename={file} />
          </Suspense>
        ) : (
          <div className="p-5">
            {isMarkdown ? (
              <div className="prose-chat">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  rehypePlugins={[rehypeHighlight]}
                  components={markdownComponents}
                >
                  {original}
                </ReactMarkdown>
              </div>
            ) : (
              <pre className="text-[12.5px] font-mono whitespace-pre-wrap break-words text-ink-dim leading-relaxed">
                {original}
              </pre>
            )}
          </div>
        )}
      </div>
    </div>
  )
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
