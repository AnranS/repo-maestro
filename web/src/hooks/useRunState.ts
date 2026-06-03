import { useEffect, useMemo, useState } from "react"
import { api } from "../api"
import type { RunState, RunSummary } from "../types"

/** Per-status task tallies for the current run, or null when there is no run. */
export type RunTaskSummary = {
  running: number
  done: number
  failed: number
  pending: number
}

/**
 * Owns the live run state: bootstraps the current run + run history, subscribes
 * to the `/api/events` SSE stream for state pushes, and refreshes run history
 * every 30s. `connected` tracks the SSE link for the header's live indicator.
 */
export function useRunState() {
  const [state, setState] = useState<RunState | null>(null)
  const [runs, setRuns] = useState<RunSummary[]>([])
  const [connected, setConnected] = useState(false)

  // Bootstrap current run + history.
  useEffect(() => {
    api.state().then(setState).catch(() => {})
    api.runs().then(setRuns).catch(() => {})
  }, [])

  // SSE for state updates.
  useEffect(() => {
    const es = new EventSource("/api/events")
    es.addEventListener("state", (e: MessageEvent) => {
      try {
        const parsed = JSON.parse(e.data)
        // Server emits `null` when there is no current run. Defensively treat
        // an empty `{}` as "no run" too so the UI doesn't crash on stale data.
        if (!parsed || typeof parsed !== "object" || !("tasks" in parsed)) {
          setState(null)
        } else {
          setState(parsed)
        }
      } catch {}
      setConnected(true)
    })
    es.addEventListener("ping", () => setConnected(true))
    es.onerror = () => setConnected(false)
    return () => es.close()
  }, [])

  // Periodic refresh of run history.
  useEffect(() => {
    const t = setInterval(() => api.runs().then(setRuns).catch(() => {}), 30000)
    return () => clearInterval(t)
  }, [])

  // Polling fallback for run state when the SSE link is down. The state
  // normally arrives via `/api/events`; if SSE never connects or dies for
  // good, that push channel goes silent and the live view would freeze. While
  // `connected` is false, poll `api.state()` every ~7s (same shape as the run
  // history poll above) using the SSE setter's null-guarding, and stop as soon
  // as an event re-establishes the link (connected=true) or on unmount.
  useEffect(() => {
    if (connected) return
    const t = setInterval(() => {
      api
        .state()
        .then((parsed) => {
          if (!parsed || typeof parsed !== "object" || !("tasks" in parsed)) {
            setState(null)
          } else {
            setState(parsed)
          }
        })
        .catch(() => {})
    }, 7000)
    return () => clearInterval(t)
  }, [connected])

  const runSummary = useMemo<RunTaskSummary | null>(() => {
    if (!state || !state.tasks) return null
    const tasks = Object.values(state.tasks)
    const c = (s: string) => tasks.filter((t) => t.status === s).length
    return {
      running: c("running"),
      done: c("done"),
      failed: c("failed"),
      pending: c("pending") + c("awaiting_approval"),
    }
  }, [state])

  return { state, runs, connected, runSummary }
}
