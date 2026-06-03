#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/run-t1-cycle.sh [--run-id <id>] [--run-root <path>] [--maestro <bin>]

Runs the Tier 1 cold-start gauntlet cases T1-C1..T1-C5 through
scripts/run-experience-case.sh, writes a metrics.md template, and prints a
summary of transcript paths and command exit codes.

Environment defaults:
  EXPERIENCE_RUN_ID    override generated run id
  EXPERIENCE_RUN_ROOT  override docs/cases/T1-runs
  MAESTRO_BIN          override maestro binary
USAGE
}

repo_root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
runner="$repo_root/scripts/run-experience-case.sh"

run_id="${EXPERIENCE_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
run_root="${EXPERIENCE_RUN_ROOT:-$repo_root/docs/cases/T1-runs}"
maestro_bin="${MAESTRO_BIN:-maestro}"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --run-id)
      if [[ "$#" -lt 2 ]]; then
        echo "--run-id requires a value" >&2
        exit 2
      fi
      run_id="$2"
      shift 2
      ;;
    --run-root)
      if [[ "$#" -lt 2 ]]; then
        echo "--run-root requires a value" >&2
        exit 2
      fi
      run_root="$2"
      shift 2
      ;;
    --maestro)
      if [[ "$#" -lt 2 ]]; then
        echo "--maestro requires a value" >&2
        exit 2
      fi
      maestro_bin="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -x "$runner" ]]; then
  echo "missing executable runner: $runner" >&2
  exit 1
fi

mkdir -p "$run_root/$run_id"
export EXPERIENCE_RUN_ID="$run_id"
export EXPERIENCE_RUN_ROOT="$run_root"

tmp_dirs=()
cleanup() {
  local dir
  for dir in "${tmp_dirs[@]:-}"; do
    rm -rf "$dir"
  done
}
trap cleanup EXIT

run_at_repo_root() {
  local case_id="$1"
  shift
  (
    cd "$repo_root"
    "$runner" "$case_id" -- "$@"
  )
}

run_in_empty_workspace() {
  local case_id="$1"
  shift
  local workspace
  workspace=$(mktemp -d)
  tmp_dirs+=("$workspace")
  (
    cd "$workspace"
    "$runner" "$case_id" -- "$@"
  )
}

write_metrics_template() {
  local metrics="$run_root/$run_id/metrics.md"
  cat >"$metrics" <<'METRICS'
| case | time_to_first_report_s | commands_count | manual_interventions | error_message_quality | artifact_discoverability | recovery_success | notes |
|---|---:|---:|---:|---:|---:|---|---|
| T1-C1 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C2 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C3 |  |  |  |  |  | yes/no/n/a |  |
| T1-C4 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C5 | n/a |  |  |  |  | yes/no/n/a |  |
METRICS
}

case_exit() {
  local case_id="$1"
  local exit_file="$run_root/$run_id/$case_id/exit-code.txt"
  if [[ -f "$exit_file" ]]; then
    tr -d '\n' <"$exit_file"
  else
    printf 'missing'
  fi
}

run_at_repo_root T1-C1 "$maestro_bin" setup
run_at_repo_root T1-C2 bash -c '"$1" setup && "$1" providers && "$1" doctor' _ "$maestro_bin"
run_at_repo_root T1-C3 bash -c '"$1" demo --run && "$1" init --analyze --root examples --agent mock' _ "$maestro_bin"
run_in_empty_workspace T1-C4 "$maestro_bin" work "fake goal"
run_at_repo_root T1-C5 "$maestro_bin" bench all --json --offline

write_metrics_template

cat <<SUMMARY
T1 cycle complete
run_id: $run_id
transcripts: $run_root/$run_id
metrics: $run_root/$run_id/metrics.md
cases:
  T1-C1: exit $(case_exit T1-C1)
  T1-C2: exit $(case_exit T1-C2)
  T1-C3: exit $(case_exit T1-C3)
  T1-C4: exit $(case_exit T1-C4)
  T1-C5: exit $(case_exit T1-C5)
SUMMARY
