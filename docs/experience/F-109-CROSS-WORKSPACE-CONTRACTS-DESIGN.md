# F-109 — cross-workspace contract provider index (BACKLOG #41)

> **Scope.** Design note for letting a consumer link to a contract whose
> **producer lives in a sibling repo** (a different workspace root).
> **Rules + decisions only**, no raw monorepo content; the neutral fixture
> shape (`shared-lib` provider in repo A, `app-alpha` consumer in repo B)
> stands in for the real layout. Owner: 街溜子-大福 (drafting, impl) ·
> 大力 (review). Status: design — impl after dali's GO.

## 1. Problem (from the round-2 dogfood)

`promote_contracts` builds its `provider_index` **only** from
`report.projects` — the projects under the single scanned root
(`src/config/discovery/mod.rs:515`). On a real multi-monorepo workspace,
discovery promoted the in-tree contract **providers** but found **0
consumers**, because the services consuming those contracts live in a
**separate sibling repo** outside the scanned root. The generated-client
subdirs are right there in the consumer, but the matching producer name
isn't in the index, so `provides`/`consumes` never link across the repo
boundary. (Surfaced by the `RUST_LOG=debug` "saw generated-client
subdirs but no registered producer matched" near-miss logs.)

## 2. Goals / non-goals

**Goals:**
- A consumer in workspace B can link `consumes` to a producer registered
  in a declared sibling workspace A.
- **Opt-in** — a single-root workspace is byte-for-byte unchanged.
- Reuse the existing F-103 name-match + false-positive guards; don't
  loosen them across the boundary.

**Non-goals:**
- Not auto-discovering sibling repos — they're explicitly declared.
- Not fetching/cloning remote repos — siblings are local checkouts the
  user already has.
- Not a global contract registry / service mesh. Just: "also look in
  these roots I told you about."

## 3. Config — declaring siblings

`defaults.sibling_workspaces: Vec<String>` in `.maestro/projects.yaml`
(serde default empty). Each entry is a path to a sibling **workspace
root** (one that has its own `.maestro/projects.yaml`). Empty ⇒ today's
single-root behavior, unchanged.

```yaml
defaults:
  sibling_workspaces:
    - ../backend-monorepo      # has its own .maestro/projects.yaml
```

Relative paths resolve against this workspace root. Absolute paths are
allowed. Missing / unreadable sibling roots are **skipped with a
warning**, never an error (the user may not have that repo checked out).

## 4. The index extension (the whole change)

`promote_contracts`'s `provider_index` is the single extension point.
After building it from `report.projects`, **fold in providers from each
sibling workspace**:

- For each declared sibling root, load its `.maestro/projects.yaml`
  (best-effort; skip on error).
- For each sibling project that has `contracts.provides` set, add
  `name → (sibling_project_abs_path, provides_rel)` to the index, but
  **only if the name isn't already a local provider** (local wins — never
  let a sibling shadow an in-tree producer).
- Tag these entries as cross-workspace so the consumer pass can mark the
  edge.

Then `find_generated_client_match` runs **unchanged** against the unioned
index — the consumer's generated-client subdir name matches a sibling
producer the same way it matches a local one (same normalized
name-match + the existing length / multi-word false-positive guards from
F-103, so a coincidental short name in a sibling can't false-link).

## 5. What `consumes` is set to, and freshness

`consumes` = `relative_to(consumer.path, sibling_producer_contract_abs)`
— the same edge-loop convention used today, just resolving across roots
(e.g. `../../backend-monorepo/shared-lib/idl/user.proto`). This keeps
downstream readers (`consumed_contract_section`) working: they
`read_to_string` the resolved path and get the sibling's **current**
contract content at run time (freshness = whatever's on disk now). If the
sibling moved or isn't checked out, the read fails gracefully → no
injected contract, no crash (existing `.ok()` path).

The promoted edge carries `reason: "cross-workspace generated-client
contract"` (distinct from the in-tree `"generated-client contract"`) so
the provenance is auditable and the dashboard can tell them apart.

## 6. False-positive / safety guards

| Guard | What it stops |
|---|---|
| Opt-in (empty `sibling_workspaces` = off) | any behavior change for single-root users |
| Local provider wins over sibling of same name | a sibling shadowing an in-tree producer |
| Reuse F-103 name-match guards (normalized len ≥ 7 / multi-word) | a short coincidental name cross-linking |
| Missing/unreadable sibling root → skip + warn | a half-checked-out workspace breaking discovery |
| Only reads roots the user **declared** | reading arbitrary sibling trees (security) |

## 7. Tests

- `sibling_provider_links_cross_workspace_consumer` — provider in root A
  (its projects.yaml has `provides`), consumer with a matching
  generated-client subdir in root B + `sibling_workspaces: [A]` →
  `consumes` set, cross-workspace edge emitted.
- `local_provider_wins_over_sibling_of_same_name` — same name in both;
  the in-tree one is used.
- `empty_sibling_workspaces_is_unchanged` — no siblings ⇒ identical to
  current single-root promotion (regression guard).
- `missing_sibling_root_is_skipped_not_fatal`.
- `cross_workspace_consumes_path_resolves_to_the_sibling_contract_file`
  (round-trips through `read_to_string`, mirroring F-103's file-path test).

## 8. Rollout

- Direct push to main + post-push CI + dali review.
- Order: (1) `defaults.sibling_workspaces` config + serde default;
  (2) extend `provider_index` in `promote_contracts` with sibling
  providers (local-wins) + the skip-on-error/warn path; (3) cross-
  workspace edge `reason`; (4) tests; (5) docs (projects-yaml reference +
  a short note in the discovery/contracts doc).
- Default off ⇒ no behavior change until a user declares siblings.

## 9. Open questions for dali

1. **`consumes` path: relative (proposed) vs absolute** when the producer
   is cross-root. Relative is consistent with the existing convention and
   survives a repo move *iff* the sibling stays at the same relative
   location; absolute is machine-specific and leaks an absolute path into
   `projects.yaml`. I lean relative. Agree?
2. **Where siblings are declared**: `defaults.sibling_workspaces`
   (proposed) vs a separate `.maestro/workspaces.yaml`. I lean the
   `defaults` field — one fewer file, and it's a workspace-level setting.
3. **Should the sibling's `provides` be auto-promoted if unset?** No —
   v1 only indexes siblings that **already** have `provides` (set by their
   own `init`); we don't run discovery into a sibling tree. Confirm that's
   an acceptable v1 boundary.
