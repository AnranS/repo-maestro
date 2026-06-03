import { lazy, Suspense } from "react"
import type { Components } from "react-markdown"

/**
 * Shared markdown configuration for chat, docs, memory, and skill panes.
 * The only special-case is a `code` override that detects fenced
 * ```mermaid``` blocks and renders them with the MermaidBlock component.
 *
 * Components that need this typically already configure remark / rehype
 * plugins themselves; we just hand them an extra `components={...}` map.
 */

const MermaidBlock = lazy(() =>
  import("./MermaidBlock").then((m) => ({ default: m.MermaidBlock })),
)

/**
 * Returns the `components` prop you can pass to <ReactMarkdown>. Other
 * overrides (like internal `<a>` link interception in DocsView) should
 * be merged on top by the caller using `{ ...markdownComponents, a: ... }`.
 */
export const markdownComponents: Partial<Components> = {
  code({ className, children, ...props }) {
    // react-markdown passes the language as `language-foo` in className.
    const language = /language-(\w+)/.exec(className || "")?.[1]
    if (language === "mermaid") {
      const source = String(children).trim()
      return (
        <Suspense
          fallback={
            <pre className="text-[11px] text-ink-faint p-2 bg-bg-inset rounded">
              loading diagram…
            </pre>
          }
        >
          <MermaidBlock source={source} />
        </Suspense>
      )
    }
    // Fall through to react-markdown's default <code> rendering with the
    // class still attached (rehype-highlight will pick it up).
    return (
      <code className={className} {...props}>
        {children}
      </code>
    )
  },
}
