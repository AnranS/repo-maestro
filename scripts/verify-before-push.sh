#!/usr/bin/env bash
#
# Local verify gate for code changes before pushing to main.
#
# Why: maestro follows a direct-to-main convention. CI runs after push, which
# means a red build sits on origin/main until the next push fixes it. This
# script lets a contributor run the same checks locally and avoid the round
# trip. It is voluntary — there is no git hook — but a green run here is the
# expected pre-condition for `git push origin main`.
#
# Usage:
#   ./scripts/verify-before-push.sh                   # full gate (default):
#                                                       web build → fmt → clippy → all-targets test
#   QUICK=1 ./scripts/verify-before-push.sh           # skip web build, fmt + clippy + lib tests only
#   SKIP_WEB=1 ./scripts/verify-before-push.sh        # skip web build, full Rust gate
#   SCAN_PRIVATE=1 ./scripts/verify-before-push.sh    # also run secret-scan first
#
# The web build always runs FIRST in the default gate because the Rust
# crate embeds web/dist/ via rust_embed; without that folder, even fmt
# is fine but clippy/test fail with a cryptic "method `get` not found".
# QUICK / SKIP_WEB bypass the build itself but still require an
# existing web/dist/index.html — we fail-fast with a clear hint if
# that's missing.
#
# Exits non-zero on the FIRST failure with a short hint at the bottom so the
# contributor knows the exact command to re-run + fix.
#
# Docs-only commits (only *.md, docs/**, LICENSE, etc.) can be pushed without
# running this — the gate is for code changes, not editorial ones.

set -euo pipefail

# Pretty step printer + failure breadcrumb.
step() {
    printf '\n\033[1;36m▸ %s\033[0m\n' "$*"
}
fail() {
    printf '\n\033[1;31m✗ verify-before-push failed at: %s\033[0m\n' "$1"
    printf '   Re-run that step locally:\n     %s\n' "$2"
    printf '   When green, push again.\n'
    exit 1
}

# Anchor to repo root so the script works from any subdir.
cd "$(git rev-parse --show-toplevel)"

QUICK="${QUICK:-0}"
SKIP_WEB="${SKIP_WEB:-0}"
SCAN_PRIVATE="${SCAN_PRIVATE:-0}"

# Optional pre-step: secret / internal-string scan. OFF by default so
# day-to-day pushes aren't gated on a scan that's most relevant near
# an open-source export. Recommended ON for the release-readiness gate
# (see docs/experience/OPEN_SOURCE_READINESS_CHECKLIST.md once it lands).
if [[ "$SCAN_PRIVATE" == "1" ]]; then
    step "0/4  secret-scan (SCAN_PRIVATE=1)"
    ./scripts/secret-scan.sh \
        || fail "secret-scan" "fix the flagged file:line entries or extend MAESTRO_PRIVATE_WORDLIST"
fi

# Step ordering note: the Rust gate (fmt/clippy/test) needs
# `web/dist/index.html` to exist because `src/server/ui.rs` embeds
# `web/dist/` via the rust_embed proc-macro. Without it, clippy/test
# fail on a fresh clone with a cryptic "method `get` not found on
# WebAsset". So the web build runs FIRST in the default gate.
# QUICK / SKIP_WEB still skip the build itself, but in that case we
# fail-fast with a clear message if web/dist is also missing — the
# rust gate is guaranteed to fail anyway, and a clear hint up front
# beats four lines of macro errors.
if [[ "$SKIP_WEB" == "1" || "$QUICK" == "1" ]]; then
    step "1/4  web build  (skipped — QUICK or SKIP_WEB)"
    if [[ ! -f web/dist/index.html ]]; then
        fail "web/dist missing" "pnpm -C web install --frozen-lockfile --config.dangerously-allow-all-builds=true && pnpm -C web build   # rust gate needs the embedded UI"
    fi
else
    step "1/4  pnpm -C web build"
    # In the full gate, missing pnpm is a HARD failure — quietly skipping
    # would let a contributor push web changes that never got built.
    # Use SKIP_WEB=1 or QUICK=1 to bypass intentionally.
    if ! command -v pnpm >/dev/null 2>&1; then
        fail "web build" "install pnpm (https://pnpm.io) — or re-run with SKIP_WEB=1 if you intentionally didn't touch web/"
    fi
    # `pnpm install` is idempotent and skipped quickly if the lockfile
    # is already satisfied, so this is cheap on warm clones and the
    # only path on fresh ones. --config.dangerously-allow-all-builds
    # approves esbuild's build script non-interactively; without it,
    # pnpm 10+ silently leaves the native binary missing and Vite's
    # bundling fails.
    pnpm -C web install --frozen-lockfile --config.dangerously-allow-all-builds=true \
        || fail "web install" "pnpm -C web install --frozen-lockfile --config.dangerously-allow-all-builds=true"
    pnpm -C web build || fail "web build" "pnpm -C web build   # or SKIP_WEB=1 if intentionally web-skipped"
fi

step "2/4  cargo fmt --all -- --check"
cargo fmt --all -- --check || fail "fmt" "cargo fmt --all"

step "3/4  cargo clippy --all-targets --features codegraph -- -D warnings"
cargo clippy --all-targets --features codegraph -- -D warnings \
    || fail "clippy" "cargo clippy --all-targets --features codegraph --fix --allow-dirty   # then re-check"

if [[ "$QUICK" == "1" ]]; then
    step "4/4  cargo test --lib --features codegraph  (QUICK=1, lib only)"
    cargo test --lib --features codegraph \
        || fail "lib tests" "cargo test --lib --features codegraph"
else
    step "4/4  cargo test --all-targets --features codegraph"
    cargo test --all-targets --features codegraph \
        || fail "tests" "cargo test --all-targets --features codegraph   # or QUICK=1 for a faster lib-only pass"
fi

printf '\n\033[1;32m✓ all checks passed — safe to push\033[0m\n'
