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
    mermaidPromise = import("mermaid").then((m) => {
      m.default.initialize({
        startOnLoad: false,
        // Match the dark UI; mermaid's "dark" theme is reasonable out of the box.
        theme: "dark",
        securityLevel: "strict",
        themeVariables: {
          background: "#161616",
        },
      })
      return m.default
    })
  }
  return mermaidPromise
}

export function MermaidBlock({ source }: { source: string }) {
  const id = useId().replace(/:/g, "")
  const ref = useRef<HTMLDivElement>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    loadMermaid()
      .then(async (mermaid) => {
        try {
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
  }, [id, source])

  if (error) {
    return (
      <pre className="bg-red-500/10 border border-red-500/30 text-red-300 text-xs p-2 rounded">
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
