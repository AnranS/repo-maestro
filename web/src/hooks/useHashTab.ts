import { useEffect, useState } from "react"

export type Tab =
  | "chat"
  | "tasks"
  | "context"
  | "memory"
  | "architecture"
  | "codegraph"
  | "docs"

/**
 * The docs tab uses sub-routes (`#docs/<page-id>`), so the matcher accepts any
 * hash that *starts* with `docs/` as well as the bare tab names.
 */
export function tabFromHash(hash: string): Tab {
  const h = hash.replace(/^#/, "")
  if (
    h === "tasks" ||
    h === "context" ||
    h === "memory" ||
    h === "architecture" ||
    h === "codegraph" ||
    h === "chat"
  ) {
    return h
  }
  if (h === "docs" || h.startsWith("docs/")) return "docs"
  return "chat"
}

/**
 * Two-way bind the active tab to `window.location.hash`. The docs tab owns its
 * own sub-route (`#docs/<page-id>`): switching INTO docs leaves an existing
 * sub-route alone, switching AWAY replaces the whole hash. External hash
 * changes (e.g. `maestro doc` deep-linking to `#docs/quick-start`) flow back
 * into tab state.
 */
export function useHashTab(): [Tab, (tab: Tab) => void] {
  const [tab, setTab] = useState<Tab>(() => tabFromHash(window.location.hash))

  useEffect(() => {
    const current = window.location.hash.replace(/^#/, "")
    if (tab === "docs") {
      if (!current.startsWith("docs")) {
        window.location.hash = "docs"
      }
    } else if (current !== tab) {
      window.location.hash = tab
    }
  }, [tab])

  useEffect(() => {
    const onHash = () => setTab(tabFromHash(window.location.hash))
    window.addEventListener("hashchange", onHash)
    return () => window.removeEventListener("hashchange", onHash)
  }, [])

  return [tab, setTab]
}
