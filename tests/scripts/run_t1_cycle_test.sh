#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

FAKE_BIN="$TMP/bin"
mkdir -p "$FAKE_BIN"

cat >"$FAKE_BIN/maestro" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${MAESTRO_FAKE_COMMANDS:?}"

case "$*" in
  "work fake goal")
    echo "no projects registered" >&2
    exit 1
    ;;
  "bench all --json --offline")
    echo '{"fixtures":[],"summary":{"failed":1}}'
    exit 1
    ;;
  *)
    echo "ok: $*"
    exit 0
    ;;
esac
SCRIPT
chmod +x "$FAKE_BIN/maestro"

export PATH="$FAKE_BIN:$PATH"
export MAESTRO_FAKE_COMMANDS="$TMP/commands.log"
export EXPERIENCE_RUN_ROOT="$TMP/runs"

output=$("$ROOT/scripts/run-t1-cycle.sh" --run-id cycle-test)

run_root="$EXPERIENCE_RUN_ROOT/cycle-test"
metrics="$run_root/metrics.md"

grep -Fq "run_id: cycle-test" <<<"$output"
grep -Fq "metrics: $metrics" <<<"$output"
grep -Fq "T1-C4: exit 1" <<<"$output"
grep -Fq "T1-C5: exit 1" <<<"$output"

for case_id in T1-C1 T1-C2 T1-C3 T1-C4 T1-C5; do
  test -f "$run_root/$case_id/command.txt"
  test -f "$run_root/$case_id/exit-code.txt"
  grep -Fq "| $case_id |" "$metrics"
done

grep -Fxq "setup" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "providers" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "doctor" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "demo --run" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "init --analyze --root examples --agent mock" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "work fake goal" "$MAESTRO_FAKE_COMMANDS"
grep -Fxq "bench all --json --offline" "$MAESTRO_FAKE_COMMANDS"
