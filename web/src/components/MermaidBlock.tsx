import { useEffect, useId, useRef, useState } from "react"

/**
 * Lazy-loaded mermaid renderer for ```mermaid``` code blocks.
 *
 * Why lazy: the `mermaid` bundle is ~600KB minified; loading it on every
 * page render is wasteful when most chat messages never have a diagram.
 * We `import("mermaid")` only when the first MermaidBlock mounts.
 *
 * Why this odd structure: mermaid renders by parsing source into SVG via
 * a singleton. We initialize it once (idempotent) and call render with a
 * per-block unique id.
 */

let mermaidPromise: Promise<typeof import("mermaid").default> | null = null
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import("mermaid").then((m) => m.default)
  }
  return mermaidPromise
}

function currentTheme(): "light" | "dark" {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark"
}

export function MermaidBlock({ source }: { source: string }) {
  const id = useId().replace(/:/g, "")
  const ref = useRef<HTMLDivElement>(null)
  const [error, setError] = useState<string | null>(null)
  const [theme, setTheme] = useState<"light" | "dark">(() => currentTheme())

  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(currentTheme()))
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    })
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    let cancelled = false
    loadMermaid()
      .then(async (mermaid) => {
        try {
          mermaid.initialize({
            startOnLoad: false,
            theme: theme === "light" ? "base" : "dark",
            securityLevel: "strict",
            themeVariables:
              theme === "light"
                ? {
                    background: "#ffffff",
                    primaryColor: "#f8fafc",
                    primaryTextColor: "#0f172a",
                    primaryBorderColor: "#cbd5e1",
                    lineColor: "#64748b",
                    edgeLabelBackground: "#ffffff",
                    clusterBkg: "#f1f5f9",
                    clusterBorder: "#cbd5e1",
                  }
                : {
                    background: "#161616",
                    primaryColor: "#161616",
                    primaryTextColor: "#ededed",
                    primaryBorderColor: "#262626",
                    lineColor: "#94a3b8",
                    edgeLabelBackground: "#111111",
                    clusterBkg: "#111111",
                    clusterBorder: "#262626",
                  },
          })
          const { svg } = await mermaid.render(`m${id}`, source)
          if (!cancelled && ref.current) {
            ref.current.innerHTML = svg
          }
        } catch (e: unknown) {
          if (!cancelled) {
            setError(e instanceof Error ? e.message : String(e))
          }
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e))
      })
    return () => {
      cancelled = true
    }
  }, [id, source, theme])

  if (error) {
    return (
      <pre className="bg-red-500/10 border border-red-500/30 text-status-danger text-xs p-2 rounded">
        {`mermaid error: ${error}`}
        {"\n\n"}
        {source}
      </pre>
    )
  }

  return (
    <div
      ref={ref}
      className="my-2 flex justify-center [&_svg]:max-w-full [&_svg]:h-auto"
    />
  )
}
