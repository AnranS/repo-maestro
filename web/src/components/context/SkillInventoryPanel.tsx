import { useEffect, useMemo, useState, type ReactNode } from "react"
import { Layers, Globe, Box, AlertTriangle } from "lucide-react"
import { api } from "../../api"
import type { InventoryIssue, RefResolution, SkillInventory } from "../../types"

/**
 * F-121 — read-only skill/profile visibility inventory for the Context tab.
 * Shows what a project (and, optionally, a specialist profile) can see, using
 * the `/api/skills/inventory` projection. Metadata only — it never displays a
 * skill body; the skill editor (SkillDetail) is untouched. A missing manifest /
 * empty scope is a calm empty state, not an error.
 */

const fmtKB = (b: number) => `${(b / 1024).toFixed(1)} KB`

const RES_STYLE: Record<RefResolution, string> = {
  resolved: "bg-emerald-500/10 text-status-success border-emerald-500/25",
  missing: "bg-red-500/10 text-status-danger border-red-500/25",
  invalid: "bg-amber-500/10 text-status-warning border-amber-500/25",
  deferred: "bg-bg-inset text-ink-faint border-line",
}

function Chip({ children, amber }: { children: ReactNode; amber?: boolean }) {
  return (
    <span
      className={`rounded-full border px-2 py-0.5 text-[10px] font-mono ${
        amber
          ? "bg-amber-500/10 text-status-warning border-amber-500/25"
          : "bg-bg-inset text-ink-dim border-line"
      }`}
    >
      {children}
    </span>
  )
}

export function SkillInventoryPanel({ project }: { project: string | null }) {
  const [profile, setProfile] = useState("")
  const [inv, setInv] = useState<SkillInventory | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(false)
  const [profileMissing, setProfileMissing] = useState(false)

  useEffect(() => {
    let cancelled = false
    const handle = setTimeout(async () => {
      setLoading(true)
      setError(false)
      setProfileMissing(false)
      try {
        const r = await api.skillInventory(project, profile.trim() || null)
        if (cancelled) return
        if (r === null) {
          // 404. The project filter comes from the known project list, so this
          // can only be an unknown profile.
          setInv(null)
          if (profile.trim()) setProfileMissing(true)
        } else {
          setInv(r)
        }
      } catch {
        if (!cancelled) {
          // N2: drop any prior inventory so an error never renders stale rows
          // under the "unavailable" notice.
          setError(true)
          setInv(null)
        }
      } finally {
        if (!cancelled) setLoading(false)
      }
    }, 300)
    return () => {
      cancelled = true
      clearTimeout(handle)
    }
  }, [project, profile])

  const byScope = useMemo(() => {
    const groups: Record<string, SkillInventory["visible_skills"]> = {}
    for (const d of inv?.visible_skills ?? []) {
      ;(groups[d.scope] ??= []).push(d)
    }
    return groups
  }, [inv])

  const scopes = useMemo(
    () =>
      Object.keys(byScope).sort((a, b) =>
        a === "_global" ? -1 : b === "_global" ? 1 : a.localeCompare(b),
      ),
    [byScope],
  )

  const issues: InventoryIssue[] = inv?.issues ?? []
  const budgetPartial = issues.some((i) => i.code === "skill_inventory.budget_exceeded")
  // The issue rollup excludes per-ref `missing_skill_ref` (already shown as a
  // resolution badge) and `info` (shadowing is shown inline on the ref).
  const scanIssues = issues.filter(
    (i) => i.severity !== "info" && i.code !== "skill_inventory.missing_skill_ref",
  )
  const isShadowed = (ref: string) =>
    issues.some((i) => i.code === "skill_inventory.shadowed_by_project" && i.skill === ref)

  return (
    <div className="p-4 space-y-3 text-[12px]">
      <div className="flex items-center gap-2">
        <Layers size={14} className="text-ink-faint" />
        <h2 className="text-sm font-semibold text-ink">Skill inventory</h2>
        <span className="text-[11px] text-ink-faint">
          {project ? `project: ${project}` : "all projects · _global only"}
        </span>
      </div>
      <p className="text-[11px] text-ink-faint">
        Read-only visibility — metadata only, never skill bodies.
      </p>

      <div className="flex items-center gap-2">
        <label className="text-[11px] text-ink-mute">profile</label>
        <input
          value={profile}
          onChange={(e) => setProfile(e.target.value)}
          placeholder="specialist profile name…"
          spellCheck={false}
          className="w-56 bg-bg-inset border border-line rounded px-2 py-0.5 text-[11px] font-mono focus:outline-none focus:border-accent"
        />
      </div>

      {loading && <div className="text-[11px] text-ink-faint">loading…</div>}
      {error && <div className="text-[11px] text-status-warning/80">inventory unavailable</div>}
      {profileMissing && !loading && (
        <div className="text-[11px] text-status-warning/80">profile not found</div>
      )}

      {inv && !loading && (
        <>
          <div className="flex flex-wrap items-center gap-1.5">
            <Chip>{inv.summary.visible_count} visible</Chip>
            {inv.profile_resolution && <Chip>{inv.summary.profile_ref_count} refs</Chip>}
            {inv.summary.missing_ref_count > 0 && (
              <Chip amber>{inv.summary.missing_ref_count} missing</Chip>
            )}
            {inv.summary.skipped_count > 0 && <Chip>{inv.summary.skipped_count} skipped</Chip>}
            {inv.summary.truncated_count > 0 && (
              <Chip amber>{inv.summary.truncated_count} truncated</Chip>
            )}
            {budgetPartial && <Chip amber>budget partial</Chip>}
          </div>

          {inv.profile_resolution && (
            <section className="rounded-lg border border-line bg-bg-panel p-2 space-y-1">
              <div className="text-[11px] text-ink-mute">
                profile{" "}
                <span className="font-mono text-ink-dim">{inv.profile_resolution.profile}</span>
                {!inv.profile_resolution.enabled && (
                  <span className="ml-1 text-status-warning/80">(disabled)</span>
                )}
                {inv.profile_resolution.role && (
                  <span className="ml-2 font-mono text-ink-faint">
                    role/{inv.profile_resolution.role}
                  </span>
                )}
              </div>
              <ul className="space-y-0.5">
                {inv.profile_resolution.declared_skills.map((r, i) => (
                  <li key={i} className="flex flex-wrap items-center gap-2 text-[11px]">
                    <span className="font-mono text-ink-dim">{r.ref}</span>
                    <span className={`rounded border px-1 text-[10px] ${RES_STYLE[r.resolution]}`}>
                      {r.resolution}
                    </span>
                    {r.resolved_scope && (
                      <span className="font-mono text-ink-faint">
                        {r.resolved_scope}/{r.resolved_name}
                      </span>
                    )}
                    {isShadowed(r.ref) && (
                      <span className="rounded border border-sky-500/25 bg-sky-500/10 px-1 text-[10px] text-sky-300">
                        shadowed by project
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          )}

          {scopes.length === 0 ? (
            <div className="text-[11px] text-ink-faint">No skills visible in this scope.</div>
          ) : (
            scopes.map((scope) => (
              <section key={scope}>
                <div className="mb-0.5 flex items-center gap-1 text-[11px] text-ink-mute">
                  {scope === "_global" ? <Globe size={10} /> : <Box size={10} />}
                  {scope === "_global" ? "global" : scope}
                  <span className="text-ink-faint">· {byScope[scope].length}</span>
                </div>
                <ul className="space-y-0.5">
                  {byScope[scope].map((d, i) => (
                    <li
                      key={i}
                      className="flex flex-wrap items-baseline gap-x-2 rounded bg-bg-inset px-2 py-1 text-[11px]"
                    >
                      <span className="font-mono text-ink-dim">{d.name}</span>
                      {d.declared_name && d.declared_name !== d.name && (
                        <span className="font-mono text-ink-faint">({d.declared_name})</span>
                      )}
                      {d.description && <span className="text-ink-mute">{d.description}</span>}
                      {d.trigger && <span className="text-ink-faint">trigger: {d.trigger}</span>}
                      <span className="ml-auto font-mono text-ink-faint">{fmtKB(d.file_bytes)}</span>
                      {d.truncated && (
                        <span className="rounded bg-amber-500/10 px-1 text-[10px] text-status-warning/80">
                          truncated
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </section>
            ))
          )}

          {scanIssues.length > 0 && (
            <section className="space-y-0.5 pt-1">
              {scanIssues.map((iss, i) => (
                <div key={i} className="flex items-start gap-1 text-[11px] text-status-warning/80">
                  <AlertTriangle size={11} className="mt-0.5 shrink-0" />
                  <span>
                    <span className="font-mono">{iss.code.replace("skill_inventory.", "")}</span> —{" "}
                    {iss.message}
                    {iss.skill ? ` (${iss.skill})` : ""}
                  </span>
                </div>
              ))}
            </section>
          )}
        </>
      )}
    </div>
  )
}
