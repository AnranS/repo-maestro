# F-103 — `contracts.provides` / `contracts.consumes` auto-promotion

> **Scope.** This is the design note for Cycle 7 Patch 2. **Rules only**,
> no raw monorepo content. The heuristics are described against a
> generic example (`shared-lib` / `app-alpha` / `webapp-alpha`) matching
> the neutral fixture set we already ship in tests.

## 1. Problem

`Project.contracts.provides` and `Project.contracts.consumes` exist and
are wired into `wire_contract_dependencies`, but on real workspaces
**discovery never sets them**. On a 65-project dogfood workspace:

- `mst brief` reports `0 contract provider(s)` even when one library
  has 30+ consumers.
- `wire_contract_dependencies` has no fuel and cannot reorder DAGs to
  protect contract races.
- The codegraph-derived dependency fold-in (commit `4f94b94`)
  compensates for ordering but not for the "this project is a
  contract surface" signal users want to read.

The previous round (Cycle 7 Stage 1, F-103) marked this P2 with a
**design-first** note — discovery already has some promotion code on
the import side; silent heuristic changes here are risky because
false positives propagate into `wire_contract_dependencies` and
silently re-order plans.

## 2. Goals and non-goals

**Goals:**
- Populate `contracts.provides` on projects that publish a contract
  surface (IDL / OpenAPI / proto / thrift).
- Populate `contracts.consumes` on projects with a generated-client
  directory pointing at a known producer.
- Be **conservative** — prefer no signal over a false positive.
- Preserve the existing user-override invariant
  (`merge_project_from_discovery` already keeps user-set contracts).

**Non-goals:**
- Not a replacement for the codegraph fold-in. The fold-in still
  produces the runtime DAG ordering; contract promotion only adds
  the *named-contract* surface so `wire_contract_dependencies` can
  reason about which producer/consumer pair shares a contract.
- No CLI flag yet. Promotion runs as part of discovery unconditionally
  (matches every other heuristic in `discovery.rs`).
- No retro-fit pass — promotion fires on `mst init` / `mst work`'s
  discovery step; existing `projects.yaml` entries are updated via the
  normal merge path.

## 3. Triggers

Two independent decisions: provider side and consumer side. A project
can be both.

### 3.1 Provider promotion

Fire `contracts.provides = "<selected contract file relative to
provider project root>"` when **all** of:

1. **Directory marker present.** The project tree contains at least
   one of these subdirectories (case-insensitive, name match — depth
   up to the project root + 2):
   ```
   idl/         idl/rpc/         idl/contracts/
   proto/       protos/
   openapi/     openapi/schemas/
   schemas/     api-spec/        contracts/
   thrift/      thrifts/
   ```
2. **At least one contract-file inside.** That marker directory must
   contain ≥ 1 file with one of: `.thrift`, `.proto`, `.yaml`, `.yml`,
   `.json`, `.openapi.yaml`. Empty marker dirs are ignored.
3. **Not a generated-client directory.** Reject if the marker path is
   nested under any of the `is_generated_client_path` markers (`/bam-
   idl/`, `/openapi-client/`, etc.) — those are the consumer-side
   mirror, not a publishing surface.
4. **Project root is registered.** Don't synthesize `provides` for an
   unregistered project; that would dangle.

The promoted string is the **relative path from the producer's project
root to a representative contract FILE inside the marker dir** — not
the marker dir itself. Downstream code (notably
`scheduler::executor::consumed_contract_section`) resolves
`provider_root.join(provides)` and calls `read_to_string` on it, so a
directory would break the contract-content injection. **Never store a
workspace-relative path in `contracts.provides`** — that would break
the resolution chain in `wire_contract_dependencies` and every
downstream reader.

Picking the representative file inside the marker:
1. **Canonical filename** wins first: `openapi.yaml/yml/json`,
   `swagger.yaml/yml/json`, `schema.graphql`.
2. Otherwise the **shortest path** wins.
3. Tie-break is lexicographic.

If multiple marker dirs match, prefer (a) `idl/`-family, (b) shortest
file path, (c) lexicographic order.

Multi-contract publishing surfaces (a project that genuinely declares
multiple separate contracts) is out of scope for F-103 and stays in
the followup list.

### 3.2 Consumer promotion

Fire `contracts.consumes = <relative-path-to-producer-contract>` when
**all** of:

1. **Generated-client marker present.** The project tree contains a
   directory matching `is_generated_client_path` (reuse the existing
   list — don't fork).
2. **Generated-client subdir names a registered producer.** The
   immediate subdir under the marker (e.g. `bam-idl/<ServiceName>/`)
   maps via the existing `path_contains_token` word-boundary match to
   a project that **already has** `contracts.provides` set by step 3.1.
3. **Producer's `provides` is reachable.** If the matched producer has
   no `provides` (because its trigger 3.1 didn't fire), do not invent
   a string — skip and let codegraph fold-in continue to carry the
   ordering. This is the key false-positive guard: consumer-side
   promotion is **strictly downstream of** provider-side promotion.

The promoted string is the **path from the consumer project root to
the producer's contract** — matching the existing edge-loop convention
`relative_to(&consumer_dir, provider_dir.join(provides))` (typically
yields `../../producer/idl/...`). Same hard rule as §3.1: project-
relative, never workspace-relative; the resolution chain relies on
this.

When one consumer maps to multiple producers, pick the producer with
the largest in-tree file count under the marker (heaviest signal).

## 4. False-positive guards (the hard constraints)

These are non-negotiable; the heuristic must refuse to fire if any
trips:

| Guard | What it stops |
|---|---|
| Generated-client dirs cannot also be providers | A `bam-idl/X` dir under project P does not make P provide X's contract |
| Empty marker dir | Placeholder `proto/` with no files is not a publishing surface |
| Producer not registered | Don't promote a consumer pointing at an unknown name |
| Producer has no `provides` set | Don't invent a contract path for the consumer to "consume" |
| User-set `contracts` survives | Existing merge logic preserves user edits; promotion is additive only when both sides are unset |
| One file in vendor / node_modules | Walk respects existing `.gitignore` + skip set |

## 5. What gets logged

Discovery already prints `→ codegraph: folded N derived dependency
edge(s)`. Add a sibling line so promotion is visible:

```
→ codegraph: promoted N project(s) to contract provider(s), M to consumer(s).
```

Per-pair provenance goes into the **existing** `edges[]` array in
`.maestro/topology.json` as a new entry shape that the current reader
already accepts:

```rust
DiscoveredEdge {
    from: "<producer-id>",
    to: "<consumer-id>",
    reason: "generated-client contract",
    confidence: 90,
    evidence: [/* relative paths to the generated-client dir + the
                 matched producer's contract file */],
}
```

We deliberately do **not** introduce a sibling `contract_edges` key.
The dashboard and brief already derive contract / dependency edges
from `projects.yaml.contracts` once promotion populates the fields;
`topology.json` only carries provenance, not a second schema.

## 6. Tests we'll require before merge

Mirroring the F-101 acceptance bar (helpers + integration):

**Helper level:**
- `provider_promotion_requires_marker_dir_with_at_least_one_file`
- `provider_promotion_rejects_generated_client_dirs`
- `consumer_promotion_requires_known_producer_with_provides`
- `consumer_promotion_picks_heaviest_producer_on_ambiguity`

**Integration (`discover_and_apply` level):**
- `discover_promotes_provider_when_idl_dir_present`
- `discover_does_not_promote_consumer_when_producer_has_no_provides`
- `discover_preserves_user_set_contracts_through_promotion`

## 7. Rollout boundaries

- This goes in as a **direct push to main** with post-push CI +
  dali's review (current project flow — no PRs).
- Implementation order (per dali's N2): (1) expose `is_generated
  _client_path` and `path_contains_token` as `pub(crate)` from
  `codegraph.rs` so discovery can reuse them — **no marker-list
  fork**; (2) provider promotion (project-relative paths); (3)
  consumer promotion as a `DiscoveredEdge` with the new `reason`;
  (4) logging + tests; (5) real-workspace dry-run, raw evidence
  stays out of git.
- Before exposing `is_generated_client_path`: make it component-
  aware so a root-level `bam-idl/...` (no leading `/marker/`) is
  also caught. Today it greps for `/marker/` substrings, which
  misses a marker placed at the project root.
- Verify against a real workspace with `mst init` + `mst brief`. The
  brief's `N contract provider(s)` line should change from `0` to a
  number that matches the producer count. Raw evidence stays out of
  git per cycle 7 boundary.
- If brief shows fewer providers than expected, do NOT widen the
  marker list silently — file a F-103-followup for a separate
  discussion.

## 8. Open questions — resolved with dali

1. **`discovery_overrides.yaml` for operator overrides** — **no**.
   Existing user-set contracts already survive the merge; we don't
   need a second override surface and shouldn't expand the API.
2. **Near-miss "would have fired" logs** — **debug only**. Default
   stdout shows the summary line in §5; verbose hits live behind
   `RUST_LOG=debug` so they don't paper over the headline counts.
3. **`topology.json` schema** — **stay with `edges[]`**. New entries
   carry `reason: "generated-client contract"` + `evidence` so the
   provenance is readable, but we do not add a sibling
   `contract_edges` key. Old readers keep working unchanged.

---

Owner: 街溜子-大福 (drafting, implementation) · 大力 (review).
Cycle: 7 Patch 2.  
Status: design ready — implementation goes after dali's GO on this
revision.
