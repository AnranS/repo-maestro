#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

FAKE_MAESTRO="$TMP/maestro"
COMMANDS="$TMP/commands.log"

cat >"$FAKE_MAESTRO" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${MAESTRO_FAKE_COMMANDS:?}"

case "$1" in
  init)
    mkdir -p .maestro
    echo "initialized"
    ;;
  work)
    mkdir -p plans
    cat >plans/demo.yaml <<'YAML'
tasks:
  - id: T_change_demo_core
    command: |
      pnpm test
YAML
    echo "workflow ready: plans/demo.yaml"
    ;;
  plan)
    echo "ok · 0 errors · 0 warnings"
    ;;
  run)
    mkdir -p .maestro/runs/run-1
    cat >.maestro/runs/run-1/RUN_STATE.json <<'JSON'
{"run_id":"run-1","status":"done"}
JSON
    cat >.maestro/runs/run-1/REPORT.md <<'REPORT'
# Demo report
REPORT
    echo "run done"
    ;;
  ui)
    echo "→ dashboard:  http://127.0.0.1:7788"
    ;;
  *)
    echo "unexpected command: $*" >&2
    exit 2
    ;;
esac
SCRIPT
chmod +x "$FAKE_MAESTRO"

export MAESTRO_BIN="$FAKE_MAESTRO"
export MAESTRO_FAKE_COMMANDS="$COMMANDS"
export MAESTRO_DEMO_SKIP_UI_WAIT=1

output=$("$ROOT/scripts/run-complex-demo.sh" --port 7788 --no-browser)

grep -Fq "web UI: http://127.0.0.1:7788" <<<"$output"
grep -Fq "workspace kept for Web UI" <<<"$output"
for _ in {1..20}; do
  if grep -Fxq "ui --host 127.0.0.1 --port 7788" "$COMMANDS" 2>/dev/null; then
    exit 0
  fi
  sleep 0.05
done
grep -Fxq "ui --host 127.0.0.1 --port 7788" "$COMMANDS"
