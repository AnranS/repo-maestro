import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import rehypeHighlight from "rehype-highlight"
import { Bot, User, Copy, Check } from "lucide-react"
import { lazy, Suspense, useState } from "react"
import type { Action, Message } from "../types"

// Lazy so chat tab doesn't pay ~600KB of mermaid until a diagram actually
// arrives in a message.
const MermaidBlock = lazy(() =>
  import("./MermaidBlock").then((m) => ({ default: m.MermaidBlock })),
)
import { ActionCard } from "./ActionCard"

interface Props {
  message: Message
  streaming?: boolean
  onAction?: (action: Action, decision: "approve" | "reject") => void
}

export function MessageBubble({ message, streaming, onAction }: Props) {
  const isUser = message.role === "user"
  const [copied, setCopied] = useState(false)

  const copy = () => {
    navigator.clipboard
      ?.writeText(message.content)
      .then(() => {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      })
      .catch(() => {})
  }

  return (
    <div className={`flex gap-3 ${isUser ? "flex-row-reverse" : ""}`}>
      <div
        className={`shrink-0 w-7 h-7 rounded-full flex items-center justify-center ${
          isUser
            ? "bg-accent/20 border border-accent/40 text-accent"
            : "bg-bg-panel border border-line text-ink-dim"
        }`}
      >
        {isUser ? <User size={13} /> : <Bot size={13} />}
      </div>

      <div className={`flex-1 min-w-0 ${isUser ? "flex justify-end" : ""}`}>
        <div
          className={
            isUser
              ? "max-w-[85%] bg-bg-panel border border-line rounded-2xl rounded-tr-md px-4 py-2.5 text-sm whitespace-pre-wrap leading-relaxed"
              : "w-full"
          }
        >
          {/* Reasoning trace (collapsible) — shown above the answer when the
              model emitted one, so the user can peek without it dominating. */}
          {!isUser && message.thinking && message.thinking.trim().length > 0 && (
            <details className="mb-2 rounded-md border border-line/60 bg-bg-inset/40 text-[11px] text-ink-mute">
              <summary className="cursor-pointer select-none px-3 py-1.5 text-ink-faint hover:text-ink-dim">
                💭 thinking · {message.thinking.length.toLocaleString()} chars
              </summary>
              <pre className="px-3 pb-2 pt-1 whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-ink-mute">
                {message.thinking.trim()}
              </pre>
            </details>
          )}

          {isUser ? (
            message.content
          ) : (
            <div className="prose-chat">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                rehypePlugins={[rehypeHighlight]}
                components={{
                  // react-markdown v9 dropped the `inline` prop on the code
                  // renderer; the supported split now is: <pre> owns block
                  // code (always its only child is <code>), and the bare
                  // <code> renderer always means inline. Earlier we had this
                  // inverted, which wrapped every inline `name` in a full
                  // code-card and shredded prose readability — exactly what
                  // 高鹏 just hit. See react-markdown v9 release notes.
                  pre({ children }: any) {
                    // Sniff the inner <code>'s language + raw text. ReactMarkdown
                    // hands us one element child whose props.className carries
                    // the language and whose props.children is the highlight
                    // tree from rehypeHighlight (we re-use it for visual
                    // fidelity, and reconstruct the plain text for clipboard).
                    const codeEl: any = Array.isArray(children) ? children[0] : children
                    const className: string = codeEl?.props?.className ?? ""
                    const lang = /language-(\w+)/.exec(className)?.[1]
                    const innerChildren = codeEl?.props?.children
                    const flatten = (n: any): string => {
                      if (n == null) return ""
                      if (typeof n === "string") return n
                      if (Array.isArray(n)) return n.map(flatten).join("")
                      if (typeof n === "object" && n.props) return flatten(n.props.children)
                      return ""
                    }
                    const raw = flatten(innerChildren)
                    if (lang === "maestro-action") {
                      // Hide the raw block in favor of the action card rendered separately.
                      return (
                        <div className="my-2 px-3 py-2 rounded-md bg-blue-500/5 border border-blue-500/20 text-xs text-status-info font-mono">
                          ▸ <span className="text-status-info">maestro-action</span> — see action card below
                        </div>
                      )
                    }
                    if (lang === "mermaid") {
                      const source = raw.trim()
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
                    return (
                      <CodeBlock lang={lang} raw={raw} className={className}>
                        {innerChildren}
                      </CodeBlock>
                    )
                  },
                  code({ className, children, ...rest }: any) {
                    // Always inline — block code is intercepted by `pre` above.
                    return (
                      <code className={className} {...rest}>
                        {children}
                      </code>
                    )
                  },
                }}
              >
                {message.content || ""}
              </ReactMarkdown>
              {streaming && (
                <span className="caret-blink inline-block" aria-hidden />
              )}
            </div>
          )}
        </div>

        {message.actions && message.actions.length > 0 && (
          <div className={`mt-3 space-y-2 ${isUser ? "" : ""}`}>
            {message.actions.map((a) => (
              <ActionCard
                key={a.id}
                action={a}
                onDecide={(d) => onAction?.(a, d)}
              />
            ))}
          </div>
        )}

        {!isUser && !streaming && (
          <div className="mt-1 flex items-center gap-2 text-[11px] text-ink-faint">
            <button
              onClick={copy}
              className="inline-flex items-center gap-1 hover:text-ink-dim"
            >
              {copied ? <Check size={11} /> : <Copy size={11} />}
              {copied ? "copied" : "copy"}
            </button>
            <span>·</span>
            <span>{new Date(message.timestamp).toLocaleTimeString()}</span>
          </div>
        )}
      </div>
    </div>
  )
}

/**
 * Styled wrapper around a fenced code block: a header bar with the detected
 * language label + a copy-to-clipboard button (with a brief "copied!"
 * confirmation), then the highlighted code in a horizontally-scrollable pre.
 * 89% of technical docs include syntax-highlighted code with copy
 * affordances (2025 dev survey); this brings the chat in line.
 */
function CodeBlock({
  lang,
  raw,
  className,
  children,
}: {
  lang?: string
  raw: string
  className?: string
  children: React.ReactNode
}) {
  const [copied, setCopied] = useState(false)
  const onCopy = () => {
    navigator.clipboard.writeText(raw).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1400)
    })
  }
  return (
    <div className="my-2 rounded-md border border-line overflow-hidden bg-bg-inset">
      <div className="flex items-center justify-between px-2.5 py-1 bg-bg-panel border-b border-line text-[10px]">
        <span className="text-ink-faint font-mono uppercase tracking-wider">
          {lang || "code"}
        </span>
        <button
          type="button"
          onClick={onCopy}
          title="copy to clipboard"
          className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-ink-faint hover:text-ink-dim hover:bg-bg-hover"
        >
          {copied ? (
            <>
              <Check size={10} className="text-status-success" /> copied
            </>
          ) : (
            <>
              <Copy size={10} /> copy
            </>
          )}
        </button>
      </div>
      <pre className="overflow-x-auto p-2.5 text-[12px] leading-relaxed">
        <code className={className}>{children}</code>
      </pre>
    </div>
  )
}
