import { useEffect, useMemo, useRef, useState } from "react"
import type { StarMap, Star } from "../types"
import { api } from "../api"
import { t } from "../i18n"

/**
 * Memory constellation. Every L2 decision is a node; nodes cluster into project
 * "systems"; a run draws a line through the nodes it produced across projects
 * (the visual of different projects' memory assembled together); contracts are
 * shown as faint structural edges that pull coupled systems together.
 *
 * Deliberately restrained — a developer knowledge map, not a screensaver: flat
 * dark canvas, crisp small nodes, thin edges, muted palette, no ambient motion.
 * The only animation is a subtle pulse along a run's line when you hover it,
 * which traces how that change propagated across projects. Layout is a small
 * hand-rolled force relaxation (no deps), deterministic (seeded by id).
 */

const PALETTE = [
  "#6ea8d8", "#cf7e9c", "#5fb89a", "#c7a45e", "#9d8bd0",
  "#5aa9bd", "#cf8c63", "#7bbf7b", "#c08ac9", "#cf7f7f",
]

const W = 960
const H = 640

function seed(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return (h >>> 0) / 4294967295
}

function hexToRgb(hex: string): [number, number, number] {
  const r = hex.replace("#", "")
  return [parseInt(r.slice(0, 2), 16), parseInt(r.slice(2, 4), 16), parseInt(r.slice(4, 6), 16)]
}

type Pt = { x: number; y: number }

function computeLayout(data: StarMap) {
  const cx = W / 2, cy = H / 2
  const names = data.projects.map((p) => p.name)
  const n = Math.max(1, names.length)
  const colorOf: Record<string, string> = {}
  names.forEach((name, i) => (colorOf[name] = PALETTE[i % PALETTE.length]))

  const anchors: Record<string, Pt> = {}
  names.forEach((name, i) => {
    const a = (i / n) * Math.PI * 2 - Math.PI / 2
    anchors[name] = { x: cx + Math.cos(a) * 230, y: cy + Math.sin(a) * 205 }
  })
  for (let it = 0; it < 170; it++) {
    for (const c of data.contracts) {
      const a = anchors[c.from], b = anchors[c.to]
      if (!a || !b) continue
      const dx = b.x - a.x, dy = b.y - a.y, k = 0.004
      a.x += dx * k; a.y += dy * k; b.x -= dx * k; b.y -= dy * k
    }
    for (let i = 0; i < names.length; i++)
      for (let j = i + 1; j < names.length; j++) {
        const a = anchors[names[i]], b = anchors[names[j]]
        let dx = b.x - a.x, dy = b.y - a.y
        const d = Math.hypot(dx, dy) || 1, min = 190
        if (d < min) { const f = ((min - d) / d) * 0.5; dx *= f; dy *= f; a.x -= dx; a.y -= dy; b.x += dx; b.y += dy }
      }
  }

  const pos: Record<string, Pt> = {}
  for (const s of data.stars) {
    const a = anchors[s.project] ?? { x: cx, y: cy }
    pos[s.id] = { x: a.x + (seed(s.id) - 0.5) * 58, y: a.y + (seed(s.id + "y") - 0.5) * 58 }
  }
  for (let it = 0; it < 150; it++) {
    for (const s of data.stars) {
      const a = anchors[s.project] ?? { x: cx, y: cy }
      const p = pos[s.id]; p.x += (a.x - p.x) * 0.06; p.y += (a.y - p.y) * 0.06
    }
    for (let i = 0; i < data.stars.length; i++)
      for (let j = i + 1; j < data.stars.length; j++) {
        const p = pos[data.stars[i].id], q = pos[data.stars[j].id]
        let dx = q.x - p.x, dy = q.y - p.y
        const d = Math.hypot(dx, dy) || 1
        if (d < 30) { const f = ((30 - d) / d) * 0.5; dx *= f; dy *= f; p.x -= dx; p.y -= dy; q.x += dx; q.y += dy }
      }
  }
  return { anchors, pos, colorOf }
}

function nodeRadius(s: Star): number {
  return 2.8 + Math.min(6, s.blast) * 0.7
}

// sparse, faint, static background dust (subtle texture, not a galaxy)
const DUST = Array.from({ length: 130 }).map((_, i) => ({
  x: seed(`dx${i}`) * W, y: seed(`dy${i}`) * H,
  r: seed(`dr${i}`) * 0.7 + 0.25, a: seed(`da${i}`) * 0.12 + 0.03,
}))

export function StarMapPanel() {
  const [data, setData] = useState<StarMap | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [hover, setHover] = useState<Star | null>(null)
  const [activeRun, setActiveRun] = useState<string | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)

  useEffect(() => { api.memoryStarmap().then(setData).catch((e) => setErr(String(e))) }, [])

  const layout = useMemo(() => (data ? computeLayout(data) : null), [data])
  const times = useMemo(() => {
    if (!data) return [] as string[]
    const set = new Set<string>()
    data.stars.forEach((s) => s.ts && set.add(s.ts))
    return [...set].sort()
  }, [data])
  const [cutoffIdx, setCutoffIdx] = useState<number | null>(null)
  const [decay, setDecay] = useState(false)
  const [playing, setPlaying] = useState(false)
  const effIdx = cutoffIdx ?? Math.max(0, times.length - 1)

  useEffect(() => {
    if (!playing || times.length === 0) return
    const id = setInterval(() => {
      setCutoffIdx((prev) => {
        const cur = prev ?? times.length - 1
        if (cur >= times.length - 1) { setPlaying(false); return cur }
        return cur + 1
      })
    }, 950)
    return () => clearInterval(id)
  }, [playing, times.length])

  const view = useRef({ data, layout, times, effIdx, decay, activeRun })
  view.current = { data, layout, times, effIdx, decay, activeRun }

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const dpr = Math.min(2, window.devicePixelRatio || 1)
    canvas.width = W * dpr; canvas.height = H * dpr
    const ctx = canvas.getContext("2d")!
    let raf = 0
    const t0 = performance.now()

    const draw = (now: number) => {
      const tt = (now - t0) / 1000
      const { data, layout, times, effIdx, decay, activeRun } = view.current
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)

      ctx.fillStyle = "#0a0c11"
      ctx.fillRect(0, 0, W, H)
      // faint vignette for depth (very subtle)
      const vg = ctx.createRadialGradient(W / 2, H / 2, H * 0.2, W / 2, H / 2, H * 0.75)
      vg.addColorStop(0, "rgba(255,255,255,0.015)")
      vg.addColorStop(1, "rgba(0,0,0,0.25)")
      ctx.fillStyle = vg
      ctx.fillRect(0, 0, W, H)

      if (!data || !layout) { raf = requestAnimationFrame(draw); return }
      const { anchors, pos, colorOf } = layout
      const timeIdx: Record<string, number> = {}
      times.forEach((x, i) => (timeIdx[x] = i))
      const revealed = (s: Star) => !s.ts || (timeIdx[s.ts] ?? 0) <= effIdx
      const bright = (s: Star) => {
        const base = decay
          ? Math.max(0.18, 1 - (effIdx - (s.ts ? timeIdx[s.ts] ?? 0 : effIdx)) * 0.2)
          : 0.5 + 0.5 * s.recency
        return base * (activeRun && s.run_id !== activeRun ? 0.25 : 1)
      }
      const runColor: Record<string, string> = {}
      data.runs.forEach((r, i) => (runColor[r.run_id] = PALETTE[(i + 3) % PALETTE.length]))

      // faint static dust
      for (const d of DUST) {
        ctx.fillStyle = `rgba(180,195,225,${d.a})`
        ctx.beginPath(); ctx.arc(d.x, d.y, d.r, 0, Math.PI * 2); ctx.fill()
      }

      // contract edges (structural, dashed)
      ctx.setLineDash([2, 5])
      for (const c of data.contracts) {
        const a = anchors[c.from], b = anchors[c.to]
        if (!a || !b) continue
        ctx.strokeStyle = "rgba(110,128,160,0.22)"
        ctx.lineWidth = 1
        ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.stroke()
      }
      ctx.setLineDash([])

      // run lines (a run stitching its nodes across projects)
      for (const run of data.runs) {
        const pts = run.star_ids
          .filter((id) => { const s = data.stars.find((x) => x.id === id); return s && revealed(s) })
          .map((id) => pos[id]).filter(Boolean) as Pt[]
        if (pts.length < 2) continue
        const cxr = pts.reduce((s, p) => s + p.x, 0) / pts.length
        const cyr = pts.reduce((s, p) => s + p.y, 0) / pts.length
        const ord = [...pts].sort((p, q) => Math.atan2(p.y - cyr, p.x - cxr) - Math.atan2(q.y - cyr, q.x - cxr))
        const [r, g, b] = hexToRgb(runColor[run.run_id])
        const on = activeRun === run.run_id
        const al = activeRun === null ? 0.3 : on ? 0.85 : 0.06
        ctx.strokeStyle = `rgba(${r},${g},${b},${al})`
        ctx.lineWidth = on ? 1.6 : 1
        ctx.beginPath()
        ord.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)))
        ctx.stroke()

        // subtle propagation pulse — only on the hovered run
        if (on) {
          const segs: { a: Pt; b: Pt; len: number; acc: number }[] = []
          let total = 0
          for (let i = 1; i < ord.length; i++) {
            const len = Math.hypot(ord[i].x - ord[i - 1].x, ord[i].y - ord[i - 1].y)
            segs.push({ a: ord[i - 1], b: ord[i], len, acc: total }); total += len
          }
          if (total > 0) {
            const f = (tt * 0.22) % 1
            const d = f * total
            const seg = segs.find((s) => d >= s.acc && d <= s.acc + s.len) ?? segs[segs.length - 1]
            const u = seg.len ? (d - seg.acc) / seg.len : 0
            const px = seg.a.x + (seg.b.x - seg.a.x) * u
            const py = seg.a.y + (seg.b.y - seg.a.y) * u
            ctx.fillStyle = `rgba(${r},${g},${b},0.9)`
            ctx.beginPath(); ctx.arc(px, py, 2.6, 0, Math.PI * 2); ctx.fill()
          }
        }
      }

      // decision nodes — crisp, minimal
      for (const s of data.stars) {
        const p = pos[s.id]
        if (!p || !revealed(s)) continue
        const rr = nodeRadius(s)
        const intensity = bright(s)
        const [r, g, b] = hexToRgb(colorOf[s.project])
        // very subtle halo for a touch of depth (not a glow)
        const halo = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, rr * 2.4)
        halo.addColorStop(0, `rgba(${r},${g},${b},${0.16 * intensity})`)
        halo.addColorStop(1, `rgba(${r},${g},${b},0)`)
        ctx.fillStyle = halo
        ctx.beginPath(); ctx.arc(p.x, p.y, rr * 2.4, 0, Math.PI * 2); ctx.fill()
        // body + ring
        ctx.fillStyle = `rgba(${r},${g},${b},${intensity})`
        ctx.beginPath(); ctx.arc(p.x, p.y, rr, 0, Math.PI * 2); ctx.fill()
        ctx.strokeStyle = `rgba(230,238,255,${0.35 * intensity})`
        ctx.lineWidth = 0.8
        ctx.beginPath(); ctx.arc(p.x, p.y, rr, 0, Math.PI * 2); ctx.stroke()
      }

      // labels
      ctx.font = "600 11px ui-sans-serif, system-ui"
      ctx.textAlign = "center"
      for (const p of data.projects) {
        const a = anchors[p.name]
        if (!a) continue
        const [r, g, b] = hexToRgb(colorOf[p.name])
        ctx.fillStyle = `rgba(${r},${g},${b},0.85)`
        ctx.fillText(p.name, a.x, a.y - 26)
      }

      raf = requestAnimationFrame(draw)
    }
    raf = requestAnimationFrame(draw)
    return () => cancelAnimationFrame(raf)
  }, [layout])

  if (err) return <div className="p-6 text-xs text-red-400">star map error: {err}</div>
  if (!data || !layout) return <div className="p-6 text-xs text-ink-faint">loading…</div>
  if (data.stars.length === 0)
    return <div className="p-6 text-xs text-ink-faint">{t("starmap.empty")}</div>

  const { pos } = layout
  const runColor: Record<string, string> = {}
  data.runs.forEach((r, i) => (runColor[r.run_id] = PALETTE[(i + 3) % PALETTE.length]))

  const onMove = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const x = ((e.clientX - rect.left) / rect.width) * W
    const y = ((e.clientY - rect.top) / rect.height) * H
    let best: Star | null = null, bd = 13 * 13
    for (const s of data.stars) {
      const p = pos[s.id]
      if (!p) continue
      const d = (p.x - x) ** 2 + (p.y - y) ** 2
      if (d < bd) { bd = d; best = s }
    }
    setHover(best)
  }

  return (
    <div className="flex gap-3">
      <div className="relative flex-1 overflow-hidden rounded-lg border border-line">
        <canvas ref={canvasRef} className="block w-full" style={{ height: "auto" }}
          onMouseMove={onMove} onMouseLeave={() => setHover(null)} />

        {times.length > 1 && (
          <div className="absolute inset-x-3 bottom-3 flex items-center gap-2 rounded-lg border border-line bg-bg-inset/85 px-3 py-2 text-[11px] backdrop-blur">
            <button
              onClick={() => { if (playing) { setPlaying(false) } else { setCutoffIdx(0); setPlaying(true) } }}
              className="shrink-0 rounded bg-bg-hover px-2 py-1 text-ink hover:text-accent">
              {playing ? `⏸ ${t("starmap.pause")}` : `▶ ${t("starmap.play")}`}
            </button>
            <input type="range" min={0} max={times.length - 1} value={effIdx}
              onChange={(e) => { setPlaying(false); setCutoffIdx(Number(e.target.value)) }}
              className="flex-1 accent-blue-400" />
            <span className="w-24 shrink-0 font-mono text-ink-faint">{times[effIdx]}</span>
            <label className="flex shrink-0 cursor-pointer items-center gap-1 text-ink-dim">
              <input type="checkbox" checked={decay} onChange={(e) => setDecay(e.target.checked)} />
              {t("starmap.decay")}
            </label>
          </div>
        )}

        {hover && (
          <div className="pointer-events-none absolute left-3 top-3 max-w-xs rounded-lg border border-line bg-bg-inset/95 p-2.5 text-[11px]">
            <div className="font-semibold text-ink">{hover.project}</div>
            <div className="mt-0.5 text-ink-dim">{hover.title}</div>
            <div className="mt-1 flex flex-wrap gap-2 text-ink-faint">
              {hover.status && <span>· {hover.status}</span>}
              {hover.ts && <span>· {hover.ts}</span>}
              <span>· blast {hover.blast}</span>
            </div>
          </div>
        )}
      </div>

      <div className="w-56 shrink-0 space-y-3 text-[11px]">
        <div className="rounded-lg border border-line bg-bg-inset p-2.5">
          <div className="mb-1.5 font-semibold text-ink-dim">{t("starmap.runs")} ({data.runs.length})</div>
          <div className="max-h-64 space-y-1 overflow-y-auto scrollbar-thin">
            {data.runs.map((r) => (
              <div key={r.run_id}
                onMouseEnter={() => setActiveRun(r.run_id)} onMouseLeave={() => setActiveRun(null)}
                className={`flex cursor-pointer items-start gap-1.5 rounded px-1 py-0.5 ${activeRun === r.run_id ? "bg-bg-hover" : ""}`}>
                <span className="mt-1 h-2 w-2 shrink-0 rounded-full" style={{ background: runColor[r.run_id] }} />
                <span className="text-ink-dim">{r.spec}<span className="text-ink-faint"> · {r.star_ids.length}</span></span>
              </div>
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-line bg-bg-inset p-2.5 text-ink-faint">
          <div className="mb-1 font-semibold text-ink-dim">{t("starmap.legend")}</div>
          <div>· {t("starmap.legend.star")}</div>
          <div>· {t("starmap.legend.size")}</div>
          <div>· {t("starmap.legend.bright")}</div>
          <div>· {t("starmap.legend.line")}</div>
          <div>· {t("starmap.legend.gravity")}</div>
        </div>
      </div>
    </div>
  )
}
