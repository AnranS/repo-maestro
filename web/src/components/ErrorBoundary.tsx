import { Component, type ReactNode } from "react"
import { AlertTriangle, RefreshCw, Copy, Check } from "lucide-react"

/**
 * React's last-line defense: if a descendant throws during render (e.g. a
 * malformed action payload, an unexpected null), this boundary catches it
 * and shows a recoverable fallback instead of unmounting the whole tree
 * into a black screen. Each tab gets its own boundary so a crash in
 * TasksView can't take down ChatView.
 *
 * Why custom and not react-error-boundary: zero deps, ~80 LOC, and we can
 * style the fallback to match the rest of the dev-tool chrome.
 */
interface Props {
  /** Label shown in the fallback ("Something went wrong in <label>"). */
  label?: string
  /** When this value changes, the boundary auto-resets (e.g. on tab switch). */
  resetKey?: string | number
  children: ReactNode
}
interface State {
  error: Error | null
  copied: boolean
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, copied: false }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error }
  }

  componentDidCatch(error: Error, info: { componentStack?: string }) {
    // Keep the console message useful — devs hit this in DevTools first.
    // eslint-disable-next-line no-console
    console.error(`[ErrorBoundary${this.props.label ? ` · ${this.props.label}` : ""}]`, error, info)
  }

  componentDidUpdate(prev: Props) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null, copied: false })
    }
  }

  reset = () => this.setState({ error: null, copied: false })

  copy = () => {
    const err = this.state.error
    if (!err) return
    const text = `${err.name}: ${err.message}\n\n${err.stack ?? "(no stack)"}`
    navigator.clipboard.writeText(text).then(() => {
      this.setState({ copied: true })
      setTimeout(() => this.setState({ copied: false }), 1400)
    })
  }

  render() {
    const { error, copied } = this.state
    if (!error) return this.props.children

    const label = this.props.label ?? "this view"
    return (
      <div className="flex-1 flex items-center justify-center p-6 overflow-auto">
        <div className="max-w-xl w-full rounded-lg border border-red-500/30 bg-red-500/5 p-5">
          <div className="flex items-center gap-2 text-status-danger text-sm font-semibold">
            <AlertTriangle size={14} />
            Something went wrong in {label}
          </div>
          <div className="mt-1 text-[12px] text-ink-faint">
            The rest of the app is still running — switch tabs or retry.
          </div>
          <pre className="mt-3 text-[11px] leading-relaxed bg-bg-inset border border-line rounded-md p-2.5 overflow-x-auto whitespace-pre-wrap text-ink-dim">
            {error.name}: {error.message}
          </pre>
          <div className="mt-3 flex items-center gap-2">
            <button
              onClick={this.reset}
              className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded text-xs border border-line hover:bg-bg-hover"
            >
              <RefreshCw size={11} /> retry
            </button>
            <button
              onClick={this.copy}
              className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded text-xs border border-line hover:bg-bg-hover"
              title="copy full stack trace"
            >
              {copied ? (
                <>
                  <Check size={11} className="text-status-success" /> copied
                </>
              ) : (
                <>
                  <Copy size={11} /> copy details
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    )
  }
}
