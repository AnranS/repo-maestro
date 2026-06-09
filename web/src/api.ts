import type {
  AcceptVerdict,
  Action,
  AddProjectBody,
  ArchitectureView,
  CodeGraph,
  DeliveryListEntry,
  DeliveryView,
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
  ProviderEnforcementProfile,
  RunEvidence,
  RunMonitor,
  RunOutcome,
  RunReplay,
  RunState,
  RunStatusLite,
  RunSummary,
  RuntimeHealthReport,
  Session,
  SessionMeta,
  Skill,
  SkillInventory,
  SkillsByScope,
  TaskContextManifest,
  TaskDetail,
  TimelineEvent,
} from "./types"

async function json<T>(r: Response): Promise<T> {
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`)
  return r.json()
}

// F-120: a stable, path-safe event-stream consumer id for this browser profile.
// `webui-<hex>` is always path-safe (hex + a single hyphen). Two layers of
// stability:
//   1. localStorage-persisted id — stable across reloads (the normal case); a
//      tampered value failing the path-safe check is regenerated, never sent.
//   2. module-level fallback id — when localStorage is unavailable (private mode /
//      disabled), one id is minted per page session so the browser-local consumer
//      stays stable (and can still reuse its stored ack) instead of degrading to a
//      fresh anonymous consumer on every Events-tab mount.
const EVENT_CONSUMER_KEY = "maestro.eventConsumerId"
const PATH_SAFE_ID = /^[A-Za-z0-9_-]+$/
let fallbackEventConsumerId: string | null = null

/** Path-safe `webui-<hex>`; `crypto.randomUUID` → `getRandomValues` → Math.random. */
function makeConsumerId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `webui-${crypto.randomUUID()}`
  }
  const bytes = new Uint8Array(16)
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    crypto.getRandomValues(bytes)
  } else {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256)
  }
  return `webui-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`
}

export function eventConsumerId(): string {
  try {
    const existing = localStorage.getItem(EVENT_CONSUMER_KEY)
    if (existing && PATH_SAFE_ID.test(existing)) return existing
    const fresh = makeConsumerId()
    localStorage.setItem(EVENT_CONSUMER_KEY, fresh)
    return fresh
  } catch {
    // localStorage unavailable: reuse the per-session module fallback so this page
    // keeps ONE stable consumer id rather than a fresh one per call.
    if (!fallbackEventConsumerId) fallbackEventConsumerId = makeConsumerId()
    return fallbackEventConsumerId
  }
}

/** Balanced run-event stream URL for a browser consumer (F-120). */
export function runEventStreamUrl(runId: string, consumerId: string): string {
  const params = new URLSearchParams({ delivery: "balanced", consumer_id: consumerId })
  return `/api/runs/${encodeURIComponent(runId)}/events/stream?${params}`
}

export const api = {
  async state(): Promise<RunState | null> {
    const r = await fetch("/api/state")
    if (r.status === 404) return null
    return json(r)
  },

  /** F-118: read-only local runtime readiness (powers the Dashboard six rows). */
  async runtimeHealth(): Promise<RuntimeHealthReport> {
    return json(await fetch("/api/runtime/health"))
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
  async runMonitor(id: string = "current"): Promise<RunMonitor> {
    return json(await fetch(`/api/runs/${encodeURIComponent(id)}/monitor`))
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
  /**
   * F-120: record this browser consumer's monotonic event high-water. Best-effort
   * consumer state — a 409 (raced/backwards) or 500 must not break the live stream,
   * so callers ignore the rejection.
   */
  async ackEvents(runId: string, consumerId: string, highWaterSeq: number): Promise<void> {
    const r = await fetch(`/api/runs/${encodeURIComponent(runId)}/events/ack`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        consumer_id: consumerId,
        high_water_seq: highWaterSeq,
        delivery: "balanced",
      }),
    })
    if (!r.ok) throw new Error(`${r.status}`)
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
  async taskDetail(runId: string, task: string): Promise<TaskDetail> {
    return json(
      await fetch(
        `/api/runs/${encodeURIComponent(runId)}/tasks/${encodeURIComponent(task)}/detail`,
      ),
    )
  },
  // F-116: returns null when the task has no context manifest (e.g. verify/shell
  // tasks, or a task that never dispatched) — the panel treats that as empty,
  // not an error.
  async taskContext(
    runId: string,
    task: string,
  ): Promise<TaskContextManifest | null> {
    const r = await fetch(
      `/api/runs/${encodeURIComponent(runId)}/tasks/${encodeURIComponent(task)}/context`,
    )
    if (r.status === 404) return null
    return json(r)
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

  // F-128 read-only Delivery Web UI.
  async deliveries(): Promise<DeliveryListEntry[]> {
    return json(await fetch("/api/deliveries"))
  },
  async delivery(id: string): Promise<DeliveryView> {
    return json(await fetch(`/api/deliveries/${encodeURIComponent(id)}`))
  },
  // F-131: slim live run status for the Delivery detail. `run` is null when there is no
  // run (idle); a corrupt/missing linked run is a 500 (json() throws) — never silent idle.
  async deliveryRunStatus(id: string): Promise<{ run: RunStatusLite | null }> {
    return json(await fetch(`/api/deliveries/${encodeURIComponent(id)}/run-status`))
  },
  // F-132: read-only audit timeline. A corrupt delivery / audit-node inconsistency is a
  // 500 (json() throws) — never a silent empty timeline.
  async deliveryTimeline(id: string): Promise<{ events: TimelineEvent[] }> {
    return json(await fetch(`/api/deliveries/${encodeURIComponent(id)}/timeline`))
  },

  // F-129 Web mutation (forward actions). confirm-spec/plan return the updated
  // DeliveryView; run returns 202 {ok, pid} (the run executes detached).
  async deliveryConfirmSpec(id: string, by?: string): Promise<DeliveryView> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/confirm-spec`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ by }),
      }),
    )
  },
  async deliveryPlan(id: string, by?: string): Promise<DeliveryView> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/plan`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ by }),
      }),
    )
  },
  async deliveryRun(id: string, by?: string): Promise<{ ok: boolean; pid?: number }> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/run`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ by }),
      }),
    )
  },

  // F-130 Web mutation slice 2 (accept / closeout). Both reuse the SYNC store fns
  // and return the updated DeliveryView (the run-outcome gate is server-side).
  async deliveryAccept(
    id: string,
    body: {
      verdict: AcceptVerdict
      by?: string
      notes?: string
      debt?: string[]
      accept_failed_with_debt?: boolean
    },
  ): Promise<DeliveryView> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/accept`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    )
  },
  async deliveryCloseout(
    id: string,
    body: {
      commits?: string[]
      ci?: string[]
      reviews?: string[]
      doc_revisions?: string[]
      evidence?: string[]
      writeback?: boolean
      by?: string
    },
  ): Promise<DeliveryView> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/closeout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    )
  },
  // F-133: reopen a changes_requested delivery for rework (supersedes the prior round).
  async deliveryReopen(id: string, by?: string, reason?: string): Promise<DeliveryView> {
    return json(
      await fetch(`/api/deliveries/${encodeURIComponent(id)}/reopen`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ by, reason }),
      }),
    )
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
  // F-136a2: read-only per-provider enforcement matrix for the Settings Providers block.
  async providerProfiles(): Promise<ProviderEnforcementProfile[]> {
    return json(await fetch("/api/providers/profiles"))
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
  // F-121: read-only skill/profile visibility inventory (metadata only, never
  // bodies). Returns null on 404 (unknown project/profile) so a typo'd profile
  // is a neutral empty state, not a thrown error.
  async skillInventory(
    project?: string | null,
    profile?: string | null,
  ): Promise<SkillInventory | null> {
    const q = new URLSearchParams()
    if (project) q.set("project", project)
    if (profile) q.set("profile", profile)
    const qs = q.toString()
    const r = await fetch(`/api/skills/inventory${qs ? `?${qs}` : ""}`)
    if (r.status === 404) return null
    return json(r)
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
  /** F-119 idempotency key for this submission (a retry of the same turn must not
   *  append the user message / first-turn prelude twice). */
  turnId?: string,
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
          turn_id: turnId ?? undefined,
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
