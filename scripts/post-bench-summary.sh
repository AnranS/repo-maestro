#!/usr/bin/env bash
set -euo pipefail

result_file="${1:-bench-result.json}"

if [[ ! -s "$result_file" ]]; then
  echo "bench result missing or empty: $result_file" >&2
  exit 0
fi

if [[ -z "${GH_TOKEN:-}" ]]; then
  echo "GH_TOKEN not set; skipping bench tracker comment" >&2
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh not found; skipping bench tracker comment" >&2
  exit 0
fi

summary="$(python3 - "$result_file" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = json.loads(path.read_text())
summary = data.get("summary", {})
started = data.get("started_at", "unknown")
finished = data.get("finished_at", "unknown")

print(f"### Bench run")
print()
print(f"- started: `{started}`")
print(f"- finished: `{finished}`")
print(f"- total: `{summary.get('total', 0)}`")
print(f"- passed: `{summary.get('passed', 0)}`")
print(f"- failed: `{summary.get('failed', 0)}`")
print(f"- OSS replay pass rate: `{summary.get('oss_replay_pass_rate', 0):.2f}`")
print(f"- Contract-break pass rate: `{summary.get('contract_break_pass_rate', 0):.2f}`")
print()
print("| Fixture | Kind | Status |")
print("|---|---|---|")
for result in data.get("results", []):
    status = "PASS" if result.get("passed") else "FAIL"
    print(f"| `{result.get('fixture_id')}` | `{result.get('kind')}` | {status} |")
PY
)"

issue_number="$(
  gh issue list \
    --state open \
    --search "bench-tracker in:title" \
    --json number,title \
    --jq '.[] | select(.title == "bench-tracker") | .number' \
    | head -n 1
)"

if [[ -z "$issue_number" ]]; then
  issue_number="$(gh issue create \
    --title "bench-tracker" \
    --body "Weekly benchmark run summaries for maestro." \
    --json number \
    --jq '.number')"
fi

gh issue comment "$issue_number" --body "$summary" >/dev/null
echo "posted bench summary to issue #$issue_number"
