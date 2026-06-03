import { useEffect, useMemo, useState } from "react"
import {
  X,
  Folder,
  FileText,
  ChevronUp,
  Home,
  Briefcase,
  Eye,
  EyeOff,
  Check,
  AlertCircle,
} from "lucide-react"
import { api } from "../api"
import type { FsListing } from "../types"
import { t } from "../i18n"

interface Props {
  /** Where to start. `~`, `~/work`, or an absolute path. Falls back to `$HOME`. */
  initialPath?: string
  /** Title shown in the modal header. */
  title?: string
  /** Called with the absolute path the user confirms. */
  onSelect: (absolutePath: string) => void
  onClose: () => void
}

/**
 * Server-driven directory picker. The browser can't read absolute paths
 * for security reasons, so we ask the backend (`GET /api/fs/list`) to
 * walk the filesystem on our behalf — fine because `maestro ui` only
 * binds to localhost and the user already owns these files.
 *
 * UX rules:
 *   - One click on a directory descends into it.
 *   - "Up" button or any breadcrumb segment goes back.
 *   - "Use this folder" confirms the *current* path (not a selection).
 *   - Files are shown read-only so the user knows what's in there but
 *     can't pick one (maestro registers directories, not files).
 */
export function PathPicker({ initialPath, title, onSelect, onClose }: Props) {
  const [path, setPath] = useState<string>(initialPath ?? "~")
  const [listing, setListing] = useState<FsListing | null>(null)
  const [showHidden, setShowHidden] = useState(false)
  const [loading, setLoading] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setErr(null)
    api
      .fsList(path, showHidden)
      .then((res) => {
        if (cancelled) return
        setListing(res)
        // Replace the typed value with the canonical absolute path the
        // server resolved, so the breadcrumb is always trustworthy.
        if (res.path !== path) setPath(res.path)
      })
      .catch((e) => {
        if (cancelled) return
        setErr(String(e.message ?? e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
    // intentionally only re-run when the user navigates or toggles hidden
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, showHidden])

  const breadcrumbs = useMemo(() => splitToBreadcrumbs(listing?.path ?? path), [
    listing?.path,
    path,
  ])

  const handleDescend = (name: string) => {
    if (!listing) return
    const sep = listing.path.endsWith("/") ? "" : "/"
    setPath(listing.path + sep + name)
  }

  const handleUp = () => {
    if (listing?.parent) setPath(listing.parent)
  }

  const handleBreadcrumb = (idx: number) => {
    const slice = breadcrumbs.slice(0, idx + 1).join("/")
    setPath(slice.startsWith("/") ? slice : "/" + slice)
  }

  return (
    <div
      className="fixed inset-0 z-[60] bg-black/70 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-bg-panel border border-line rounded-xl w-full max-w-2xl shadow-2xl flex flex-col"
        onClick={(e) => e.stopPropagation()}
        style={{ maxHeight: "min(80vh, 600px)" }}
      >
        {/* header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-line shrink-0">
          <div className="flex items-center gap-2">
            <Folder size={14} className="text-blue-300" />
            <h2 className="text-sm font-semibold">
              {title ?? t("picker.title")}
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-ink-faint hover:text-ink"
            aria-label="close"
          >
            <X size={14} />
          </button>
        </div>

        {/* shortcuts + breadcrumb */}
        <div className="px-5 py-2.5 border-b border-line/60 space-y-2 shrink-0">
          <div className="flex items-center gap-1.5 text-[11px]">
            {listing?.home && (
              <Shortcut
                icon={<Home size={10} />}
                label={t("picker.home")}
                onClick={() => setPath(listing.home!)}
                active={listing.path === listing.home}
              />
            )}
            {listing?.workspace_root && (
              <Shortcut
                icon={<Briefcase size={10} />}
                label={t("picker.workspace")}
                onClick={() => setPath(listing.workspace_root!)}
                active={listing.path === listing.workspace_root}
              />
            )}
            <button
              type="button"
              onClick={() => setShowHidden((v) => !v)}
              className="ml-auto inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-ink-faint hover:text-ink-dim"
              title={t("picker.toggleHidden")}
            >
              {showHidden ? <Eye size={10} /> : <EyeOff size={10} />}
              {t("picker.hidden")}
            </button>
          </div>

          <div className="flex items-center gap-1 text-[11px] font-mono overflow-x-auto scrollbar-thin">
            <button
              type="button"
              onClick={handleUp}
              disabled={!listing?.parent}
              className="shrink-0 inline-flex items-center justify-center w-5 h-5 rounded hover:bg-bg-inset disabled:opacity-30"
              aria-label="up"
              title={t("picker.up")}
            >
              <ChevronUp size={11} />
            </button>
            {breadcrumbs.map((seg, i) => (
              <button
                key={i}
                type="button"
                onClick={() => handleBreadcrumb(i)}
                className="hover:underline text-ink-dim hover:text-ink"
              >
                {i === 0 ? seg || "/" : seg}
              </button>
            )).reduce<React.ReactNode[]>((acc, el, i) => {
              if (i > 0) acc.push(
                <span key={`sep-${i}`} className="text-ink-faint">
                  /
                </span>
              )
              acc.push(el)
              return acc
            }, [])}
          </div>
        </div>

        {/* entries */}
        <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin">
          {loading && (
            <p className="px-5 py-6 text-xs text-ink-faint">
              {t("common.loading")}
            </p>
          )}
          {err && (
            <p className="mx-5 my-4 text-xs text-red-300 bg-red-500/10 border border-red-500/30 rounded px-2.5 py-2 inline-flex items-start gap-2">
              <AlertCircle size={12} className="mt-0.5 shrink-0" />
              {err}
            </p>
          )}
          {!loading && !err && listing && (
            <ul className="divide-y divide-line/40">
              {listing.entries.length === 0 ? (
                <li className="px-5 py-4 text-[11px] text-ink-faint">
                  {t("picker.empty")}
                </li>
              ) : (
                listing.entries.map((e) => {
                  const isDir = e.kind === "dir" || e.kind === "symlink"
                  return (
                    <li key={e.name}>
                      <button
                        type="button"
                        onClick={isDir ? () => handleDescend(e.name) : undefined}
                        disabled={!isDir}
                        className={`w-full text-left px-5 py-1.5 flex items-center gap-2 text-xs font-mono ${
                          isDir
                            ? "hover:bg-bg-inset cursor-pointer"
                            : "text-ink-faint cursor-default"
                        }`}
                      >
                        {isDir ? (
                          <Folder
                            size={11}
                            className={
                              e.kind === "symlink"
                                ? "text-cyan-300"
                                : "text-blue-300"
                            }
                          />
                        ) : (
                          <FileText size={11} className="text-ink-faint" />
                        )}
                        <span className="truncate">{e.name}</span>
                        {e.kind === "symlink" && (
                          <span className="ml-1 text-[9px] text-ink-faint">
                            →
                          </span>
                        )}
                      </button>
                    </li>
                  )
                })
              )}
              {listing.truncated && (
                <li className="px-5 py-2 text-[10px] text-amber-300/80">
                  {t("picker.truncated")}
                </li>
              )}
            </ul>
          )}
        </div>

        {/* footer */}
        <div className="flex items-center justify-between gap-2 px-5 py-3 border-t border-line shrink-0">
          <code className="text-[11px] text-ink-dim truncate" title={listing?.path}>
            {listing?.path ?? path}
          </code>
          <div className="flex items-center gap-2 shrink-0">
            <button
              type="button"
              onClick={onClose}
              className="px-3 py-1.5 rounded text-sm text-ink-dim hover:text-ink"
            >
              {t("common.cancel")}
            </button>
            <button
              type="button"
              onClick={() => listing && onSelect(listing.path)}
              disabled={!listing}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded text-sm bg-blue-600 hover:bg-blue-500 disabled:bg-bg-hover disabled:text-ink-faint"
            >
              <Check size={12} />
              {t("picker.use")}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

function Shortcut({
  icon,
  label,
  onClick,
  active,
}: {
  icon: React.ReactNode
  label: string
  onClick: () => void
  active: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded ${
        active
          ? "bg-blue-500/15 text-blue-200 border border-blue-500/30"
          : "text-ink-faint hover:text-ink-dim hover:bg-bg-inset"
      }`}
    >
      {icon}
      {label}
    </button>
  )
}

/**
 * Split an absolute path into breadcrumb segments. `/` becomes `[""]`,
 * `/a/b/c` becomes `["", "a", "b", "c"]`. The empty first
 * segment is rendered as `/` by the caller.
 */
function splitToBreadcrumbs(p: string): string[] {
  if (!p) return [""]
  const parts = p.split("/")
  // `/foo/bar` → ["", "foo", "bar"]. Trailing empties (e.g. `/`) drop to
  // just [""].
  while (parts.length > 1 && parts[parts.length - 1] === "") parts.pop()
  if (parts.length === 0) return [""]
  return parts
}
