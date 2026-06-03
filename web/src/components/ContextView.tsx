import { Suspense, lazy, useEffect, useMemo, useState } from "react"
import { Brain, Globe, Box, Plus, Sparkles, Filter } from "lucide-react"
import { api } from "../api"
import type { MemoryIndex, ProjectMemoryView, SkillsByScope } from "../types"
import { Group, Item, Section } from "./context/Sidebar"

/**
 * Context tab orchestrator. Owns the cross-cutting state (which item is
 * selected, which creator dialog is open, the cached memory + skills
 * listings) and delegates everything else:
 *
 *   - `./context/Sidebar`    — the left rail bits (Section / Group / Item)
 *   - `./context/MemoryDetail`  — right-pane read/edit for a memory file
 *   - `./context/SkillDetail`   — right-pane read/edit for a skill
 *   - `./context/CreatorDialogs` — modal flows for "+ new" actions
 */

type SelectionKind = "memory" | "skill"
interface Selection {
  kind: SelectionKind
  scope: string // memory: topic; skill: scope dir
  name: string
}

const MemoryDetail = lazy(() =>
  import("./context/MemoryDetail").then((m) => ({ default: m.MemoryDetail })),
)
const SkillDetail = lazy(() =>
  import("./context/SkillDetail").then((m) => ({ default: m.SkillDetail })),
)
const CreateMemoryDialog = lazy(() =>
  import("./context/CreatorDialogs").then((m) => ({
    default: m.CreateMemoryDialog,
  })),
)
const CreateSkillDialog = lazy(() =>
  import("./context/CreatorDialogs").then((m) => ({
    default: m.CreateSkillDialog,
  })),
)

export function ContextView() {
  const [memory, setMemory] = useState<MemoryIndex>({})
  const [skills, setSkills] = useState<SkillsByScope>({})
  const [projects, setProjects] = useState<string[]>([])
  const [selected, setSelected] = useState<Selection | null>(null)
  const [creatorOpen, setCreatorOpen] = useState<"skill" | "memory" | null>(null)

  /**
   * When set, the sidebar shows only what's IN SCOPE for this project:
   *   - memory topics whose names appear in the project's `memory_scope`
   *   - L2 decisions archived under `<project>/`
   *   - skills scoped to `_global` (always relevant) and this project
   * Plus a "L2 decisions" section that only exists when filtering, since
   * L2 is project-keyed and otherwise has no natural sidebar home.
   */
  const [projectFilter, setProjectFilter] = useState<string | null>(null)
  const [projectView, setProjectView] = useState<ProjectMemoryView | null>(null)

  const refresh = async () => {
    try {
      const [m, s, p] = await Promise.all([api.memory(), api.skills(), api.projects()])
      setMemory(m)
      setSkills(s)
      setProjects(p)
    } catch (e) {
      console.error(e)
    }
  }

  useEffect(() => {
    refresh()
  }, [])

  // When a project filter is set, fetch its aggregate so we know its
  // memory_scope (for filtering L1 topics) and can show L2 decisions.
  useEffect(() => {
    let cancelled = false
    if (!projectFilter) {
      setProjectView(null)
      return
    }
    api
      .projectMemory(projectFilter)
      .then((v) => {
        if (!cancelled) setProjectView(v)
      })
      .catch((e) => {
        if (!cancelled) console.error("project memory:", e)
      })
    return () => {
      cancelled = true
    }
  }, [projectFilter])

  // Auto-select the first item on load so the right pane is never blank
  // when there's *something* to show.
  useEffect(() => {
    if (selected) return
    const topics = Object.keys(memory)
    for (const t of topics) {
      const files = memory[t]
      if (files && files.length > 0) {
        setSelected({ kind: "memory", scope: t, name: files[0] })
        return
      }
    }
    for (const sc of Object.keys(skills)) {
      if (skills[sc] && skills[sc].length > 0) {
        setSelected({ kind: "skill", scope: sc, name: skills[sc][0].name })
        return
      }
    }
  }, [memory, skills, selected])

  // When projectFilter is set, restrict topics to its memory_scope.
  // Topics outside the scope are intentionally hidden so the user can
  // see what the project's tasks ACTUALLY get injected.
  const memoryTopics = useMemo(() => {
    const all = Object.keys(memory).sort()
    if (!projectFilter || !projectView) return all
    const scope = new Set(projectView.memory_scope)
    if (scope.size === 0) return [] // no scope means: this project sees no L1
    return all.filter((t) => scope.has(t))
  }, [memory, projectFilter, projectView])

  // Skills are scoped to `_global` (always visible) and a specific
  // project name. When filtering, hide skills from unrelated projects.
  const visibleSkillScopes = useMemo(() => {
    const all = Object.keys(skills).sort((a, b) =>
      a === "_global" ? -1 : b === "_global" ? 1 : a.localeCompare(b),
    )
    if (!projectFilter) return all
    return all.filter((s) => s === "_global" || s === projectFilter)
  }, [skills, projectFilter])

  return (
    <div className="flex-1 flex min-h-0">
      <aside className="w-72 shrink-0 flex flex-col border-r border-line bg-bg-soft overflow-y-auto scrollbar-thin">
        {projects.length > 0 && (
          <div className="px-3 py-2 border-b border-line/60 flex items-center gap-2">
            <Filter size={11} className="text-ink-faint shrink-0" />
            <select
              value={projectFilter ?? ""}
              onChange={(e) => setProjectFilter(e.target.value || null)}
              className="flex-1 bg-bg-inset border border-line rounded px-1.5 py-0.5 text-[11px] font-mono focus:outline-none focus:border-blue-600"
            >
              <option value="">all projects</option>
              {projects.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </div>
        )}
        <Section
          icon={<Brain size={13} />}
          label="memory · l1 facts"
          right={
            <button
              onClick={() => setCreatorOpen("memory")}
              className="text-ink-faint hover:text-ink"
              title="new memory file"
            >
              <Plus size={12} />
            </button>
          }
        >
          {memoryTopics.length === 0 ? (
            <p className="px-3 py-1 text-[11px] text-ink-faint">
              No facts yet. Click + or run{" "}
              <code className="bg-bg-inset px-1 rounded">maestro memory add</code>.
            </p>
          ) : (
            memoryTopics.map((topic) => (
              <Group key={topic} label={topic}>
                {memory[topic].map((file) => (
                  <Item
                    key={file}
                    label={file}
                    selected={
                      selected?.kind === "memory" &&
                      selected.scope === topic &&
                      selected.name === file
                    }
                    onClick={() =>
                      setSelected({ kind: "memory", scope: topic, name: file })
                    }
                  />
                ))}
              </Group>
            ))
          )}
        </Section>

        {projectFilter && projectView && projectView.l2_decisions.length > 0 && (
          <Section
            icon={<Box size={13} />}
            label={`L2 decisions · ${projectFilter}`}
          >
            <Group label={projectFilter}>
              {projectView.l2_decisions.map((d) => (
                <Item
                  key={d.file}
                  label={d.file}
                  sub={`${Math.ceil(d.bytes / 1024)} KB`}
                  selected={false}
                  onClick={() => {
                    // Open the file in a new tab as a quick way to view —
                    // a dedicated detail pane for L2 is a follow-up.
                    window.open(
                      `/api/projects/${encodeURIComponent(projectFilter)}/memory`,
                      "_blank",
                    )
                  }}
                />
              ))}
            </Group>
          </Section>
        )}

        <Section
          icon={<Sparkles size={13} />}
          label="skills · playbooks"
          right={
            <button
              onClick={() => setCreatorOpen("skill")}
              className="text-ink-faint hover:text-ink"
              title="new skill"
            >
              <Plus size={12} />
            </button>
          }
        >
          {visibleSkillScopes
            .map((scope) => [scope, skills[scope] ?? []] as const)
            // Hide empty per-project scopes so a 65-project workspace doesn't
            // bury the real skills under dozens of "(empty)" rows. Always keep
            // the global scope visible (it's the canonical playbook home, even
            // when temporarily empty).
            .filter(([scope, list]) => scope === "_global" || list.length > 0)
            .map(([scope, list]) => (
              <Group
                key={scope}
                label={scope === "_global" ? "global" : scope}
                icon={scope === "_global" ? <Globe size={9} /> : <Box size={9} />}
              >
                {list.length === 0 ? (
                  <div className="px-3 py-1 text-[11px] text-ink-faint">(empty)</div>
                ) : (
                  list.map((s) => (
                    <Item
                      key={s.name}
                      label={s.name}
                      sub={s.description || undefined}
                      selected={
                        selected?.kind === "skill" &&
                        selected.scope === scope &&
                        selected.name === s.name
                      }
                      onClick={() =>
                        setSelected({ kind: "skill", scope, name: s.name })
                      }
                    />
                  ))
                )}
              </Group>
            ))}
        </Section>
      </aside>

      <div className="flex-1 overflow-y-auto scrollbar-thin">
        {selected ? (
          <Suspense fallback={<ContextPaneFallback />}>
            {selected.kind === "memory" ? (
              <MemoryDetail
                key={`mem:${selected.scope}:${selected.name}`}
                topic={selected.scope}
                file={selected.name}
                onSaved={refresh}
                onDeleted={async () => {
                  setSelected(null)
                  await refresh()
                }}
              />
            ) : (
              <SkillDetail
                key={`skill:${selected.scope}:${selected.name}`}
                scope={selected.scope}
                name={selected.name}
                onSaved={refresh}
                onDeleted={async () => {
                  setSelected(null)
                  await refresh()
                }}
              />
            )}
          </Suspense>
        ) : (
          <div className="h-full flex items-center justify-center text-ink-faint text-sm">
            Select a fact or a skill on the left.
          </div>
        )}
      </div>

      {creatorOpen === "skill" && (
        <Suspense fallback={<DialogFallback />}>
          <CreateSkillDialog
            existingScopes={Object.keys(skills)}
            projects={projects}
            onClose={() => setCreatorOpen(null)}
            onCreated={async (scope, name) => {
              setCreatorOpen(null)
              await refresh()
              setSelected({ kind: "skill", scope, name })
            }}
          />
        </Suspense>
      )}
      {creatorOpen === "memory" && (
        <Suspense fallback={<DialogFallback />}>
          <CreateMemoryDialog
            existingTopics={memoryTopics}
            onClose={() => setCreatorOpen(null)}
            onCreated={async (topic, name) => {
              setCreatorOpen(null)
              await refresh()
              setSelected({ kind: "memory", scope: topic, name })
            }}
          />
        </Suspense>
      )}
    </div>
  )
}

function ContextPaneFallback() {
  return (
    <div className="h-full flex items-center justify-center text-ink-faint text-sm">
      loading…
    </div>
  )
}

function DialogFallback() {
  return (
    <div className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4">
      <div className="bg-bg-panel border border-line rounded-xl px-4 py-3 text-sm text-ink-faint">
        loading…
      </div>
    </div>
  )
}
