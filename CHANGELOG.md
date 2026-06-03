# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Demo asciicast** — `assets/demo.gif` (12s) showing
  `maestro demo --run` end-to-end. Embedded in both READMEs. Rebuild
  with `./scripts/record-demo.sh`; the script synthesizes the cast
  programmatically so it works in headless / CI environments.
- **Channel Adapter MVP** — bidirectional integration with chat tools
  (Feishu via [botmux](https://github.com/deepcoldy/botmux)). Inbound
  messages become runs; outbound run events stream back as replies. See
  the PR-Ch1 → PR-Ch5 chain below for the implementation phases.
  - **PR-Ch1 (#28)** — `ChannelEnvelope` v1 schema and `channels.yaml`
    config parser.
  - **PR-Ch2 (#30)** — Feishu inbound parser, trust resolver, dispatch
    routing.
  - **PR-Ch3 (#31)** — Outbound formatter turning `RunEvent`s into
    Feishu reply payloads.
  - **PR-Ch4a (#32)** — Inbound envelope persistence and run-origin
    resolver.
  - **PR-Ch4b (#33)** — Outbound subscription handler that appends
    formatted replies to `outbound_replies.ndjson`.
  - **PR-Ch4c (#34)** — Executor subscribe hook and
    `maestro channels listen --once --from-stdin` CLI.
  - **PR-Ch5 (#35)** — True closed loop: `maestro channels drain` sends
    outbound replies via `botmux send`; `maestro channels listen --poll`
    fetches inbound messages via `botmux history` and writes
    `RouteDecision`s to `channel_inbound_decisions.ndjson`. Cursors are
    persisted atomically (write-tmp + rename); inbound dedup uses
    `(create_time, message_id)` tuples instead of lex-sorted message IDs
    (fixes a silent drop hazard on Feishu's non-monotonic `om_xxx` IDs).
- **WorktreePolicy** with `copy_files` guard for safer per-task git
  worktree isolation (#29).
- **Schema v1** capability and permission evidence (#25).
- **Init analysis audit workflow** (#24).

### Changed

- **README** redesigned for open-source presentation: shields.io badges,
  a comparison table vs LangGraph / CrewAI / Cursor, six-bullet feature
  list, hero-shot grid, and collapsible workspace-layout / CLI-reference
  sections. English + 中文 both updated.
- **App icon** redesigned (`assets/icon.png`,
  `web/public/icon-{512,192}.png`, `web/public/favicon.png`,
  `web/dist/*`). New mark: an M-shaped constellation of glowing nodes
  on a deep-navy night-sky background, replacing the earlier 512×341
  pearl-on-string M (was non-square, less readable at small sizes).

### Added (docs / project infrastructure)

- `LICENSE` (MIT) — previously linked from README but missing on disk.
- `CONTRIBUTING.md` with dev setup, branching, TDD norms, PR conventions.
- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1).
- `SECURITY.md` with private GitHub Security Advisory reporting flow.
- `.github/PULL_REQUEST_TEMPLATE.md`.
- `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml`.

### Deferred (next-PR follow-ups, tracked)

See [`docs/experience/BACKLOG.md`](docs/experience/BACKLOG.md) for the full,
rationale-annotated list with file anchors and how-to-resume notes. In brief:

- `BotmuxTransport.poll` does not yet pass `--since-message-id` to `botmux`
  (deduplication is local within the polling window); needs verification that
  the installed `botmux` supports the flag (#8).
- `run_outcome` runs git twice per task — cosmetic; the safe fix must not
  regress contract-drift detection for in-place runs (#30).
- `dispatch_one_task`'s worktree/spawn phase is not yet extracted — it's
  entangled with run-state locks and async control flow (#20 remainder).
- `subscribe.rs::append_reply` and `origin.rs::persist` use plain append-write
  to NDJSON; not crash-atomic — fold into a shared atomic-append helper.

---

[Unreleased]: https://github.com/AnranS/repo-maestro/commits/main
