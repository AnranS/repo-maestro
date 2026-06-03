#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/run-experience-case.sh <case-id> -- <command> [args...]

Records one dogfood command transcript under:
  docs/cases/T1-runs/<timestamp>/<case-id>/

Environment overrides:
  EXPERIENCE_RUN_ID    deterministic run id for a batch
  EXPERIENCE_RUN_ROOT  output root, useful for tests

Command placeholders:
  <latest-run-dir>     replaced with newest directory name under .maestro/runs

The runner exits 0 after recording the transcript, even when the wrapped
command fails. Inspect exit-code.txt or summary.md for the command result.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$#" -lt 3 || "${2:-}" != "--" ]]; then
  usage >&2
  exit 2
fi

case_id="$1"
shift 2

if [[ -z "$case_id" ]]; then
  echo "case id must not be empty" >&2
  exit 2
fi

repo_root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
run_id="${EXPERIENCE_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
run_root="${EXPERIENCE_RUN_ROOT:-$repo_root/docs/cases/T1-runs}"
case_dir="$run_root/$run_id/$case_id"

mkdir -p "$case_dir"

quote_command() {
  local quoted=()
  local arg
  for arg in "$@"; do
    quoted+=("$(printf '%q' "$arg")")
  done
  printf '%s\n' "${quoted[*]}"
}

latest_run_dir_name() {
  local runs_dir=".maestro/runs"
  [[ -d "$runs_dir" ]] || return 1
  local latest
  latest=$(find "$runs_dir" -mindepth 1 -maxdepth 1 -type d -print | while IFS= read -r path; do
    if [[ "$(uname -s)" == "Darwin" ]]; then
      printf '%s\t%s\n' "$(stat -f %m "$path")" "$path"
    else
      printf '%s\t%s\n' "$(stat -c %Y "$path")" "$path"
    fi
  done | sort -rn | head -n 1 | cut -f 2-)
  [[ -n "$latest" ]] || return 1
  basename "$latest"
}

resolved_args=()
latest_run_dir=""
for arg in "$@"; do
  if [[ "$arg" == *"<latest-run-dir>"* ]]; then
    if [[ -z "$latest_run_dir" ]]; then
      latest_run_dir=$(latest_run_dir_name) || {
        echo "cannot resolve <latest-run-dir>: no .maestro/runs entry found" >&2
        exit 2
      }
    fi
    arg="${arg//<latest-run-dir>/$latest_run_dir}"
  fi
  resolved_args+=("$arg")
done
set -- "${resolved_args[@]}"

started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started_sec=$(date +%s)
command_text=$(quote_command "$@")

printf '%s\n' "$command_text" >"$case_dir/command.txt"
{
  printf 'case_id: %s\n' "$case_id"
  printf 'run_id: %s\n' "$run_id"
  printf 'started_at: %s\n' "$started_at"
  printf 'cwd: %s\n' "$PWD"
  printf 'git_head: %s\n' "$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || echo unknown)"
} >"$case_dir/metadata.txt"

set +e
"$@" >"$case_dir/stdout.log" 2>"$case_dir/stderr.log"
command_status=$?
set -e

ended_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
ended_sec=$(date +%s)
duration_ms=$(( (ended_sec - started_sec) * 1000 ))

printf '%s\n' "$command_status" >"$case_dir/exit-code.txt"
printf '%s\n' "$duration_ms" >"$case_dir/duration-ms.txt"

cat >"$case_dir/summary.md" <<SUMMARY
# Experience Case Transcript

- case_id: $case_id
- run_id: $run_id
- started_at: $started_at
- ended_at: $ended_at
- duration_ms: $duration_ms
- command_exit_code: $command_status

## Command

\`\`\`bash
$command_text
\`\`\`

## Files

- stdout: \`stdout.log\`
- stderr: \`stderr.log\`
- metadata: \`metadata.txt\`
SUMMARY

printf 'recorded %s (exit %s)\n' "$case_dir" "$command_status"
exit 0
