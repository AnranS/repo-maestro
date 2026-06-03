#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

export EXPERIENCE_RUN_ID="test-run"
export EXPERIENCE_RUN_ROOT="$TMP/runs"

set +e
"$ROOT/scripts/run-experience-case.sh" C5 -- bash -c 'echo stdout-line; echo stderr-line >&2; exit 7'
runner_status=$?
set -e

if [[ "$runner_status" -ne 0 ]]; then
  echo "runner should record failed commands without failing itself; got $runner_status" >&2
  exit 1
fi

case_dir="$EXPERIENCE_RUN_ROOT/test-run/C5"
test -f "$case_dir/command.txt"
test -f "$case_dir/stdout.log"
test -f "$case_dir/stderr.log"
test -f "$case_dir/exit-code.txt"
test -f "$case_dir/duration-ms.txt"
test -f "$case_dir/summary.md"

grep -Fq "bash -c" "$case_dir/command.txt"
grep -Fq "stdout-line" "$case_dir/stdout.log"
grep -Fq "stderr-line" "$case_dir/stderr.log"
grep -Fxq "7" "$case_dir/exit-code.txt"
grep -Fq "command_exit_code: 7" "$case_dir/summary.md"

mkdir -p "$TMP/latest/.maestro/runs/run-old" "$TMP/latest/.maestro/runs/run-new"
touch -t 202605240800 "$TMP/latest/.maestro/runs/run-old"
touch -t 202605240801 "$TMP/latest/.maestro/runs/run-new"
(
  cd "$TMP/latest"
  "$ROOT/scripts/run-experience-case.sh" Latest -- printf '%s\n' ".maestro/runs/<latest-run-dir>"
)

latest_dir="$EXPERIENCE_RUN_ROOT/test-run/Latest"
grep -Fxq ".maestro/runs/run-new" "$latest_dir/stdout.log"
grep -Fq ".maestro/runs/run-new" "$latest_dir/command.txt"

mkdir -p "$TMP/no-runs"
(
  cd "$TMP/no-runs"
  set +e
  "$ROOT/scripts/run-experience-case.sh" Missing -- printf '%s\n' "<latest-run-dir>" >missing.stdout 2>missing.stderr
  status=$?
  set -e
  test "$status" -eq 2
  grep -Fq "cannot resolve <latest-run-dir>" missing.stderr
)
