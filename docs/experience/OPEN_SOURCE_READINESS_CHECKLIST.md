# Open-source readiness checklist

> **Scope.** This checklist is a **rules-only** document. It captures the
> policy the project must hold *before* a public release. It deliberately
> contains no codenames, internal paths, internal commit hashes, or other
> raw inventory: the checklist file itself ships with the open-source
> export, so anything raw inside it becomes a leak.
>
> Concrete findings, command outputs, and environment-specific wordlists
> are kept **out of the repository** (local notes, a private wiki, etc.).
> The repository carries the rules and the tools; the operator carries
> the findings.

---

## Strategy decision (already taken)

Open-source happens via a **clean export repository**, not by flipping
the current working repository's visibility. The current repo stays
private indefinitely and serves as the internal working history; the
public repo is a fresh project initialised from a sanitised snapshot at
release time. Rationale:

- PR comments, CR metadata, contributor email addresses, dependency
  caches, and historical attachments would otherwise leak into a
  visibility flip.
- A `git filter-repo` + force-push approach is not robust enough on its
  own: it doesn't touch issue comments, PR review threads, GitHub-side
  caches, or local clones contributors have already taken.
- Starting clean is the only path that survives audit.

The clean-export flow is described in §3.

---

## 1 — Standing gate (every push to main)

Recorded in `CONTRIBUTING.md` so contributors can self-serve. The local
verify gate (`scripts/verify-before-push.sh`) handles the
day-to-day enforcement:

| Step | Default | Notes |
|---|---|---|
| `cargo fmt --all -- --check` | always | CI also runs it |
| `cargo clippy --all-targets … -D warnings` | always | |
| `cargo test --all-targets …` | always (lib only when `QUICK=1`) | |
| `pnpm -C web build` | always except `SKIP_WEB=1`/`QUICK=1` | missing `pnpm` is a hard fail in the default gate |
| `scripts/secret-scan.sh` | OFF by default, ON when `SCAN_PRIVATE=1` | recommended for any commit that touches docs/comments/fixtures |

Contributors should run at least `QUICK=1` before pushing a code change
and the full gate (no flags) at least once per branch.

## 2 — Release-readiness gate (the GO/NO-GO bar)

A release is only allowed after **every** rule below holds. None of the
rules require any specific local state beyond a fresh clone of the
public repository.

### 2.1 Tools and CI

- [ ] `main` is at a green CI run (rust stable + web + features).
- [ ] `SCAN_PRIVATE=1 ./scripts/verify-before-push.sh` exits 0 on a
      fresh clone, with the operator's private wordlist loaded.
- [ ] `MAESTRO_PRIVATE_WORDLIST` is set to a wordlist that covers the
      release context (organisation codenames, internal domains,
      environment-specific paths, employee handle patterns). The
      wordlist file lives **outside** the repository.
- [ ] `./scripts/secret-scan.sh` also exits 0 with the wordlist unset
      (i.e. the built-in generic categories are clean on tracked files).

### 2.2 Documentation surfaces

- [ ] `README.md` opens with a working **quickstart** path:
      install → init → demo run → first report, no prior knowledge of
      the project assumed.
- [ ] `CONTRIBUTING.md` covers branching, the verify gate, secret-scan,
      and PR conventions.
- [ ] `SECURITY.md` lists the responsible-disclosure channel.
- [ ] `LICENSE` is present at the project root.
- [ ] A changelog / release notes mechanism is described (even if
      lightweight: "release notes live in GitHub releases").
- [ ] `CONTRIBUTING.md` calls out which subsystems are stable vs
      experimental so external users don't build on something that
      may move.

### 2.3 Install / build paths

- [ ] At least two install paths are documented and verified on a
      fresh machine: from-source (`cargo install --path .` or
      equivalent) and from a release artefact (binary, brew tap, etc.,
      if available).
- [ ] Building from scratch on a stock Linux and macOS runner takes
      the documented step list and nothing else (no internal tap,
      no SSO-protected mirror).

### 2.4 Demo and first-run UX

- [ ] `maestro demo --run` works end-to-end on a fresh clone with no
      LLM credentials. The output and report paths match the README.
- [ ] `maestro --help` (and one level down: `maestro work --help`,
      `maestro chat --help`, `maestro tui --help`) prints accurate,
      non-internal text.
- [ ] A first-time user can complete the README quickstart in under
      ten minutes on a stock laptop.

### 2.5 Ignored-paths boundary

`docs/design/` and a small number of other paths are `.gitignore`d.
`secret-scan.sh` scopes to `git ls-files`, so ignored paths are
intentionally not scanned. The release flow must therefore:

- [ ] Confirm that no ignored local directory is being packaged into
      the clean export tarball / repo. The clean-export script in §3
      uses `git ls-files` (not `cp -r`) for exactly this reason.
- [ ] Confirm that no test fixture under bench/scenarios/*/expected/
      ships with content that's still internal. The default scanner
      excludes that directory; release-time scans must run with
      `SECRET_SCAN_NO_EXCLUSIONS=1` so every shipped byte is in scope.

## 3 — Clean-export flow

The mechanics for producing a publishable snapshot. The flow stays in
this document as a rule-set; the operator runs it locally near the
release date.

```bash
# 1. Start from a green private repo. Tag the snapshot point so the
#    internal history retains a marker that maps to the public initial
#    commit.
git tag -a internal/public-snapshot-vX.Y -m "snapshot for public release vX.Y"

# 2. Run the readiness gate end-to-end. Stop here if §2 has any
#    unchecked item.
SCAN_PRIVATE=1 ./scripts/verify-before-push.sh

# 3. Build a sanitised tree from `git ls-files` (NOT `cp -r` — this is
#    how we enforce that ignored paths are not packaged). NUL-delimited
#    loop so paths with whitespace / special chars are safe; `cp -p`
#    preserves the executable bit on scripts. The loop variable is
#    `relpath` (not `path`) because in zsh `path` is a special array
#    that mirrors $PATH — using it as a loop variable would clobber
#    PATH mid-loop and break every subsequent command.
EXPORT="$(mktemp -d)"
git ls-files -z | while IFS= read -r -d '' relpath; do
    mkdir -p "$EXPORT/$(dirname "$relpath")"
    cp -p "$relpath" "$EXPORT/$relpath"
done

# 4. Initialise the export tree as a new repo BEFORE running the
#    scanner — `secret-scan.sh` uses `git ls-files`, so it needs an
#    actual git repo to operate on. We init + stage everything first,
#    then scan in release mode (no exclusions), then commit only after
#    the scan is green.
PUB="$EXPORT-repo"
git init -q "$PUB"
cp -a "$EXPORT/." "$PUB/"
(
    cd "$PUB"
    git add -A
    SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 \
        ./scripts/secret-scan.sh
)

# 5. Commit only after the no-exclusions scan exits 0. The internal
#    history is intentionally NOT carried over — clean export starts
#    from a single initial commit.
(
    cd "$PUB"
    git commit -q -m "Initial public release vX.Y"
)

# 6. Push to the public remote (separate org/account). After push,
#    archive the private repository as read-only.
```

The first published commit always reads `Initial public release vX.Y`.
No squash of the internal history is attempted — it is left behind on
purpose.

## 4 — Standing rules going forward

Once the public release is out, these rules apply to every subsequent
commit. They are duplicates of items elsewhere; gathered here so the
review boundary is small and explicit.

- Commits never include internal codenames, paths, handles, or commit
  hashes — not in source, comments, test fixtures, doc bodies, or
  commit messages.
- `OPEN_SOURCE_READINESS_CHECKLIST.md` (this file) and
  `secret-scan.sh` never grow a checked-in wordlist of internal terms.
- New work that needs example identifiers uses neutral placeholders
  (`app_alpha`, `service_beta`, `shared-lib`, etc.) the same way the
  existing fixtures do.
- Deferred risks tracked in `docs/experience/CYCLE-2-REPORT.md` are
  picked up before the next release; the readiness gate is not
  considered green until they're triaged.

---

Owner: 街溜子-大福 (implementation) · 大力 (review).
Initial draft: 2026-05-28.
