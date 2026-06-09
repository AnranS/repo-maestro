import { Suspense, lazy, useEffect, useState } from "react"
import type { RunState } from "./types"
import { api } from "./api"
import { ChatView } from "./components/ChatView"
import { Dashboard } from "./components/Dashboard"
import { SessionSidebar } from "./components/SessionSidebar"
import { RunsSidebar } from "./components/RunsSidebar"
import { SettingsModal } from "./components/SettingsModal"
import { Header } from "./components/Header"
import { ErrorBoundary } from "./components/ErrorBoundary"
import { useLang } from "./i18n"
import { useHashTab } from "./hooks/useHashTab"
import { useRunState } from "./hooks/useRunState"
import { useSessions } from "./hooks/useSessions"

// Heavy panels: reactflow + dagre live in TasksView; CodeMirror lives in
// ContextView. Lazy-loading shaves ~600KB off the initial chunk.
const TasksView = lazy(() =>
  import("./components/TasksView").then((m) => ({ default: m.TasksView })),
)
const ContextView = lazy(() =>
  import("./components/ContextView").then((m) => ({ default: m.ContextView })),
)
const MemoryView = lazy(() =>
  import("./components/MemoryView").then((m) => ({ default: m.MemoryView })),
)
const ArchitectureView = lazy(() =>
  import("./components/ArchitectureView").then((m) => ({
    default: m.ArchitectureView,
  })),
)
const CodeGraphView = lazy(() =>
  import("./components/CodeGraphView").then((m) => ({ default: m.CodeGraphView })),
)
const DeliveriesView = lazy(() =>
  import("./components/DeliveriesView").then((m) => ({ default: m.DeliveriesView })),
)
const DocsView = lazy(() =>
  import("./components/DocsView").then((m) => ({ default: m.DocsView })),
)

export default function App() {
  // Subscribe to language changes so the whole tree re-renders on flip.
  useLang()

  const [theme, setTheme] = useState<"light" | "dark">(() => {
    const current = document.documentElement.dataset.theme
    return current === "light" ? "light" : "dark"
  })
  const [tab, setTab] = useHashTab()
  const { state, runs, connected, runSummary } = useRunState()
  const {
    sessions,
    currentSession,
    setCurrentSession,
    models,
    reloadSessions,
    openSession,
    newSession,
    deleteSession,
    renameSession,
    refreshModels,
  } = useSessions()

  const [settingsOpen, setSettingsOpen] = useState(false)
  const [goalOpen, setGoalOpen] = useState(false)

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.documentElement.classList.toggle("dark", theme === "dark")
    localStorage.setItem("maestro-theme", theme)
  }, [theme])

  // Selecting a run from the sidebar. When it's the live/current run we keep
  // showing the SSE-updated `state`; for any older run we fetch its frozen
  // snapshot via /api/runs/:id.
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [selectedState, setSelectedState] = useState<RunState | null>(null)
  const viewingHistorical = !!selectedRunId && selectedRunId !== state?.run_id
  useEffect(() => {
    if (!viewingHistorical) {
      setSelectedState(null)
      return
    }
    let cancelled = false
    api
      .run(selectedRunId!)
      .then((s) => !cancelled && setSelectedState(s))
      .catch(() => !cancelled && setSelectedState(null))
    return () => {
      cancelled = true
    }
  }, [viewingHistorical, selectedRunId])
  const displayState = viewingHistorical ? selectedState : state

  return (
    <div className="h-screen flex flex-col bg-bg text-ink">
      <Header
        tab={tab}
        setTab={setTab}
        theme={theme}
        onToggleTheme={() => setTheme((value) => (value === "dark" ? "light" : "dark"))}
        sessions={sessions}
        runSummary={runSummary}
        connected={connected}
        state={state}
        goalOpen={goalOpen}
        setGoalOpen={setGoalOpen}
        onOpenSettings={() => setSettingsOpen(true)}
      />

      {settingsOpen && (
        <SettingsModal
          models={models}
          onClose={() => setSettingsOpen(false)}
          onRefreshModels={refreshModels}
        />
      )}

      {/* BODY */}
      <main className="flex-1 flex min-h-0 overflow-hidden">
        {/* Each tab gets its own ErrorBoundary keyed off the current tab name
            so a crash in one view (a stale Suspense module, a malformed
            payload) can't take the whole app black. Switching tabs auto-
            resets the boundary so the next visit is fresh. */}
        {tab === "dashboard" && (
          <ErrorBoundary label="dashboard" resetKey={tab}>
            <div className="min-h-0 flex-1 overflow-auto">
              <Dashboard
                state={state}
                runs={runs}
                onOpenRun={(runId) => {
                  setSelectedRunId(runId ?? null)
                  setTab("tasks")
                }}
              />
            </div>
          </ErrorBoundary>
        )}
        {tab === "chat" && (
          <ErrorBoundary label="chat" resetKey={tab}>
            <div className="flex min-h-0 flex-1 flex-col md:flex-row">
              <SessionSidebar
                sessions={sessions}
                currentId={currentSession?.id ?? null}
                onOpen={openSession}
                onNew={newSession}
                onDelete={deleteSession}
                onRename={renameSession}
                onReload={reloadSessions}
              />
              <ChatView
                session={currentSession}
                onSessionUpdated={async () => {
                  if (currentSession) {
                    setCurrentSession(await api.session(currentSession.id))
                  }
                  await reloadSessions()
                }}
                onCreateIfMissing={async () => {
                  if (!currentSession) {
                    return newSession()
                      .then(() => api.current())
                      .then(async (id) => {
                        if (id) {
                          const s = await api.session(id)
                          setCurrentSession(s)
                          return s
                        }
                        return null
                      })
                  }
                  return currentSession
                }}
                liveRun={state}
                onOpenRun={(runId) => {
                  setSelectedRunId(runId)
                  setTab("tasks")
                }}
                onOpenDashboard={() => setTab("dashboard")}
              />
            </div>
          </ErrorBoundary>
        )}
        {tab === "tasks" && (
          <ErrorBoundary label="tasks" resetKey={tab}>
            <div className="flex min-h-0 flex-1 flex-col md:flex-row">
              <RunsSidebar
                runs={runs}
                selectedRunId={displayState?.run_id ?? null}
                liveRunId={state?.run_id ?? null}
                onSelect={setSelectedRunId}
              />
              <Suspense fallback={<LazyFallback />}>
                <TasksView
                  state={displayState}
                  onJumpToSession={async (sid) => {
                    await api.setCurrent(sid)
                    setCurrentSession(await api.session(sid))
                    await reloadSessions()
                    setTab("chat")
                  }}
                />
              </Suspense>
            </div>
          </ErrorBoundary>
        )}
        {tab === "context" && (
          <ErrorBoundary label="context" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <ContextView />
            </Suspense>
          </ErrorBoundary>
        )}
        {tab === "memory" && (
          <ErrorBoundary label="memory" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <MemoryView
                onJumpToRun={(runId) => {
                  setSelectedRunId(runId)
                  setTab("tasks")
                }}
              />
            </Suspense>
          </ErrorBoundary>
        )}
        {tab === "architecture" && (
          <ErrorBoundary label="architecture" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <ArchitectureView />
            </Suspense>
          </ErrorBoundary>
        )}
        {tab === "codegraph" && (
          <ErrorBoundary label="codegraph" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <CodeGraphView />
            </Suspense>
          </ErrorBoundary>
        )}
        {tab === "deliveries" && (
          <ErrorBoundary label="deliveries" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <DeliveriesView
                onOpenRun={(runId) => {
                  setSelectedRunId(runId)
                  setTab("tasks")
                }}
              />
            </Suspense>
          </ErrorBoundary>
        )}
        {tab === "docs" && (
          <ErrorBoundary label="docs" resetKey={tab}>
            <Suspense fallback={<LazyFallback />}>
              <DocsView />
            </Suspense>
          </ErrorBoundary>
        )}
      </main>
    </div>
  )
}

function LazyFallback() {
  return (
    <div className="flex-1 flex items-center justify-center text-ink-faint text-sm">
      loading…
    </div>
  )
}
