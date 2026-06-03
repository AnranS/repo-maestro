import { useCallback, useEffect, useState } from "react"
import { api } from "../api"
import type { ModelInfo, Session, SessionMeta } from "../types"

/**
 * Owns chat session state (list + currently open session) and the model
 * registry, plus the session CRUD operations the sidebar and chat view drive.
 * Bootstraps the session list, models, and the server's "current" session on
 * mount.
 */
export function useSessions() {
  const [sessions, setSessions] = useState<SessionMeta[]>([])
  const [currentSession, setCurrentSession] = useState<Session | null>(null)
  const [models, setModels] = useState<ModelInfo[]>([])

  useEffect(() => {
    api.sessions().then(setSessions).catch(() => {})
    api.models().then(setModels).catch(() => {})
    api
      .current()
      .then((id) => (id ? api.session(id) : null))
      .then((s) => s && setCurrentSession(s))
      .catch(() => {})
  }, [])

  const reloadSessions = useCallback(async () => {
    const list = await api.sessions()
    setSessions(list)
  }, [])

  const openSession = useCallback(
    async (id: string) => {
      await api.setCurrent(id)
      const s = await api.session(id)
      setCurrentSession(s)
      await reloadSessions()
    },
    [reloadSessions],
  )

  const newSession = useCallback(async () => {
    const s = await api.createSession()
    setCurrentSession(s)
    await reloadSessions()
  }, [reloadSessions])

  const deleteSession = useCallback(
    async (id: string) => {
      await api.deleteSession(id)
      setCurrentSession((cur) => (cur?.id === id ? null : cur))
      await reloadSessions()
    },
    [reloadSessions],
  )

  const renameSession = useCallback(
    async (id: string, title: string) => {
      const updated = await api.renameSession(id, title)
      setCurrentSession((cur) => (cur?.id === id ? updated : cur))
      await reloadSessions()
    },
    [reloadSessions],
  )

  const refreshModels = useCallback(async () => {
    const m = await api.refreshModels()
    setModels(m)
  }, [])

  return {
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
  }
}
