# Tier 1 Cold-Start Experience Cases

Cycle 1 validates the first 10 minutes of a user who already has Rust and can
install the local `maestro` binary. The "no Rust installed" path is explicitly
out of scope for this cycle.

## How To Record

Use the transcript runner for every command that represents a user-visible
step:

```bash
scripts/run-experience-case.sh T1-C1 -- maestro setup
```

To record the whole Tier 1 cycle in one pass, use the batch wrapper:

```bash
scripts/run-t1-cycle.sh
```

It generates one run id, runs T1-C1 through T1-C5, writes transcript folders,
and creates `docs/cases/T1-runs/<run-id>/metrics.md`.

Before starting a full cycle, make the binary freshness explicit. Prefer the
installed `maestro` on PATH; if you are dogfooding `target/release/maestro`,
rebuild it first and record the exact binary path:

```bash
cargo build --release
which maestro
maestro --version
```

Set one shared run id when collecting a full session:

```bash
export EXPERIENCE_RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
```

Each case writes to `docs/cases/T1-runs/<run-id>/<case-id>/`:

- `command.txt`
- `stdout.log`
- `stderr.log`
- `exit-code.txt`
- `duration-ms.txt`
- `summary.md`

At the run root, keep a manual `metrics.md` using the template below. The
runner intentionally records raw command facts only; the metrics capture the
human experience of the whole case.

```markdown
| case | time_to_first_report_s | commands_count | manual_interventions | error_message_quality | artifact_discoverability | recovery_success | notes |
|---|---:|---:|---:|---:|---:|---|---|
| T1-C1 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C2 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C3 |  |  |  |  |  | yes/no/n/a |  |
| T1-C4 | n/a |  |  |  |  | yes/no/n/a |  |
| T1-C5 | n/a |  |  |  |  | yes/no/n/a |  |
```

Scale fields:

- `error_message_quality`: 1 means opaque/internal; 5 means clear and
  copy-pasteable.
- `artifact_discoverability`: 1 means artifacts are effectively hidden; 5
  means the next artifact path is obvious from output.
- `recovery_success`: whether the user can recover using only command output.

After running, copy findings into `docs/cases/T1-friction-log.md`.

## Case Format

Each case is evaluated against:

- setup: the starting state and any environment overrides.
- command: the exact command a user runs.
- expected: the product behavior we want.
- capture: artifacts to save in the transcript folder.
- pass signal: the minimum evidence that the case is usable.

## T1-C1: Rust Present, No Provider Ready

### Setup

- Rust and `cargo` are installed.
- `maestro` is installed from this repo.
- Provider CLIs such as `codex`, `cursor-agent`, and `claude` may be missing.
- API keys may be unset.

### Command

```bash
maestro setup
```

### Expected

The setup flow should complete local workspace initialization and tell the user
which provider command is missing or unhealthy, with one copy-pasteable next
step per provider. Missing optional providers must not hide the next useful
local command.

### Capture

- Full stdout/stderr.
- Whether `.maestro/` was created.
- The exact provider remediation text.

### Pass Signal

A first-time user can answer "what should I install or run next?" without
reading source code or opening the README.

## T1-C2: Provider Present, Credentials Missing

### Setup

- At least one provider CLI is present.
- Its credentials/API key/session are missing or expired.

### Command

```bash
maestro setup
maestro providers
maestro doctor
```

### Expected

The provider should be marked unavailable or degraded with a clear reason. The
message should explain whether the user can fall back to `shell`/`mock` for a
local demo, and how to authenticate the provider for real agent runs.

### Capture

- Provider table output.
- Doctor output.
- Any command that hangs or prompts unexpectedly.

### Pass Signal

The user can still run a local demo or understands the exact authentication
step needed before using an agent.

## T1-C3: Demo Run To Real Workspace Handoff

### Setup

- `maestro setup` has completed.
- No real project has been registered yet.

### Command

```bash
maestro demo --run
maestro init --analyze --root examples --agent mock
```

### Expected

The demo produces a report and points to the next command for a real workspace.
The `init --analyze` handoff should be discoverable and should not require the
user to infer hidden workspace state.

### Capture

- Demo report path.
- Any next-step text printed after the demo.
- The first `init --analyze --root examples` failure or success message.

### Pass Signal

The user can move from demo output to a real scan without guessing which command
comes next.

## T1-C4: Empty Workspace `maestro work`

### Setup

- Empty temporary directory.
- No `.maestro/projects.yaml` yet.

### Command

```bash
maestro work "fake goal"
```

### Expected

If the command initializes `.maestro/` and then fails because no projects are
registered, the error should include the next useful command, such as
`maestro work --root <path>` or `maestro init --analyze --root <path>`.

### Capture

- Whether `.maestro/` was created before failure.
- Error message and exit code.
- Any cleanup or rollback guidance.

### Pass Signal

The failure is actionable in one screen of output.

## T1-C5: Bench Offline Cold Cache

### Setup

- Fresh workspace with no `.maestro/bench/cache` entries for OSS replay
  fixtures.
- Network may be disabled or intentionally unused.

### Command

```bash
maestro bench all --json --offline
```

### Expected

The command may exit non-zero when OSS replay fixtures cannot be materialized,
but it must still emit valid JSON with per-fixture results. The summary should
make the cold-cache condition obvious so users do not confuse it with a product
regression.

### Capture

- Exit code.
- Full JSON output.
- Count of passed/failed fixtures.
- Any stderr explaining cache hydration.

### Pass Signal

Automation can parse the JSON even when the process exits non-zero, and a human
can tell whether the failure is cache/setup related.

## Cycle 1 Exit Criteria

Cycle 1 is complete when:

- all five cases have transcript folders for the same run id.
- `docs/cases/T1-runs/<run-id>/metrics.md` has one row per case.
- every friction is recorded with severity and friction score.
- P0 items have an owner or an explicit defer decision.
- top P1 items are ready for small fix commits of 300 LoC or less.
