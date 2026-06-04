#!/usr/bin/env bash
#
# Generic secret / internal-string scanner for the tracked working tree.
#
# Categories scanned (built-in, intentionally GENERIC — no specific
# internal codenames or domains are ever inlined here):
#
#   abs-local-path        Hard-coded /Users/<name>/... or /home/<name>/... paths
#   corp-email            Email patterns on corporate-style hosts
#                         (foo@bar-corp.example, foo@some-org.io etc.
#                         — the script flags suspicious local-part@TLD
#                         shapes, not a curated list)
#   bearer-token          Bearer-style tokens, slack-style xoxb-... etc.
#   aws-keylike           AKIA / ASIA-prefixed identifier shapes
#   ssh-keylike           BEGIN ...PRIVATE KEY headers
#
# Per-environment private wordlists are supported via the env var
#
#   MAESTRO_PRIVATE_WORDLIST=/abs/path/to/wordlist.txt
#
# The wordlist file is read at runtime; its PATH is never persisted in
# the repo and its CONTENTS are never echoed to stdout. Contributors
# keep their own lists locally (or in a private doc).
#
# Wordlist matching is NOT implicitly case-insensitive. Each line may carry
# an explicit case-mode prefix:
#
#   ci:<regex>   case-insensitive  (catches Titlecase / PascalCase variants;
#                                    use for brand words that appear in mixed case)
#   cs:<regex>   case-sensitive    (use for short tokens that would otherwise
#                                    collide with camelCase identifiers, e.g. a
#                                    3-letter codename vs `offsetTop`)
#   <regex>      no prefix → case-sensitive (backward compatible default, so a
#                                    pre-existing wordlist never suddenly flips
#                                    to case-insensitive and floods false positives)
#
# A rule that needs case folding MUST opt in with `ci:`.
#
# Output format (always):
#
#   path:line  category
#
# The matched text is NEVER printed — flagging which file/line tripped
# which class is enough to act, and printing the match would itself be
# the kind of leak this script is meant to prevent.
#
# Exit codes:
#   0  no hits across all enabled categories
#   1  hits found (table printed above)
#   2  invocation / environment problem (e.g. wordlist path unreadable)
#
# Default scope: tracked files in the worktree (git ls-files).  History
# scanning is intentionally NOT done here — for the open-source export
# flow, run a separate clean-export rebuild instead of trying to scrub
# the existing history.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

scope_files() {
    # Default skip set:
    #   - build artifacts and lock files (not source)
    #   - bench/scenarios/*/expected/ — fixture diffs / outputs that
    #     legitimately contain example absolute paths and aren't shipped
    #     at runtime.
    #
    # Release-readiness mode (SECRET_SCAN_NO_EXCLUSIONS=1) drops the
    # bench/scenarios exclusion so every byte that would actually ship
    # in the open-source export gets scanned. Build artifacts and lock
    # files stay excluded — they're either gitignored or not source.
    local skip='^(\.git/|Cargo\.lock$|web/(dist|node_modules)/|web/pnpm-lock|target/)'
    if [[ "${SECRET_SCAN_NO_EXCLUSIONS:-0}" != "1" ]]; then
        skip='^(\.git/|Cargo\.lock$|web/(dist|node_modules)/|web/pnpm-lock|target/|bench/scenarios/.+/expected/)'
    fi
    git ls-files | grep -vE "$skip"
}

# Built-in pattern table. Each row is `category\tregex` (extended).
# Patterns deliberately err on the side of generic shapes; the private
# wordlist below is where environment-specific names go.
declare -a CATEGORIES=(
    'abs-local-path'        '/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/'
    'corp-email'            '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.(corp|internal|priv|com\.(cn|sg))'
    'bearer-token'          'xox[bopas]-[A-Za-z0-9-]{10,}|gh[posu]_[A-Za-z0-9]{30,}|sk-[A-Za-z0-9]{20,}'
    'aws-keylike'           'AKIA[0-9A-Z]{12,}|ASIA[0-9A-Z]{12,}'
    'ssh-keylike'           '-----BEGIN [A-Z ]*PRIVATE KEY-----'
)

# Collect optional environment-specific patterns from a private wordlist.
# Format: one regex per line; lines starting with `#` are comments. A line may
# carry a `ci:` / `cs:` case-mode prefix (see header); no prefix → case-sensitive.
# PRIVATE_PATTERNS and PRIVATE_MODE_FLAGS are parallel arrays: index i holds the
# bare regex and its grep flag ("" for case-sensitive, "-i" for case-insensitive).
PRIVATE_PATTERNS=()
PRIVATE_MODE_FLAGS=()
if [[ -n "${MAESTRO_PRIVATE_WORDLIST:-}" ]]; then
    if [[ ! -r "$MAESTRO_PRIVATE_WORDLIST" ]]; then
        printf 'secret-scan: cannot read MAESTRO_PRIVATE_WORDLIST=%s\n' \
            "$MAESTRO_PRIVATE_WORDLIST" >&2
        exit 2
    fi
    # Read lines, drop comments + blanks, split off any case-mode prefix.
    while IFS= read -r line; do
        # shellcheck disable=SC2001
        line=$(printf '%s\n' "$line" | sed 's/[[:space:]]*$//')
        [[ -z "$line" || "$line" =~ ^# ]] && continue
        mode_flag=''
        case "$line" in
            ci:*) mode_flag='-i'; line="${line#ci:}" ;;
            cs:*) mode_flag='';   line="${line#cs:}" ;;
            # no prefix → case-sensitive (mode_flag stays empty)
        esac
        # A prefix with an empty body (e.g. a bare `ci:`) would become an
        # empty regex that matches every line — skip it.
        [[ -z "$line" ]] && continue
        PRIVATE_PATTERNS+=("$line")
        PRIVATE_MODE_FLAGS+=("$mode_flag")
    done <"$MAESTRO_PRIVATE_WORDLIST"
fi

# scan_one_pattern <category> <regex> [extra-grep-flags]
scan_one_pattern() {
    local category="$1"
    local pattern="$2"
    local extra="${3:-}"
    # -H always prefix filename even on single-file runs; -I skip binary;
    # -n line numbers; -E extended regex. `extra` carries optional per-pattern
    # flags (e.g. -i for a `ci:` wordlist rule); it is intentionally unquoted so
    # an empty value expands to nothing. Pipe through awk so the matched text
    # never reaches stdout — only path:line + category leave here.
    scope_files \
        | xargs -I{} grep -HInE $extra -- "$pattern" {} 2>/dev/null \
        | awk -v cat="$category" -F: '{ printf "%s:%s  %s\n", $1, $2, cat }' || true
}

HIT_FILE=$(mktemp -t maestro-secret-scan.XXXXXX)
trap 'rm -f "$HIT_FILE"' EXIT

i=0
total=${#CATEGORIES[@]}
while ((i < total)); do
    cat_name="${CATEGORIES[$i]}"
    cat_re="${CATEGORIES[$((i + 1))]}"
    scan_one_pattern "$cat_name" "$cat_re" >>"$HIT_FILE"
    i=$((i + 2))
done

# Run the private wordlist patterns under a fixed category so we don't
# echo the wordlist's contents into the output. Each pattern carries its own
# case mode (PRIVATE_MODE_FLAGS, parallel array).
if ((${#PRIVATE_PATTERNS[@]} > 0)); then
    pidx=0
    while ((pidx < ${#PRIVATE_PATTERNS[@]})); do
        scan_one_pattern 'private-wordlist' \
            "${PRIVATE_PATTERNS[$pidx]}" "${PRIVATE_MODE_FLAGS[$pidx]}" >>"$HIT_FILE"
        pidx=$((pidx + 1))
    done
fi

if [[ -s "$HIT_FILE" ]]; then
    sort -u "$HIT_FILE"
    printf '\nsecret-scan: hits found above. Fix or set MAESTRO_PRIVATE_WORDLIST and re-run.\n' >&2
    exit 1
fi

mode='default scope'
if [[ "${SECRET_SCAN_NO_EXCLUSIONS:-0}" == "1" ]]; then
    mode='release scope (no exclusions)'
fi
printf '✓ secret-scan clean — %s (%d built-in categor%s, %d private pattern(s))\n' \
    "$mode" \
    $((total / 2)) "$(if ((total / 2 == 1)); then echo y; else echo ies; fi)" \
    "${#PRIVATE_PATTERNS[@]}"
