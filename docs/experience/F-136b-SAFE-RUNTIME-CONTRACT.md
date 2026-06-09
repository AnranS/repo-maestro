# F-136b — Safe Agent Runtime, slice 1: cross-platform process-boundary hardening (design-only)

Status: **design-only contract/scan — NO code until 大力 GO.** Second F-136 implementation
slice (after F-136a1 RuntimeProfile derivation + F-136a2 settings IA). 大力's directive: harden
the **executor/adapter process boundary** along four axes — **env / timeout / output-truncation /
process-subprocess** — based on the F-136a1 `RuntimeProfile`; do the **cross-platform, low-risk**
hardening first; list Linux-only kernel isolation (cgroup / container / gVisor / Firecracker) as a
**separate later slice**; and **never disguise the opaque provider's internal tool interception as
hard enforcement.**

## 1. The one principle (honesty boundary)
maestro spawns an **untrusted code executor** (the provider CLI: codex / cursor / claude / mock)
as a child process. We can only enforce at the **boundary we own** — the `Command` we build and
the OS limits we set on that child. We do **NOT** control what the agent does *inside* its own
sandbox. So F-136b draws a hard line:

| Layer | Who enforces | Honesty label |
|------|------|------|
| Env we pass / withhold | **us, OS-hard** (child literally can't read a var we don't pass) | **hard** |
| Wall-clock kill of the process group | **us, OS-hard** (SIGKILL) | **hard** |
| Captured-output byte cap | **us** (we stop appending) | **hard** |
| `setrlimit` on the child (Unix) | **us, kernel-hard** | **hard (Unix); no-op Windows** |
| Provider's own `--sandbox` flag (codex/cursor) | **provider, opaque** | **advisory — surfaced, never claimed as ours** |
| Network egress / fs-mount confinement | nobody today | **absent — Linux-only later (F-136c+)** |

The RuntimeProfile (F-136a1) **describes** the requested capability; F-136b makes the boundary
controls **follow** that label — but the label stays advisory where the OS can't back it.

## 2. Seam map (current — verified scan)
| # | Seam | Where | State today | Gap |
|---|------|-------|-------------|-----|
| 1 | spawn (agent) | `adapter/{shell,cursor,codex}.rs` `.run()`; `scheduler/verify.rs` (acceptance `bash -lc`); `chat/{tagger,compact}.rs`; `chat/actions.rs` (self-exec) | `Command::new()`, stdout/stderr piped→log | no resource caps |
| 2 | **env** | every spawn | **parent env inherited WHOLESALE** — `rg '\.env_clear'` → **0 hits**; only `MAESTRO_SESSION_ID`/`RUN_LAUNCHED` ever set; provider binary names from `MAESTRO_{CODEX,CURSOR_AGENT,CLAUDE}` | **child sees every secret in maestro's env** |
| 3 | **timeout** | `adapter/*` `tokio::time::timeout(task.timeout, …)`; `verify.rs` 600s; `runtime_health` 2s; tagger 60s / compact 90s | **per-task** wall-clock, kills process group on breach (Unix) | **no run-level (total) wall-clock cap**; `BLOCKED_MAIL_TIMEOUT`=30min is wait-detection, not a cap |
| 4 | **output** | adapters append stdout→task log file | **unbounded** (only `verify.rs::truncate_output` keeps last 4 KiB; `capture_task_outputs` honors `spec.max_bytes`) | **task logs grow without a byte cap → disk-fill** |
| 5 | process group | `src/proc.rs` `isolate_process_group` / `kill_process_group` | `cmd.process_group(0)` + `libc::kill` — **`#[cfg(unix)]`, Windows no-op** | no child-count / file-size / cpu cap |
| 6 | cwd / worktree | `executor.rs::resolve_task_workspace`; adapters set `current_dir` / `--workspace` / `--cd` | git-worktree or project dir (`gitops::WorktreeGuard`) | fs-branch isolation only, **no mount/chroot** |
| 7 | RuntimeProfile | `schema/runtime_profile.rs::derive(permission, observed_write)` + `summarize` | **advisory, display-only** (deliveries UI); NOT consulted at spawn | not enforced |
| 8 | policy gate | `scheduler/policy_gate.rs::evaluate` (F-126) | **POST-RUN** audit; `observed_write = files_changed‖pr_url`; optional approval gate | **post-hoc — can't prevent a mid-run effect** |
| 9 | platform | `proc.rs` `cfg(unix)`; no `setrlimit`/`rlimit`/`cgroup`/`landlock`/`seccomp`/`pre_exec` anywhere (`rg` → **0**) | only process-group on Unix | all kernel limits unused |

## 3. The four pillars (cross-platform, low-risk — slice 1)
Each pillar is **opt-in / default-generous**, **honest about its layer**, and **profile-aware**.

### 3.1 Env hygiene — *hard, cross-platform*
Build the child env from an **allowlist** instead of wholesale inherit:
- **Always keep**: `PATH`, `HOME`, `USER`, `LANG`/`LC_*`, `TERM`, `TMPDIR`, the `MAESTRO_*` maestro
  passes, and the **provider's own credential env** (the one key the chosen provider needs).
- **Drop**: everything else, especially `*_TOKEN` / `*_SECRET` / `*_KEY` / `*_PASSWORD` /
  `AWS_*` / `GH_TOKEN` … that aren't the active provider's.
- Implemented as a pure `scrub_env(parent, profile, provider) -> Vec<(K,V)>` → `cmd.env_clear();
  cmd.envs(kept)`. Pure ⇒ unit-testable; no spawn needed.
- **Profile-aware**: `ReviewOnly` strips the most (no write/network creds needed); `WriteLocal` /
  `NetworkAllowlisted` keep the provider key. `RequiresReview`/`Unknown` → strictest.

### 3.2 Run-level wall-clock cap — *hard, cross-platform*
Add a **total-run deadline** (sum across tasks) alongside the existing per-task timeout. On breach,
kill the active process group(s) and mark the run `cancelled (deadline)`. tokio-timer based ⇒
cross-platform; generous default (e.g. config `max_run_minutes`, off unless set).

### 3.3 Output cap — *hard, cross-platform*
Give the adapter log-append path a **byte budget** (e.g. `max_task_log_bytes`, default generous like
2–8 MiB). On breach: stop appending, write a `…output truncated (cap N MiB)…` marker, keep the run
going. Mirrors `verify.rs`'s existing truncation but for the streaming adapters. Byte-counting ⇒
cross-platform; **honesty**: the truncation is always marked, never silent.

### 3.4 Process / subprocess boundary — *hard on Unix (macOS+Linux), no-op Windows*
Extend `proc.rs` with a `harden(cmd, limits)` that installs **`setrlimit` via `pre_exec`** (Unix):
- `RLIMIT_NPROC` — cap child+grandchild process count (fork-bomb / runaway-subprocess guard).
- `RLIMIT_FSIZE` — cap any single file the child writes (disk-fill guard).
- `RLIMIT_CPU` — CPU-seconds backstop (complements wall-clock).
- *(`RLIMIT_AS` address-space cap **deferred** — agents legitimately need lots of memory; too risky
  for slice 1 — see Open #3.)*
- All under `#[cfg(unix)]`; Windows path is a no-op (consistent with existing `proc.rs`).
- `setrlimit` is **POSIX → works on macOS AND Linux** — this is the cross-platform sweet spot. The
  hierarchical/elastic limits (cgroup v2) are **Linux-only → deferred** (§5).

## 4. RuntimeProfile → limits (pure mapping)
A pure `fn limits_for(profile: RuntimeProfile, cfg: &HardeningConfig) -> Hardening` (mirrors a1's
`derive` shape, fully testable, Rust↔nothing-to-mirror since it's runtime-only):
| Profile | env | rlimit | network | notes |
|---------|-----|--------|---------|-------|
| `ReviewOnly` | strip write/net creds | tightest NPROC/FSIZE | n/a | read-only intent |
| `WriteLocal` | keep provider key | moderate FSIZE | n/a | fs allowed |
| `NetworkAllowlisted` | keep provider key | moderate | **advisory only** (no kernel net enforce in slice 1) | honest: surfaced, not enforced |
| `ToolLimited` / `HighRiskVm` | (future-only in a1) | conservative | — | unreachable in v1 data — keep test-locked |
| `RequiresReview` / `Unknown` | strictest | tightest | — | conservative-up, honesty-first (same as a1) |

## 5. Explicitly NOT in this slice (boundaries / Do-not)
- **NO network kernel enforcement** — cross-platform egress control needs Linux namespaces / `pf` /
  per-provider proxy. Network stays the provider `--sandbox`'s job + advisory in the profile. We do
  not claim to block it.
- **NO cgroup / landlock / seccomp / namespaces / gVisor / Firecracker / sandbox-exec** — Linux-only
  (or deprecated-macOS) kernel isolation → **F-136c+ (separate slice)**.
- **NO chroot / mount confinement** — worktree stays git-branch fs isolation.
- **NO interception of the opaque provider's internal tools** — we never wrap/inspect what codex or
  cursor do inside their own sandbox and call it enforcement. The provider sandbox is **advisory**,
  surfaced via the a1 profile, never green-"hard".
- **NO behavior change for honest tasks** — caps generous, env allowlist covers provider needs;
  default posture is conservative (Open #2).
- **NO Rust↔TS schema churn** — this is runtime spawn hardening; the only surfaced state is honest
  "what was enforced" on the task/run (additive), no new web mutation.

## 6. Risk table
| Pillar | Too strict → | Too loose → | Platform caveat | Mitigation |
|--------|--------------|-------------|-----------------|------------|
| env scrub | agent loses a needed cred → fails | secret leaks to child | none (env is cross-platform) | allowlist + provider-key kept + configurable extra-allow + warn-first option |
| wall-clock | kills a legit long run | runaway burns time/cost | none | generous default, opt-in, marked `cancelled(deadline)` |
| output cap | loses evidence tail | disk fill | none | generous cap, **always mark truncated**, keep last-N tail |
| `RLIMIT_NPROC` | breaks legit parallel tools | fork bomb survives | macOS counts per-UID not per-process → weaker | generous count; **document macOS is best-effort** |
| `RLIMIT_FSIZE` | truncates a legit large artifact | disk fill | works both | size from profile; generous |
| `RLIMIT_CPU` | kills CPU-heavy legit work | runaway CPU | works both | high default; complements wall-clock |
| `RLIMIT_AS` (deferred) | OOM-kills the agent | — | — | **left out of v1** |
| honesty | — | a control looks "hard" but isn't | macOS rlimit weaker than Linux cgroup | label layer per §1; profile says advisory where true |

**macOS vs Linux summary**: `setrlimit` (NPROC/FSIZE/CPU) + process-group kill + env + wall-clock +
output cap all work on **both** (best-effort on macOS, where some rlimits are per-UID and softer).
**Linux-stronger** kernel isolation (cgroup v2 accounting, landlock fs, seccomp syscall, net
namespaces) is **deferred** to a Linux-only slice. Windows: rlimit no-op, env/wall-clock/output cap
still apply.

## 7. Test matrix
- **env (pure)**: secret `FOO_TOKEN` stripped; provider key kept; `PATH`/`HOME` kept; `ReviewOnly`
  strips more than `WriteLocal`; unknown provider → strictest. (no spawn — pure builder)
- **wall-clock**: run over `max_run_minutes` → process group killed + run marked `cancelled(deadline)`;
  under → unaffected. (mock adapter, fast fake clock or tiny limit)
- **output cap**: task output over cap → log truncated + marker + tail kept; under → full. (mock)
- **rlimit (`#[cfg(unix)]`)**: `RLIMIT_NPROC` blocks a fork-storm; `RLIMIT_FSIZE` blocks an oversized
  write; both gated so they no-op on non-Unix CI. (integration, small limits)
- **profile→limits (pure)**: each `RuntimeProfile` → expected `Hardening` set; `RequiresReview`/
  `Unknown` → strictest; future tiers test-locked-unreachable (mirror a1's pattern).
- **honesty**: a `NetworkAllowlisted` task surfaces network as **advisory**, never "enforced".
- **no-behavior-change**: an honest task under all caps → byte-identical output vs un-hardened.
- **Windows/non-Unix**: harden() no-op for rlimit; env/wall-clock/output still apply; nothing panics.

## 8. Open decisions (for 大力 to pin)
1. **Env model**: allowlist (strict, safe — *recommended*) vs denylist (permissive, leak-prone).
   *(lean: allowlist + a `extra_allow_env` config escape hatch.)*
2. **Default posture for slice 1**: **enforce** vs **warn-only/observe** (advisory; record "what
   WOULD be enforced" on the task, default-off). *(lean: ship the controls **off-by-default /
   opt-in**, like F-126's gate flag — honesty + zero-surprise; the profile already surfaces intent.)*
3. **rlimit set in v1**: `NPROC`+`FSIZE`+`CPU` yes, `AS` deferred (*recommended*) vs include `AS`.
   *(lean: skip `AS` — too easy to OOM-kill a legit agent.)*
4. **Config home**: `projects.yaml` defaults (`max_run_minutes`, `max_task_log_bytes`, rlimit knobs)
   + optional per-delivery override vs global only. *(lean: projects.yaml defaults, generous, off.)*
5. **Profile→limits coupling**: a **pure fn** now (like a1 `derive`, *recommended*) vs a config table.
   *(lean: pure fn; expose knobs via config later.)*
6. **macOS rlimit caveat**: **document + accept** best-effort macOS (*recommended*) vs block the slice
   on Linux-equivalence. *(lean: document; Linux-strong isolation is the later slice.)*
7. **Hook location**: a shared `proc::harden(cmd, hardening)` applied by **all** adapters
   (*recommended*) vs per-adapter. *(lean: one shared helper — single defn, like a1's shared
   `observed_write`.)*
8. **Slice granularity**: do all 4 pillars in F-136b, or split (e.g. F-136b1 env+output [purely
   cross-platform], F-136b2 wall-clock+rlimit)? *(lean: if 大力 wants smaller cuts — b1 = env scrub +
   output cap [no spawn-path risk], b2 = wall-clock + rlimit [touches the spawn/kill path].)*

## 9. Review axes: principle/honesty → §1; seam map → §2; four pillars → §3; profile mapping → §4;
Do-not/boundaries → §5; risk + macOS/Linux → §6; tests → §7; recommended + open → §8. No code until GO.
