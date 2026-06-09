# F-136b2 — Safe Agent Runtime: run deadline + Unix rlimit (design-only)

Status: **design-only contract/scan — NO code until 大力 pins + GO.** Third F-136
implementation slice, after F-136b1 (env scrub + output cap, CLOSED). 大力's directive: add a
**run-level wall-clock deadline** (kills the run, leaves clear cancelled/deadline evidence) and
**Unix `setrlimit`** (NPROC / FSIZE / CPU; AS still deferred) on the agent child. Pins: **macOS is
best-effort**; **Linux cgroup / landlock / seccomp listed as a separate later slice**; **the
deadline MUST kill the process group(s) and record explicit cancelled/deadline evidence**;
**rlimit only under `cfg(unix)`, Windows / non-Unix a no-op that never panics**; **default still
off, generous budget.**

## 1. Principle (same honesty boundary as b1)
We enforce only what the OS backs at the process boundary we own. The **deadline** kills the
process *group* (SIGKILL the whole subtree, reusing `proc::kill_process_group`) — a hard control.
**`setrlimit`** is kernel-enforced on Unix, **best-effort on macOS** (some limits are per-UID, not
per-process), **a no-op on Windows**. The Linux-stronger isolation (cgroup v2 accounting, landlock
fs, seccomp syscall, net namespaces) is **NOT** in this slice — it is the separate F-136c+ tier; we
do not claim it. Both controls are **off by default**; an honest run under default config is
unchanged.

## 2. Seam map (verified scan)
| # | Seam | Where | State today |
|---|------|-------|-------------|
| 1 | run start time | `scheduler/state.rs` `RunState.started_at: DateTime<Utc>` (set once at run creation) | exists — the deadline baseline |
| 2 | run loop / task wait | `scheduler/executor.rs` `run_plan()` → dispatch loop → `tokio::select!` on `running.join_next()` with a **500 ms** poll window | the deadline check goes here, next to the cancel poll |
| 3 | cancel mechanism | `cli/commands/builtins.rs` `cmd_cancel_run()` writes a marker `.maestro/control/cancels/<run_id>`; `executor.rs::check_cancelled()` polls it at loop-top + in the 500 ms select | **reusable** — the deadline is "an internally-triggered cancel with a deadline reason" |
| 4 | terminal status | `scheduler/state.rs` `enum RunStatus { Running, Done, Failed, Cancelled }`; executor sets `Cancelled` + `ended_at`, `mark_unfinished_tasks_cancelled()`, emits `RunEventKind::{CancelRequested,RunCancelled,TaskCancelled}` with a `message` | **reusable** — deadline maps to `Cancelled` + a distinct reason |
| 5 | per-task child kill | `proc.rs` `isolate_process_group(cmd)` (`cmd.process_group(0)`, `cfg(unix)`) + `kill_process_group(pid)` (`libc::kill(-pid, SIGKILL)`, `cfg(unix)`); adapters call them on the per-task timeout | the deadline reuses `kill_process_group` — but needs the in-flight pids |
| 6 | **live-child registry** | **ABSENT** — process groups are per-task; the executor's `JoinSet` doesn't expose the children's pids/pgids | the deadline needs a way to enumerate the in-flight task children to kill their groups |
| 7 | agent spawn | `adapter/{cursor,codex}.rs` build a `tokio::process::Command`; `proc::isolate_process_group` + b1 `apply_env_scrub` mutate it before `.spawn()` | the rlimit hook goes here |
| 8 | **`pre_exec`** | `tokio::process::Command` exposes `process_group` but **NOT `pre_exec`**; `std::os::unix::process::CommandExt::pre_exec` exists on `std::process::Command` | the rlimit wiring needs care (§4.2) |
| 9 | libc | `Cargo.toml` `libc = "0.2"`; `proc.rs` already uses `libc::kill`/`SIGKILL` | ready for `setrlimit`/`RLIMIT_*` |
| 10 | no resource caps | `rg setrlimit\|rlimit\|cgroup\|seccomp\|landlock` → **0**; no run-level deadline anywhere | blank slate |
| 11 | config | `config/projects.rs` `Defaults.runtime_hardening: RuntimeHardening { scrub_env, extra_allow_env, max_task_log_bytes }` (b1) | add the b2 knobs here, default-off |

## 3. Control A — run-level wall-clock deadline (cross-platform)
### 3.1 Config & semantics
`RuntimeHardening.max_run_minutes: u32` (default `0` = no deadline = today's behavior). When `> 0`,
the run is killed once `Utc::now() - started_at` exceeds it.

### 3.2 Enforcement point
At the existing **500 ms poll** in the dispatch `select!` (seam #2), alongside `check_cancelled()`:
compute elapsed from `started_at`; on breach, take the **deadline path** (§3.3). Reusing the same
poll means no new timer task and no change to the run loop's shape.

### 3.3 Kill + evidence (the pin: kill process group + clear evidence)
On a deadline breach:
1. **Kill the process group(s)** of every in-flight task child via `proc::kill_process_group` (SIGKILL
   the whole subtree — not just `abort_all`, which only drops the direct child) — needs the live-child
   registry (§3.4 / Open #1).
2. `running.abort_all()` + drain (existing cancel teardown).
3. `mark_unfinished_tasks_cancelled()` (existing) — but with a **deadline** reason on the task error,
   distinct from a user cancel.
4. `RunStatus::Cancelled` + `ended_at` (existing terminal write).
5. **Distinct deadline evidence** so it's unambiguously a deadline, not a user cancel: a
   `RunEventKind::RunCancelled` carrying `message: "run deadline exceeded (N min)"` + a structured
   payload `{ reason: "deadline", max_run_minutes: N, elapsed_secs: … }`. (Open #4: a dedicated
   `RunEventKind::RunDeadline` / a `cancel_reason` field on `RunState` vs reusing `RunCancelled` +
   message.)

### 3.4 Live-child registry (the missing seam #6)
The deadline (and, as a bonus, the existing cancel) needs to enumerate in-flight task children to
kill their groups. Proposal: a shared `Arc<Mutex<HashSet<u32>>>` (or a small `LiveChildren` type) the
executor owns; each task, right after it spawns its agent child, registers the child pid (= pgid
leader after `isolate_process_group`) and deregisters on completion. The deadline iterates the set
and `kill_process_group`s each. (Open #1: this registry vs. simply writing the cancel marker and
letting the existing path tear down — but the existing path does NOT kill groups, so a pure
marker-reuse would not satisfy the "kill process group" pin; the registry is the honest way.)

## 4. Control B — Unix `setrlimit` on the agent child (Unix-hard, Windows no-op)
### 4.1 Limits & config (AS deferred)
| knob (config) | rlimit | guards |
|---------------|--------|--------|
| `rlimit_nproc: u32` (0=off) | `RLIMIT_NPROC` | fork-bomb / runaway subprocess count |
| `rlimit_fsize_mb: u64` (0=off) | `RLIMIT_FSIZE` | a single oversized file write (disk fill) |
| `rlimit_cpu_secs: u32` (0=off) | `RLIMIT_CPU` | CPU-seconds backstop (complements the wall-clock deadline) |
| *(deferred)* | `RLIMIT_AS` | **NOT in b2** — too easy to OOM-kill a legit agent |
All default `0` (off); set both soft & hard to the configured value.

### 4.2 Wiring (`pre_exec`) — the one real mechanism question
`tokio::process::Command` has no `pre_exec`. Options:
- **(recommended)** build the agent command as a **`std::process::Command`**, set
  `unsafe { pre_exec(|| { setrlimit(...); Ok(()) }) }` + `process_group(0)` + the b1 env scrub on it,
  then convert to tokio via `tokio::process::Command::from(std_cmd)` for the async stream/wait. The
  `pre_exec` closure runs in the forked child before exec, calling `libc::setrlimit` for each enabled
  limit (must be async-signal-safe: only `libc::setrlimit`, no allocation — pass the values in by copy).
- *(alt)* post-spawn `setrlimit` on the child pid → **rejected**: racy (the child may already have
  forked grandchildren) and `setrlimit` targets the calling process, not another pid.
This moves the spawn setup (env scrub + process group + rlimit) onto one `std::process::Command`
that's then handed to tokio — a contained refactor of the two adapters' spawn prologue. (Open #2.)

### 4.3 Platform
A shared `proc::apply_rlimits(builder, limits)` under `#[cfg(unix)]` installs the `pre_exec`; the
non-Unix path is a no-op (consistent with `isolate_process_group`). **macOS best-effort**, documented:
`RLIMIT_NPROC` is per-UID on macOS (weaker than per-process), `RLIMIT_FSIZE`/`RLIMIT_CPU` work. We do
not claim Linux-equivalent guarantees on macOS.

## 5. Explicitly NOT in this slice (Do-not)
- **NO cgroup / landlock / seccomp / namespaces / gVisor / Firecracker** — the Linux-stronger tier is
  **F-136c+ (separate slice)**; we don't pretend macOS rlimit equals it.
- **NO `RLIMIT_AS`** (OOM risk) — deferred.
- **NO network enforcement** (still provider-`--sandbox` + advisory).
- **NO change to F-126 gate, the Web settings page, or the b1 env/output controls.**
- **NO new run-end terminal state** — the deadline reuses `RunStatus::Cancelled` (just a distinct
  reason/evidence), so existing consumers of the run status are unaffected.
- **NO behavior change under default config** — both controls default off (`0`), generous when set.

## 6. Risk table (macOS vs Linux)
| Control | Too tight → | Too loose → | Platform caveat | Mitigation |
|---------|-------------|-------------|-----------------|------------|
| deadline | kills a legit long run mid-flight | a runaway burns wall-clock/cost | none (tokio timer is cross-platform) | default off, generous; clear `cancelled(deadline)` evidence so it's diagnosable; kill the group so nothing is orphaned |
| live-child registry | — | a missed child survives the kill | none | register right after spawn, deregister on completion; the deadline also `abort_all`s as a backstop |
| `RLIMIT_NPROC` | breaks legit parallel tools | fork bomb survives | macOS per-UID (weaker) | generous default; **document macOS best-effort** |
| `RLIMIT_FSIZE` | truncates a legit large artifact (SIGXFSZ) | disk fill | works both | size from config; generous |
| `RLIMIT_CPU` | SIGKILLs CPU-heavy legit work | runaway CPU | works both | high default; complements the wall-clock deadline |
| `pre_exec` wiring | a setup bug breaks all spawns | — | Unix-only | `cfg(unix)`; async-signal-safe closure (only `setrlimit`); covered by the spawn tests + default-off |

## 7. Test matrix
- **deadline (real child)**: a fake agent that sleeps past a tiny `max_run_minutes` → run ends
  `Cancelled` with the **deadline** reason/evidence, `ended_at` set, and the child's **process group is
  gone** (assert the pid/pgid is no longer alive). Proves kill-group + evidence.
- **deadline default off**: `max_run_minutes=0` → a normal run completes `Done`, no premature kill.
- **rlimit `#[cfg(unix)]` (real child)**: `RLIMIT_NPROC` → a fork-storm child is throttled/killed;
  `RLIMIT_FSIZE` → an oversized write fails (SIGXFSZ / write error). Gated so non-Unix CI skips.
- **rlimit default off**: all `0` → an honest task runs unchanged.
- **Windows / non-Unix**: `apply_rlimits` no-op, spawn still works, nothing panics.
- **evidence distinctness**: a user `cancel-run` vs a deadline breach produce distinguishable
  reasons/events (so a dashboard/PM can tell "I cancelled" from "it timed out").
- **no-behavior-change**: an honest run under default config → identical RunState/events vs un-hardened.

## 8. Open decisions (for 大力 to pin)
1. **Deadline kill mechanism**: a **live-child pgid registry** the deadline walks + `kill_process_group`
   (recommended — honestly kills the group, satisfies the pin) vs reuse the cancel marker only (does
   NOT kill groups → fails the pin). *(lean: registry.)* Sub-question: should the **existing** cancel
   path also adopt the registry to kill groups (fixing a latent gap), or keep that out of b2?
2. **rlimit `pre_exec` wiring**: build a `std::process::Command` (pre_exec + process_group + env scrub)
   → `into()` tokio (recommended) vs keep tokio Command + a different hook. *(lean: std→tokio; moves
   the b1 env scrub onto the std builder too — a small, contained adapter-prologue refactor.)*
3. **rlimit set in b2**: `NPROC + FSIZE + CPU`, `AS` deferred (recommended) vs include `AS`. *(lean:
   no AS.)*
4. **Deadline evidence shape**: reuse `RunCancelled` + `message` + `reason:"deadline"` payload
   (recommended, no new enum) vs a dedicated `RunEventKind::RunDeadline` / a `cancel_reason` field on
   `RunState`. *(lean: reuse + reason payload; a typed field is nicer for the Web/PM surface but is more
   schema churn — could be a fast-follow.)*
5. **Config home & units**: `max_run_minutes: u32` + `rlimit_{nproc,fsize_mb,cpu_secs}` on
   `RuntimeHardening`, all `0`=off (recommended) vs a nested `rlimits` sub-struct. *(lean: flat knobs,
   like b1.)*
6. **Profile coupling**: keep b2 config-gated only (like b1), RuntimeProfile pure-mapping deferred
   (recommended) vs derive default limits from the a1 profile now. *(lean: config-gated; profile
   tiers later.)*
7. **Slice granularity**: one b2 commit (deadline + rlimit) vs split **b2a deadline** (cross-platform,
   reuses the cancel machinery — lower risk) + **b2b rlimit** (touches the spawn prologue + `pre_exec`
   — the riskier half). *(lean: offer the split; b2a is clean and independently shippable.)*

## 9. Review axes: principle/honesty → §1; seam map → §2; deadline → §3; rlimit → §4; Do-not → §5;
risk + macOS/Linux → §6; tests → §7; recommended + open → §8. No code until GO.
