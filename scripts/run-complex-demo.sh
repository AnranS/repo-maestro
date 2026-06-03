#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/run-complex-demo.sh [--keep-workspace] [--port <port>] [--no-browser]

Runs a self-contained Maestro demo against the pnpm monorepo fixture:
  1. builds the release binary,
  2. copies examples/monorepo-pnpm into a temporary git repo,
  3. analyzes the workspace,
  4. generates a dependency-aware DAG,
  5. patches demo verification commands to deterministic echo checks,
  6. executes the DAG with the mock agent,
  7. prints the latest run state and report excerpt,
  8. starts the Maestro Web UI for the demo workspace.

Options:
  --keep-workspace  accepted for compatibility; Web UI runs keep the workspace
  --port <port>     Web UI port (default: 7777)
  --no-browser      Start the Web UI without opening the browser

Environment:
  MAESTRO_BIN       use an existing Maestro binary instead of building release
USAGE
}

keep_workspace=0
no_browser=0
port=7777
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --keep-workspace)
      keep_workspace=1
      shift
      ;;
    --port)
      if [[ "$#" -lt 2 ]]; then
        echo "--port requires a value" >&2
        exit 2
      fi
      port="$2"
      shift 2
      ;;
    --no-browser)
      no_browser=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

repo_root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
maestro="${MAESTRO_BIN:-$repo_root/target/release/maestro}"

workspace=$(mktemp -d /tmp/maestro-complex-demo.XXXXXX)
cleanup() {
  echo
  echo "workspace kept for Web UI: $workspace"
}
trap cleanup EXIT

if [[ -z "${MAESTRO_BIN:-}" ]]; then
  echo "== building maestro =="
  (cd "$repo_root" && cargo build --release)
else
  echo "== using maestro binary =="
  echo "$maestro"
fi

echo
echo "== preparing temporary monorepo fixture =="
rsync -a "$repo_root/examples/monorepo-pnpm/" "$workspace/"
(
  cd "$workspace"
  git init -q
  git config user.email "maestro-demo@example.invalid"
  git config user.name "Maestro Demo"
  git add .
  git commit -qm "seed monorepo fixture"
)
echo "workspace: $workspace"

goal="upgrade the shared dependency contract across demo-core, demo-cli, and demo-mobile; update call sites and tests safely"

cd "$workspace"

echo
echo "== analyze workspace =="
"$maestro" init --analyze --root . --agent mock

echo
echo "== generate dependency-aware DAG =="
"$maestro" work "$goal" --root . --agent mock

plan=$(ls -t plans/*.yaml | head -1)

# The fixture's default synthesized checks run pnpm and may create node_modules
# churn in git worktrees. Keep this demo deterministic and focused on Maestro's
# orchestration behavior rather than package-manager side effects.
perl -0pi -e 's/      pnpm test\n/      echo verify ok\n/g' "$plan"
"$maestro" plan validate "$plan"

echo
echo "plan: $plan"
echo "---- PLAN excerpt ----"
sed -n '1,180p' "$plan"

echo
echo "== execute DAG with mock agent =="
set +e
"$maestro" run "$plan"
run_status=$?
set -e
echo "maestro run exit: $run_status"

run_dir=$(ls -td .maestro/runs/* | head -1)
echo
echo "run: $run_dir"
echo "---- RUN_STATE.json ----"
cat "$run_dir/RUN_STATE.json"

echo
echo "---- run files ----"
find "$run_dir" -maxdepth 2 -type f | sort | sed -n '1,120p'

echo
echo "---- REPORT excerpt ----"
if [[ -f "$run_dir/REPORT.md" ]]; then
  sed -n '1,220p' "$run_dir/REPORT.md"
else
  echo "REPORT.md not found; inspect run files above"
fi

echo
echo "== open Web UI =="
url="http://127.0.0.1:$port"
ui_log="$workspace/maestro-ui.log"
if command -v python3 >/dev/null 2>&1; then
  ui_pid=$(
    python3 - "$maestro" "127.0.0.1" "$port" "$ui_log" <<'PY'
import os
import subprocess
import sys

maestro, host, port, log_path = sys.argv[1:]
log = open(log_path, "ab", buffering=0)
proc = subprocess.Popen(
    [maestro, "ui", "--host", host, "--port", port],
    cwd=os.getcwd(),
    stdin=subprocess.DEVNULL,
    stdout=log,
    stderr=log,
    close_fds=True,
    start_new_session=True,
)
print(proc.pid)
PY
  )
else
  nohup "$maestro" ui --host 127.0.0.1 --port "$port" >"$ui_log" 2>&1 </dev/null &
  ui_pid=$!
  disown "$ui_pid" 2>/dev/null || true
fi

if [[ "${MAESTRO_DEMO_SKIP_UI_WAIT:-0}" != "1" ]]; then
  ready=0
  for _ in {1..50}; do
    if curl -fsS --max-time 1 "$url" >/dev/null 2>&1; then
      ready=1
      break
    fi
    if ! kill -0 "$ui_pid" 2>/dev/null; then
      echo "Web UI process exited before becoming ready; logs: $ui_log" >&2
      sed -n '1,120p' "$ui_log" >&2 || true
      exit 1
    fi
    sleep 0.1
  done
  if [[ "$ready" -ne 1 ]]; then
    echo "Web UI did not become ready at $url; logs: $ui_log" >&2
    exit 1
  fi
fi

echo "web UI: $url"
echo "web UI pid: $ui_pid"
echo "web UI logs: $ui_log"

if [[ "$no_browser" -ne 1 ]]; then
  if command -v open >/dev/null 2>&1; then
    open "$url" >/dev/null 2>&1 || true
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$url" >/dev/null 2>&1 || true
  fi
fi

echo
echo "== done =="
echo "workspace: $workspace"
echo "plan: $workspace/$plan"
echo "run: $workspace/$run_dir"
