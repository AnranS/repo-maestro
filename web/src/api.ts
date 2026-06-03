import type {
  Action,
  AddProjectBody,
  ArchitectureView,
  CodeGraph,
  CodeGraphBuildResult,
  CodeGraphEngine,
  CodeSymbol,
  DefaultsConfig,
  DocsIndex,
  ExternalListing,
  Finding,
  FsListing,
  MailMessage,
  TaskDiff,
  TaskTrajectory,
  MemoryGraph,
  StarMap,
  MemoryHit,
  MemoryItemDetail,
  MemoryIndex,
  ModelInfo,
  ProjectMemoryView,
  RunEvidence,
  RunOutcome,
  RunReplay,
  RunState,
  RunSummary,
  Session,
  SessionMeta,
  Skill,
  SkillsByScope,
} from "./types"

async function json<T>(r: Response): Promise<T> {
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
  return r.json()
}

export const api = {
  async state(): Promise<RunState | null> {
    const r = await fetch("/api/state")
    if (r.status === 404) return null
    return json(r)
  },

  async runs(): Promise<RunSummary[]> {
    return json(await fetch("/api/runs"))
  },

  async run(id: string): Promise<RunState> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}`))
  },
  async runEvidence(id: string = "current"): Promise<RunEvidence> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}/evidence`))
  },
  async runFindings(id: string = "current"): Promise<Finding[]> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}/findings`))
  },
  async runReplay(id: string = "current"): Promise<RunReplay> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}/replay`))
  },
  async runPrBody(id: string = "current"): Promise<string> {
    const r = await fetch(`/api/runs/${encodeURIComponent(id)}/pr-body`)
    if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
    return r.text()
  },
  async cancelRun(id: string = "current"): Promise<void> {
    const r = await fetch(`/api/runs/${encodeURIComponent(id)}/cancel`, { method: "POST" })
    if (!r.ok && r.status !== 204) throw new Error(`${r.status} ${await r.text()}`)
  },
  async rerunRun(id: string): Promise<{ ok: boolean; pid?: number }> {
    const r = await fetch(`/api/runs/${encodeURIComponent(id)}/rerun`, { method: "POST" })
    if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
    return r.json()
  },

  async mailbox(): Promise<MailMessage[]> {
    return json(await fetch("/api/mailbox"))
  },
  async runApprove(
    runId: string,
    task: string,
    decision: "approve" | "reject",
  ): Promise<void> {
    const r = await fetch(`/api/runs/${encodeURIComponent(runId)}/approve`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ task, decision }),
    })
    if (!r.ok && r.status !== 204) throw new Error(`${r.status} ${await r.text()}`)
  },
  async runOutcome(id: string): Promise<RunOutcome> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}/outcome`))
  },
  async taskTrajectory(runId: string, task: string): Promise<TaskTrajectory> {
    return json(
      await fetch(
        `/api/runs/${encodeURIComponent(runId)}/tasks/${encodeURIComponent(task)}/trajectory`,
      ),
    )
  },
  async taskDiff(runId: string, task: string): Promise<TaskDiff> {
    return json(
      await fetch(
        `/api/runs/${encodeURIComponent(runId)}/tasks/${encodeURIComponent(task)}/diff`,
      ),
    )
  },
  async mailboxAnswer(id: string, keys: string[]): Promise<MailMessage> {
    return json(
      await fetch("/api/mailbox/answer", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id, keys }),
      }),
    )
  },

  async memorySearch(q: string, k = 30): Promise<MemoryHit[]> {
    const params = new URLSearchParams()
    if (q.trim()) params.set("q", q.trim())
    params.set("k", String(k))
    return json(await fetch(`/api/memory/search?${params.toString()}`))
  },

  async memoryItem(kind: string, id: string): Promise<MemoryItemDetail> {
    const params = new URLSearchParams({ kind, id })
    return json(await fetch(`/api/memory/item?${params.toString()}`))
  },

  async agentMemoryStatus(): Promise<{ connected: boolean; port: number }> {
    return json(await fetch("/api/memory/agent-status"))
  },
  async memoryGraph(): Promise<MemoryGraph> {
    return json(await fetch("/api/memory/graph"))
  },
  async memoryStarmap(): Promise<StarMap> {
    return json(await fetch("/api/memory/starmap"))
  },

  async codegraph(): Promise<CodeGraph> {
    return json(await fetch("/api/codegraph/graph"))
  },
  async codegraphFile(path: string): Promise<CodeSymbol[]> {
    return json(await fetch(`/api/codegraph/file?path=${encodeURIComponent(path)}`))
  },
  async codegraphEngines(): Promise<CodeGraphEngine[]> {
    return json(await fetch("/api/codegraph/engines"))
  },
  async codegraphBuild(engine: string): Promise<CodeGraphBuildResult> {
    return json(
      await fetch("/api/codegraph/build", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ engine }),
      }),
    )
  },

  async sessions(): Promise<SessionMeta[]> {
    return json(await fetch("/api/chat/sessions"))
  },

  async session(id: string): Promise<Session> {
    return json(await fetch(`/api/chat/sessions/${encodeURIComponent(id)}`))
  },

  async createSession(): Promise<Session> {
    return json(await fetch("/api/chat/sessions", { method: "POST" }))
  },

  async deleteSession(id: string): Promise<void> {
    await fetch(`/api/chat/sessions/${encodeURIComponent(id)}`, {
      method: "DELETE",
    })
  },

  async renameSession(id: string, title: string): Promise<Session> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      }),
    )
  },

  async setSessionTags(id: string, tags: string[]): Promise<Session> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}/tags`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ tags }),
      }),
    )
  },

  async autoTagSession(id: string): Promise<Session> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}/auto-tag`, {
        method: "POST",
      }),
    )
  },

  async externalSessions(source?: string): Promise<ExternalListing> {
    const q = source ? `?source=${encodeURIComponent(source)}` : ""
    return json(await fetch(`/api/external/sessions${q}`))
  },

  // ─── models ───
  async models(provider?: string | null): Promise<ModelInfo[]> {
    const q = provider ? `?provider=${encodeURIComponent(provider)}` : ""
    return json(await fetch(`/api/models${q}`))
  },
  async refreshModels(provider?: string | null): Promise<ModelInfo[]> {
    const q = provider ? `?provider=${encodeURIComponent(provider)}` : ""
    return json(await fetch(`/api/models/refresh${q}`, { method: "POST" }))
  },
  async defaults(): Promise<DefaultsConfig> {
    return json(await fetch("/api/settings/defaults"))
  },
  async updateDefaults(patch: Partial<DefaultsConfig>): Promise<DefaultsConfig> {
    return json(
      await fetch("/api/settings/defaults", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(patch),
      }),
    )
  },

  async setSessionModel(id: string, model: string | null): Promise<Session> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ cursor_model: model }),
      }),
    )
  },

  async setSessionProvider(id: string, provider: string | null): Promise<Session> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ chat_provider: provider }),
      }),
    )
  },

  async setCurrent(id: string): Promise<void> {
    await fetch("/api/chat/current", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    })
  },

  async current(): Promise<string | null> {
    const r = await json<{ id: string | null }>(await fetch("/api/chat/current"))
    return r.id
  },

  // ─── skills ───
  async skills(): Promise<SkillsByScope> {
    return json(await fetch("/api/skills"))
  },
  async skill(scope: string, name: string): Promise<Skill> {
    return json(
      await fetch(
        `/api/skills/${encodeURIComponent(scope)}/${encodeURIComponent(name)}`,
      ),
    )
  },
  async saveSkill(scope: string, name: string, content: string): Promise<void> {
    await fetch(
      `/api/skills/${encodeURIComponent(scope)}/${encodeURIComponent(name)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ content }),
      },
    )
  },
  async deleteSkill(scope: string, name: string): Promise<void> {
    await fetch(
      `/api/skills/${encodeURIComponent(scope)}/${encodeURIComponent(name)}`,
      { method: "DELETE" },
    )
  },

  // ─── memory ───
  async memory(): Promise<MemoryIndex> {
    return json(await fetch("/api/memory/l1"))
  },
  async memoryRead(topic: string, name: string): Promise<string> {
    const r = await fetch(
      `/api/memory/l1/${encodeURIComponent(topic)}/${encodeURIComponent(name)}`,
    )
    if (!r.ok) throw new Error(`${r.status}`)
    return r.text()
  },
  async memorySave(topic: string, name: string, content: string): Promise<void> {
    await fetch(
      `/api/memory/l1/${encodeURIComponent(topic)}/${encodeURIComponent(name)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ content }),
      },
    )
  },
  async memoryDelete(topic: string, name: string): Promise<void> {
    await fetch(
      `/api/memory/l1/${encodeURIComponent(topic)}/${encodeURIComponent(name)}`,
      { method: "DELETE" },
    )
  },

  // ─── architecture ───
  async architecture(): Promise<ArchitectureView> {
    return json(await fetch("/api/architecture"))
  },

  /**
   * Collapse older turns of a session into a single system-message summary
   * via the tagger model. Returns the freshly-compacted session.
   */
  async compactSession(id: string): Promise<{ session: Session; messages_summarized: number }> {
    return json(
      await fetch(`/api/chat/sessions/${encodeURIComponent(id)}/compact`, {
        method: "POST",
      }),
    )
  },

  // ─── docs ───
  async docsIndex(lang?: string): Promise<DocsIndex> {
    const q = lang ? `?lang=${encodeURIComponent(lang)}` : ""
    return json(await fetch(`/api/docs/index${q}`))
  },
  async docsPage(file: string, lang?: string): Promise<string> {
    const params = new URLSearchParams({ file })
    if (lang) params.set("lang", lang)
    const r = await fetch(`/api/docs/page?${params.toString()}`)
    if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
    return r.text()
  },

  // ─── projects ───
  async projects(): Promise<string[]> {
    const r = await json<{ projects: string[] }>(await fetch("/api/projects"))
    return r.projects
  },
  async projectMemory(name: string): Promise<ProjectMemoryView> {
    const r = await fetch(`/api/projects/${encodeURIComponent(name)}/memory`)
    return json(r)
  },

  async fsList(path?: string, hidden = false): Promise<FsListing> {
    const qs = new URLSearchParams()
    if (path) qs.set("path", path)
    if (hidden) qs.set("hidden", "1")
    const r = await fetch(`/api/fs/list?${qs.toString()}`)
    return json(r)
  },

  async addProject(body: AddProjectBody): Promise<void> {
    const r = await fetch("/api/projects", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
  },
  async deleteProject(name: string): Promise<void> {
    const r = await fetch(`/api/projects/${encodeURIComponent(name)}`, {
      method: "DELETE",
    })
    if (!r.ok && r.status !== 204) throw new Error(`${r.status} ${await r.text()}`)
  },

  async runAction(
    sessionId: string,
    actionId: string,
    decision: "approve" | "reject",
  ): Promise<Action> {
    return json(
      await fetch(
        `/api/chat/actions/${encodeURIComponent(sessionId)}/${encodeURIComponent(actionId)}`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ decision }),
        },
      ),
    )
  },
}

export type StreamCallbacks = {
  onMeta?: (e: { message_id: string; session_id: string }) => void
  onDelta?: (text: string) => void
  /**
   * Reasoning-mode tokens (Claude `-thinking-*`, Codex high-effort, etc.).
   * Emitted BEFORE the model commits to a visible answer. Use this to
   * show a "thinking…" indicator + optional expandable trace; do NOT
   * append to the final message.
   */
  onThinking?: (text: string) => void
  onDone?: (msg: any) => void
  onError?: (err: string) => void
}

/**
 * Send a chat message and stream the assistant's reply via fetch + ReadableStream.
 * Returns a function that aborts the stream.
 */
export function streamMessage(
  text: string,
  sessionId: string | null,
  cb: StreamCallbacks,
  model?: string | null,
  /** `plan` = analyse-only; `exec` (default) = full agency. */
  mode?: "plan" | "exec",
  provider?: string | null,
): () => void {
  const abort = new AbortController()
  void (async () => {
    try {
      const r = await fetch("/api/chat/messages", {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify({
          text,
          session_id: sessionId ?? undefined,
          model: model ?? undefined,
          mode: mode ?? undefined,
          provider: provider ?? undefined,
        }),
        signal: abort.signal,
      })
      if (!r.ok || !r.body) {
        cb.onError?.(`${r.status} ${await r.text().catch(() => "")}`)
        return
      }
      const reader = r.body.getReader()
      const decoder = new TextDecoder("utf-8")
      let buf = ""
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        buf += decoder.decode(value, { stream: true })
        let idx: number
        while ((idx = buf.indexOf("\n\n")) >= 0) {
          const chunk = buf.slice(0, idx)
          buf = buf.slice(idx + 2)
          handleSseChunk(chunk, cb)
        }
      }
    } catch (e: any) {
      if (e.name !== "AbortError") cb.onError?.(String(e))
    }
  })()
  return () => abort.abort()
}

function handleSseChunk(chunk: string, cb: StreamCallbacks) {
  let event = ""
  let data = ""
  for (const line of chunk.split("\n")) {
    if (line.startsWith("event:")) event = line.slice(6).trim()
    else if (line.startsWith("data:")) data += line.slice(5).trimStart()
  }
  if (!event || !data) return
  try {
    const parsed = JSON.parse(data)
    switch (event) {
      case "meta":
        cb.onMeta?.({ message_id: parsed.message_id, session_id: parsed.session_id })
        break
      case "delta":
        cb.onDelta?.(parsed.text)
        break
      case "thinking":
        cb.onThinking?.(parsed.text)
        break
      case "done":
        cb.onDone?.(parsed.message)
        break
      case "error":
        cb.onError?.(parsed.message)
        break
    }
  } catch {
    /* ignore non-JSON keepalive */
  }
}
