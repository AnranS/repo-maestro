# Team agent-workbench absorption plan

Status: design / boundary-setting · Owner: maestro · Reviewer: cross-review

Scope: decide which **protocols** and **product-shape** ideas from a reference
team agent-workbench platform are worth absorbing into Repo Maestro — and,
equally important, which are explicitly out of scope.

Repo Maestro stays what it is: a local-first, dependency-ordered, **auditable
multi-repo / multi-agent workflow runtime**. We absorb collaboration protocols,
distribution format, the observability surface, and memory governance — not the
platform shape.

## Non-goals (hard)

- NOT re-implementing the tracker-driven single-repo auto-fix bot we also
  surveyed — out of scope entirely.
- NOT cloning the full workbench platform.
- NOT building centralized multi-tenancy, SSO, or an enterprise backend.
- NOT autonomous "system actions": no agent takes an irreversible, outward
  action without a human-approved gate. (If an OnCall-style adapter is ever
  built, Phase 1 is read-only diagnosis + human-approved action only.)

## Fit-gap matrix

| Reference capability | Maestro today | Near-term absorb | Out of scope (now) |
|---|---|---|---|
| Personal vs team instance boundary | implicit single workspace | lightweight `instance` metadata (mode / owner / channels / policy) | full SSO / RBAC / tenancy |
| Unified finding/evidence store | events + decisions + REPORT, separate streams | one `findings.ndjson` ledger (F-110) | cross-org aggregation |
| Human-on-the-loop console | dashboard leads with graphs | reprioritize first screen to approvals / blocked / findings / evidence | full ops portal for every agent |
| Best practice as installable artifact | one Claude-Code skill wrapper | `pack export/import` (roles / skills / defaults / policy / channels) | central marketplace |
| Layered team memory + provenance | local L2 decisions | add provenance fields to memory records | large shared Team Memory service |
| Channel / mobile approval | approval gate + `approve` cmd + channel/botmux | wire approve/reject from chat + dry-run blast-radius → confirm | mobile-native app |
| Scheduled / cron tasks | none | design only (`--schedule`); no daemon yet | always-on daemon |
| ROI / benefit attribution | run duration / events | cheap signals later (saved reruns, report counts) | enterprise reporting |

## Borrowed primitives (the 6 we commit to)

1. **Instance mode** — `mode: personal | team` + owner + channels + default
   approval policy, so dashboard / channel / memory / approval can branch their
   behavior. Minimal; explicitly NOT a precursor to SSO.
2. **Finding ledger** — one append-only evidence/fact store per run; every
   producer (risk, refuter, learn, doctor, channel) writes one record kind.
   Design: [F-110](F-110-FINDING-LEDGER-DESIGN.md).
3. **HOTL dashboard** — the first screen answers "what must a human supervise":
   run timeline, approval queue, blocked tasks, risk/refuter findings, evidence
   links — not decorative graphs.
4. **Pack / skill distribution** — practices ship as an installable artifact,
   not prose. `pack export/import` bundles roles, skills, project defaults,
   approval policy, channel config, and bench fixtures.
5. **Team memory provenance** — each memory record carries `source_system`,
   `external_id`, `contributor`, `memory_layer`, `provenance`, `revocation_id`
   even before any shared-memory service exists. A governance-first data model.
6. **Channel approval** — `approve` / `reject` / `status` / `report` from a chat
   message; a large plan dry-runs first and posts its blast radius to a channel
   for a human to confirm before it runs.

## Implementation order

1. **Finding ledger (F-110)** — the data foundation the dashboard, memory, and
   channel surfaces all read from. Design first, then a minimal writer + 1–2
   existing producers; no big-bang refactor of REPORT/decisions/events.
2. **HOTL dashboard surface** — reprioritize the first screen onto the ledger +
   approvals + blocked tasks. (Same area as the recent webui loading-state fix.)
3. **Pack / skill distribution** — `pack export/import`, an open-source-friendly
   "install a set of practices" entry point.
4. **Instance mode** — lands alongside (1) as the metadata the others branch on.

Deferred: a layered Team Memory service, SSO / RBAC / tenancy, cron / daemon,
ROI attribution.

## Privacy boundary — no employer-sensitive material

This absorption work draws **abstract protocol and product-shape ideas only**.
The following classes must NEVER enter this repo (or any public mirror), in any
case or normalized variant:

- Consumer-app brand names, the parent-company name, internal platform names,
  internal project names, real repo paths, real tokens, real ticket/alert
  content, internal URLs, meeting links, or screenshots.
- The reference platform's internal details, URLs, or raw examples.

The concrete forbidden terms are enumerated only in the operator-local private
wordlist (below), never spelled out in-repo.

All examples in this work use neutral fixtures only: `billing-service`,
`web-frontend`, `shared-contracts`, `example-workspace`. Any private scan
wordlist stays on the operator's machine / a private doc — never in the repo.

Before every push, run the release-scope scan and a manual keyword sweep:

```bash
SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 ./scripts/secret-scan.sh
```

plus a manual sweep for the keyword classes above, including case and
normalized variants.
