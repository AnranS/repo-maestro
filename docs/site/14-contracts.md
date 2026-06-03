# Cross-project contracts

A **contract** is a file (usually a schema or a shared module) that one project authors and one or more other projects depend on. Contracts give `maestro` enough information to:

- Draw the architecture dependency graph
- **Auto-wire** consumer tasks to run after their producers (so they never build against a stale contract)
- Detect when two parallel tasks would race on the same `provides` file
- Auto-inject the contract's *actual* content into consumer tasks' prompts (so the agent codes against the real interface, not a guess)

You can declare contracts by hand, but in most cases **discovery infers them for you** — see below.

## Auto-discovery (no declaration needed)

`maestro init --analyze` / `maestro work --root <dir>` scans your projects and, in addition to manifest dependencies (`package.json`, Cargo, etc.), follows **relative source imports** that cross a project boundary. When project B imports a file from project A:

- A gets that file as its `provides`,
- B gets a `consumes` pointing at it,
- an edge `A → B` is recorded (shown dashed, with a confidence score, in the Architecture tab).

So a monorepo or polyrepo where packages `import "../shared/..."` gets a working contract graph with **zero hand-written config**. The inferred contracts are written into `.maestro/projects.yaml`; edge provenance is persisted to `.maestro/topology.json`.

## Declaring (optional / overrides)

```yaml
projects:
  login-api:
    path: ./login-api
    contracts:
      provides: schemas/openapi.yaml

  login-web:
    path: ./login-web
    contracts:
      consumes: schemas/openapi.yaml

  mobile-app:
    path: ./mobile-app
    contracts:
      consumes: schemas/openapi.yaml
```

The path is **relative to each project's own directory**. In the example the file is `login-api/schemas/openapi.yaml`; a consumer in a sibling repo can write `consumes: ../login-api/schemas/openapi.yaml`. You do **not** need an identical string on both sides — maestro matches contracts by logical path (last two path components), so provider-relative and consumer-relative spellings of the same file resolve to the same contract.

## Graph view

The **Architecture** tab visualizes contracts as edges. The producer node has a green ▲ label (`provides`), the consumer has a blue ▼ label (`consumes`). The edge is animated to show data direction.

## Auto-wiring

Before a run, `maestro run` adds the dependency edges your contracts imply: a consumer task with no transitive dependency on its producer gets a `depends_on` edge to the producer's terminal task. You'll see a line like:

```text
→ wired 2 contract dependency edge(s) so consumers don't race ahead of producers
  + T_change_web depends_on T_change_api  (contract `schemas/openapi.yaml`)
```

Pass `--no-wire-contracts` to run the plan verbatim. Every wired edge is recorded in the run's automatic-actions ledger (REPORT.md, the run-end summary, and the dashboard).

## Static safety

`maestro plan validate` (and the implicit pre-run validation) checks:

- Two tasks running in parallel both write to the same `provides` file → **error**
- A consumer's task does not transitively depend on its producer → **warning** ("they may race"); auto-wiring fixes most of these automatically

## Auto-injection

When a consumer task fires, the consumed contract's **current content** is injected into the agent prompt:

```text
## Consumed contract: `schemas/openapi.yaml`

<the file's content — read from the producer's just-integrated change>
```

The content is read from the producer's integration worktree when available, so a downstream task sees the upstream edit even in a polyrepo where the file isn't on its own disk. A `login-web` task can be told just "make the login form work" — the agent sees the exact schema and doesn't guess endpoint shapes.

## Roadmap

Today's contract layer is single-string per direction. The roadmap covers:

- Multiple `provides` / `consumes` per project
- Diff summaries in `PLAN.yaml`'s `contracts_change` field (already in the data model)
- Contract version stamping so consumers can pin to a specific shape
