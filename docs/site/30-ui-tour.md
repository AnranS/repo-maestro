# Dashboard tour

`maestro ui` (or `maestro open`) starts a small axum server at `http://127.0.0.1:7777` and serves a React app embedded in the binary. Five tabs across the top, plus a Settings gear and a connection indicator.

## Header

| Region | Content |
|---|---|
| Logo + name | brand mark |
| Tab nav | chat · tasks · context · architecture · docs |
| Center | Current run's `spec` (the chat request that produced the plan), or `(no active run)` |
| Right | Run summary chips (running / done / failed), connection dot, settings gear |

The connection dot is **green** when the SSE stream is live, **amber** when reconnecting.

## Chat tab

- Left sidebar: sessions, filterable by tag, plus the **+ new** button and an "external history" group listing read-only sessions from Cursor IDE / Claude Code / Codex on this machine
- Middle: message stream with markdown + syntax-highlighted code
- Right column under input: model picker, attached project context, send/stop button

The session list auto-scrolls to the most recent active session. Click any session to open it; the URL hash updates to `#chat`.

## Tasks tab

- Left sidebar: every historical run with status pill, click to load
- Middle: live DAG visualizer (top half) + task list (bottom half)
- DAG nodes are draggable, scroll/pinch zoom, double-click to focus

A running task pulses its node ring; the edge between a `done` upstream and `pending` downstream lights up emerald to show "primed, ready to fire."

## Context tab

Browse and edit:

- **L1 facts** grouped by topic — CodeMirror with markdown highlight
- **Skills** grouped by scope — same editor

Saving in the browser writes straight to disk. The orchestrator picks up changes immediately on the next task.

## Architecture tab

- Header: module count, contract edge count, the legend pill, **+ add project**
- Body: ReactFlow graph with dagre layout
  - Producer nodes have a green ▲ "provides" line
  - Consumer nodes have a blue ▼ "consumes" line
  - Edges are animated bezier curves labeled with the contract file path

Hovering a node reveals a **trash** icon for quick removal (writes back to `projects.yaml`; doesn't touch your disk).

## Docs tab

The page you're reading right now. Sidebar lists every page grouped by section; clicking a page deep-links the URL hash so you can bookmark.

## Settings modal

Gear icon → `Settings · projects.yaml defaults`. Edit:

- **default agent** (codex / cursor / shell / mock)
- **agent_model** — full searchable dropdown over the 100+ models in your account
- **tagger_model** — same dropdown, but pick a cheap one
- **branch_prefix**
- **max_parallel**
- A **refresh model list** button for re-pulling provider model catalogs where supported

Save writes to `.maestro/projects.yaml` atomically.
