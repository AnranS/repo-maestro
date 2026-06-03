import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Book, ChevronRight, Search } from "lucide-react"
import ReactMarkdown from "react-markdown"
import rehypeHighlight from "rehype-highlight"
import remarkGfm from "remark-gfm"
import { api } from "../api"
import type { DocsIndex, DocsPage } from "../types"
import { LANG_LABELS, SUPPORTED_LANGS, setLang, t, useLang } from "../i18n"
import { markdownComponents } from "./markdown"

/**
 * Built-in user docs. The corpus lives in `docs/site/*.md`, is embedded into
 * the binary, and is fetched lazily through `/api/docs/*`. Routing is
 * URL-hash-based: `#docs/<page-id>` is bookmarkable and survives reloads.
 */
export function DocsView() {
  const lang = useLang()
  const [index, setIndex] = useState<DocsIndex | null>(null)
  const [pageId, setPageId] = useState<string>(() => parseHash())
  const [body, setBody] = useState<string>("")
  const [loading, setLoading] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const bodyScrollRef = useRef<HTMLDivElement>(null)

  // Reload the index whenever the language changes so titles flip in
  // place. Keeping `lang` in the dep array means flipping the picker
  // doesn't require a page reload.
  useEffect(() => {
    api.docsIndex(lang).then(setIndex).catch((e) => setErr(String(e)))
  }, [lang])

  // Sync state ↔ URL hash. We use `#docs/<id>` so the top-level tab router
  // already recognises this as the docs tab.
  useEffect(() => {
    const onHash = () => setPageId(parseHash())
    window.addEventListener("hashchange", onHash)
    return () => window.removeEventListener("hashchange", onHash)
  }, [])

  // Resolve current page, default to the first page in the first group
  const currentPage = useMemo<DocsPage | null>(() => {
    if (!index) return null
    for (const g of index.groups) {
      for (const p of g.pages) {
        if (p.id === pageId) return p
      }
    }
    return index.groups[0]?.pages[0] ?? null
  }, [index, pageId])

  // Load page body on selection change OR language flip.
  useEffect(() => {
    if (!currentPage) return
    setLoading(true)
    setErr(null)
    api
      .docsPage(currentPage.file, lang)
      .then((md) => setBody(md))
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false))
    bodyScrollRef.current?.scrollTo(0, 0)
  }, [currentPage?.file, lang])

  const navigate = useCallback((id: string) => {
    window.location.hash = `docs/${id}`
  }, [])

  if (err && !index) {
    return (
      <div className="flex-1 flex items-center justify-center text-sm text-red-300 p-6">
        {err}
      </div>
    )
  }
  if (!index) {
    return (
      <div className="flex-1 flex items-center justify-center text-ink-faint text-sm">
        {t("docs.loading")}
      </div>
    )
  }

  return (
    <div className="flex-1 flex min-h-0">
      <DocsSidebar
        index={index}
        currentId={currentPage?.id ?? null}
        query={query}
        onQueryChange={setQuery}
        onPick={navigate}
      />

      <div className="flex-1 flex flex-col min-h-0">
        {currentPage && (
          <header className="px-8 py-3 border-b border-line flex items-center gap-2 text-xs text-ink-faint">
            <Book size={12} />
            <span>{t("docs.crumb")}</span>
            <ChevronRight size={12} />
            <span className="text-ink-dim">
              {findGroup(index, currentPage.id)?.title ?? ""}
            </span>
            <ChevronRight size={12} />
            <span className="text-ink truncate">{currentPage.title}</span>
          </header>
        )}

        <div ref={bodyScrollRef} className="flex-1 overflow-y-auto scrollbar-thin">
          <article className="max-w-3xl mx-auto px-8 py-8 prose-chat">
            {loading && <p className="text-ink-faint">{t("docs.loading")}</p>}
            {err && <p className="text-red-300">{err}</p>}
            {!loading && !err && (
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                rehypePlugins={[rehypeHighlight]}
                components={{
                  // Inherit `code` from the shared markdown config (handles
                  // ```mermaid``` blocks) and add docs-specific overrides
                  // on top.
                  ...markdownComponents,
                  // Render in-doc links like `[Quick start](#docs/quick-start)`
                  // through the hash router so they don't trigger a page load.
                  a: ({ href, children, ...rest }) => {
                    const isInternal = href?.startsWith("#docs/")
                    if (isInternal) {
                      const target = href!.replace(/^#/, "")
                      return (
                        <a
                          href={`#${target}`}
                          onClick={(e) => {
                            e.preventDefault()
                            window.location.hash = target
                          }}
                          {...rest}
                        >
                          {children}
                        </a>
                      )
                    }
                    return (
                      <a href={href} target="_blank" rel="noreferrer noopener" {...rest}>
                        {children}
                      </a>
                    )
                  },
                }}
              >
                {body}
              </ReactMarkdown>
            )}
          </article>
        </div>
      </div>
    </div>
  )
}

function DocsSidebar({
  index,
  currentId,
  query,
  onQueryChange,
  onPick,
}: {
  index: DocsIndex
  currentId: string | null
  query: string
  onQueryChange: (q: string) => void
  onPick: (id: string) => void
}) {
  const q = query.trim().toLowerCase()
  const filteredGroups = useMemo(() => {
    if (!q) return index.groups
    return index.groups
      .map((g) => ({
        ...g,
        pages: g.pages.filter(
          (p) =>
            p.title.toLowerCase().includes(q) ||
            p.id.toLowerCase().includes(q),
        ),
      }))
      .filter((g) => g.pages.length > 0)
  }, [index.groups, q])

  const availableLangs = (index.languages ?? SUPPORTED_LANGS).filter((l) =>
    SUPPORTED_LANGS.includes(l as (typeof SUPPORTED_LANGS)[number]),
  ) as typeof SUPPORTED_LANGS
  const currentLang = useLang()

  return (
    <aside className="w-64 shrink-0 border-r border-line bg-bg-soft/30 flex flex-col min-h-0">
      <div className="px-3 pt-3 pb-2 border-b border-line/70">
        <div className="flex items-center justify-between gap-2 px-1">
          <div className="flex items-center gap-2 text-[11px] uppercase tracking-wider text-ink-faint font-medium">
            <Book size={11} /> {t("docs.title")}
          </div>
          {availableLangs.length > 1 && (
            <div className="flex items-center gap-0.5 bg-bg-inset border border-line/70 rounded p-0.5">
              {availableLangs.map((code) => (
                <button
                  key={code}
                  onClick={() => setLang(code)}
                  className={`px-1.5 py-0.5 text-[10px] rounded transition-colors ${
                    currentLang === code
                      ? "bg-blue-600/80 text-white"
                      : "text-ink-faint hover:text-ink"
                  }`}
                  title={LANG_LABELS[code]}
                >
                  {code.toUpperCase()}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="mt-2 relative">
          <Search
            size={11}
            className="absolute left-2 top-1/2 -translate-y-1/2 text-ink-faint"
          />
          <input
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder={t("docs.filter")}
            className="w-full bg-bg-inset border border-line rounded text-xs pl-6 pr-2 py-1 focus:outline-none focus:border-blue-600 placeholder:text-ink-faint"
          />
        </div>
      </div>

      <nav className="flex-1 overflow-y-auto scrollbar-thin py-2">
        {filteredGroups.length === 0 && (
          <div className="px-4 py-3 text-xs text-ink-faint">{t("docs.noMatches")}</div>
        )}
        {filteredGroups.map((g) => (
          <div key={g.id} className="mb-3">
            <div className="px-3 py-1 text-[10px] uppercase tracking-wider text-ink-faint">
              {g.title}
            </div>
            <ul>
              {g.pages.map((p) => {
                const active = p.id === currentId
                return (
                  <li key={p.id}>
                    <button
                      onClick={() => onPick(p.id)}
                      className={`w-full text-left px-3 py-1.5 text-[12.5px] truncate ${
                        active
                          ? "bg-blue-600/10 text-blue-300 border-l-2 border-blue-500 pl-[10px]"
                          : "text-ink-dim hover:text-ink hover:bg-bg-hover/70"
                      }`}
                    >
                      {p.title}
                    </button>
                  </li>
                )
              })}
            </ul>
          </div>
        ))}
      </nav>
    </aside>
  )
}

function findGroup(index: DocsIndex, pageId: string) {
  return index.groups.find((g) => g.pages.some((p) => p.id === pageId))
}

/** Extract the docs page id from the URL hash, e.g. `#docs/quick-start`. */
function parseHash(): string {
  const h = window.location.hash.replace(/^#/, "")
  if (h.startsWith("docs/")) return h.slice("docs/".length)
  return ""
}
