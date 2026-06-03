# DAG visualizations

Two views, both built on `@xyflow/react` + `dagre`:

| Where | What | Status semantics |
|---|---|---|
| **Tasks tab** | Per-run task DAG | colors driven by task status |
| **Architecture tab** | Project contract graph | colors driven by node type |

## Task DAG

Nodes:

- ✓ **done** — emerald background, no animation
- ◐ **running** — blue background, ring pulse, edge dash flow
- ⏸ **awaiting_approval** — amber, glow + animated edge
- ✗ **failed** — red, static
- ○ **pending** — slate, static (or emerald-tinted edge if upstream done)
- ⊖ **skipped** / **cancelled** — gray

Edges:

- Bezier curves (smoother than smoothstep elbows)
- Animated dashed flow when the source is `done` or target is `running`/`awaiting_approval`/`done`
- Color = target's status color (so "what comes next" is visible at a glance)
- Stroke width slightly thicker for `running` (2px vs 1.6px)

Visual flow indicators:

- `pending` with `done` upstream → emerald-tinted **animated** edge (primed)
- `running` → blue animated edge + pulsing ring on node
- `done → done` → emerald animated edge (the past path stays alive)
- `failed → *` → red edge, no animation downstream

Every completed run also writes an evidence bundle under the run directory:

- `evidence/summary.json` records each task's actual workspace/worktree path,
  start/end timestamps, maximum observed parallelism, overlap windows, and
  acceptance results.
- `evidence/browser/` is created when browser-facing acceptance checks or
  Playwright/Cypress artifacts are detected. It captures check excerpts,
  copied screenshots/traces/reports/videos, and console/network failure lines.
- `PR_BODY.md` is generated from the report, acceptance results, overlap
  evidence, and changed-file list.

Use `maestro runs evidence [run-id]`, `maestro runs replay [run-id]`,
`maestro runs pr-body [run-id]`, or the Tasks tab's Evidence panel when you need
proof that multi-project tasks really overlapped and, for same-repo tasks, ran
in isolated worktrees.

## Architecture graph

Nodes are produced from `projects.yaml`. Each shows:

- Type icon (backend, frontend, mobile, tool, library)
- Project name + type label
- First 3 stack tags as monospace chips (+ overflow count)
- ▲ provides — emerald
- ▼ consumes — blue
- A trash icon visible on hover

Edges are produced from contract relationships. Each edge:

- Source = producer, target = consumer
- Label = the contract file path, opaque-background pill so it doesn't get clipped
- Animated bezier with the contract direction

Layout is LR (left-to-right) with dagre. A post-pass snaps each single-upstream node's Y to its source's Y, producing straight horizontal pipelines for simple chains.

## Interactions

- **Drag** to pan, **scroll** to pan, **pinch** to zoom
- The bottom-right control panel exposes manual zoom + fit
- Double-click does nothing (deliberately disabled — prevents accidental zoom while reading)
- Architecture nodes are **draggable**; task nodes are not (they're laid out automatically and movement isn't meaningful)

## Why ReactFlow?

We considered Mermaid (cheaper but static), d3-dag (more powerful but heavier), and hand-rolled SVG. ReactFlow wins on:

- Real DOM nodes — custom interactive content per node (status icons, action menus)
- Built-in pan/zoom/keyboard handling
- Animated edge support out of the box

The cost is bundle size — `@xyflow/react` + dagre is ~150KB gzipped. Both DAG views are lazy-loaded, so the chat tab stays small.
