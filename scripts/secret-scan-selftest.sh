#!/usr/bin/env bash
#
# Self-test for the private-wordlist case-mode handling in secret-scan.sh
# (F-SCAN-001). Builds a throwaway git repo with neutral fixtures, runs the
# REAL scanner against synthetic wordlist rules, and asserts the four modes
# behave as documented. No real brand words appear here — fixtures are generic
# (AcmeWidget / offsetTop / Gadget / a neutral CJK marker).
#
# Exit 0 = all assertions pass; non-zero = at least one failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCAN="$SCRIPT_DIR/secret-scan.sh"

fail=0
pass() { printf 'ok   - %s\n' "$1"; }
bad()  { printf 'FAIL - %s\n' "$1"; fail=1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q

# Tracked fixtures (git ls-files is the scanner's scope).
printf 'const a = AcmeWidget; const b = ACMEWIDGET;\n' > pascal.txt   # Title/PascalCase
printf 'el.style.height = el.offsetTop + 2;\n'          > camel.txt    # camelCase identifier
printf 'const g = Gadget;\n'                            > noprefix.txt # mixed case
printf 'marker 示例标记 here\n'                          > cjk.txt      # non-ASCII / CJK
git add -A

WL="$tmp/wordlist.txt"   # written below each run; never git-added → not scanned

# run <wordlist-line> → scanner stdout (path:line private-wordlist | clean line)
run() {
    printf '%s\n' "$1" > "$WL"
    MAESTRO_PRIVATE_WORDLIST="$WL" SECRET_SCAN_NO_EXCLUSIONS=1 SCAN_PRIVATE=1 \
        "$SCAN" 2>/dev/null || true
}
assert_hit()   { grep -q "^$3:" <<<"$2" && pass "$1" || bad "$1 (expected $3 flagged)"; }
assert_nohit() { grep -q "^$3:" <<<"$2" && bad "$1 (did not expect $3)" || pass "$1"; }

# 1. ci: catches Titlecase / PascalCase variants.
assert_hit   "1. ci: catches Titlecase/PascalCase"            "$(run 'ci:acmewidget')" pascal.txt

# 2. cs: short token does NOT collide with a camelCase identifier...
assert_nohit "2. cs: short token avoids camelCase (offsetTop)" "$(run 'cs:top')"        camel.txt
#    ...and the control: ci: on the same short token WOULD collide (why cs: matters).
assert_hit   "2b. (control) ci: short token collides w/ offsetTop" "$(run 'ci:top')"    camel.txt

# 3. No prefix keeps the old case-sensitive behavior.
assert_nohit "3. no-prefix stays case-sensitive (misses Gadget)"  "$(run 'gadget')"     noprefix.txt
assert_hit   "3b. (control) no-prefix matches exact case"         "$(run 'Gadget')"     noprefix.txt

# 4. CJK / non-ASCII pattern matches without depending on word boundaries.
assert_hit   "4. CJK pattern matches (no word-boundary dependence)" "$(run '示例标记')"  cjk.txt

if ((fail)); then
    printf '\nsecret-scan-selftest: FAILURES above.\n' >&2
    exit 1
fi
printf '\n✓ secret-scan-selftest: all case-mode assertions passed\n'
