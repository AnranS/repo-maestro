#!/usr/bin/env bash
# Build a maestro hero asciicast.
#
# Usage:
#   ./scripts/record-demo.sh
#
# Produces:
#   assets/demo.cast            asciicast v2 (uploadable to asciinema.org)
#   assets/demo.gif             rendered gif (for README embed)
#
# Requirements:
#   - maestro on $PATH (or built in target/release/)
#   - python3
#   - agg (asciinema gif renderer)
#
# Notes:
#   We synthesize the .cast file in python (it's just JSON lines) instead
#   of using `asciinema record`, because the latter requires a controlling
#   TTY and we want the recording to be reproducible in CI / headless
#   shells.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
ASSETS="$REPO/assets"
MAESTRO="$REPO/target/release/maestro"
CAST="$ASSETS/demo.cast"
GIF="$ASSETS/demo.gif"
SCRATCH="$(mktemp -d -t maestro-demo-scratch.XXXXXX)"

mkdir -p "$ASSETS"

if [[ ! -x "$MAESTRO" ]]; then
  echo "[record-demo] building maestro..." >&2
  (cd "$REPO" && cargo build --release --quiet)
fi

echo "[record-demo] building $CAST in $SCRATCH" >&2

MAESTRO="$MAESTRO" SCRATCH="$SCRATCH" CAST="$CAST" python3 - <<'PY'
import json
import os
import shutil
import subprocess
import time

maestro = os.environ["MAESTRO"]
scratch = os.environ["SCRATCH"]
cast_path = os.environ["CAST"]

WIDTH = 92
HEIGHT = 28
PROMPT = "\x1b[36m$\x1b[0m "

frames = []          # (delta_seconds, output_bytes)
clock = 0.0

def at(delta, text):
    global clock
    frames.append((delta, text))
    clock += delta

def type_cmd(cmd, per_char=0.04, pre=0.20):
    at(pre, PROMPT)
    for ch in cmd:
        at(per_char, ch)
    at(0.10, "\r\n")

def emit(text, pre=0.05):
    at(pre, text)

def run(cmd, cwd=None, env_extra=None):
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    result = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return result.stdout.decode(errors="replace")

# 1. banner
at(0.6, "\x1b[1;36m# maestro — multi-project agent orchestrator\x1b[0m\r\n")

# 2. version
type_cmd(f"maestro --version")
ver = run([maestro, "--version"]).strip()
emit(ver + "\r\n", pre=0.15)

# 3. set up scratch workspace
type_cmd("cd $(mktemp -d)")
emit("", pre=0.05)

# 4. demo --run (zero-config shell DAG, no creds)
type_cmd("# zero-config: shell agent, no LLM credentials required")
type_cmd("maestro demo --run")
demo_out = run([maestro, "demo", "--run"], cwd=scratch)
# normalize CRLF for terminal
demo_out_term = demo_out.replace("\n", "\r\n")
emit(demo_out_term, pre=0.4)

# 5. peek at the report
report = os.path.join(scratch, "maestro-demo", ".maestro", "runs", "current", "REPORT.md")
type_cmd("head -16 maestro-demo/.maestro/runs/current/REPORT.md")
if os.path.exists(report):
    with open(report) as fh:
        lines = fh.readlines()[:16]
    emit("".join(lines).replace("\n", "\r\n"), pre=0.4)
else:
    emit("(no report)\r\n")

# 6. wrap
at(1.2, "\x1b[1;36m# DAG ran, run state + report on disk. open dashboard: \x1b[0;36mmaestro open\x1b[0m\r\n")
at(1.4, PROMPT)

# write cast
header = {
    "version": 2,
    "width": WIDTH,
    "height": HEIGHT,
    "timestamp": int(time.time()),
    "env": {"SHELL": "/bin/bash", "TERM": "xterm-256color"},
    "title": "maestro — demo",
}
with open(cast_path, "w") as fh:
    fh.write(json.dumps(header) + "\n")
    t = 0.0
    for delta, text in frames:
        t += delta
        fh.write(json.dumps([round(t, 3), "o", text]) + "\n")

print(f"[record-demo] wrote {cast_path} ({len(frames)} frames, {t:.1f}s)")
PY

echo "[record-demo] rendering $GIF" >&2
agg --theme monokai --speed 1.3 --font-size 14 --cols 92 --rows 28 "$CAST" "$GIF"

ls -lh "$CAST" "$GIF"
echo "[record-demo] done. Embed in README via assets/demo.gif"
