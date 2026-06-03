//! MCP tool catalog + dispatcher.
//!
//! Each tool returns its result as MCP's `content` array — a single text
//! block of pretty-printed JSON. Clients (Cursor, Claude Desktop, etc.)
//! display this directly in the chat, and most agents are happy to read
//! JSON-as-text. Tools that return large payloads truncate to keep the
//! context window manageable.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::chat::actions::{build_argv, Action, ActionVerb};
use crate::config::ProjectsConfig;
use crate::memory::retrieval;
use crate::paths;
use crate::roles;
use crate::scheduler::state::RunState;

const MAX_RUNS_LISTED: usize = 30;
const MAX_RESULT_BYTES: usize = 16_000;

/// Tool descriptor list, exposed via the `tools/list` MCP method.
pub fn list_descriptors() -> Value {
    json!({
        "tools": [
            {
                "name": "maestro_state",
                "description": "Get the current run state (RUN_STATE.json) — what's running, what's blocked on approval, what acceptance checks passed. Returns an empty object if no run has happened yet.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "maestro_runs_list",
                "description": "List recent maestro runs in this workspace, newest first. Returns id, spec, status, started_at, and verified flag for each.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max runs to return (default 10, max 30).", "minimum": 1, "maximum": 30 }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_run_get",
                "description": "Read the full RUN_STATE.json of one specific run.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "run_id": { "type": "string", "description": "Run identifier from maestro_runs_list, or \"current\"." }
                    },
                    "required": ["run_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_memory_search",
                "description": "TF-IDF search over .maestro/memory/{l1_facts,l2_decisions} and recent REPLAN.md files. Use this to surface prior decisions, captured facts, or past acceptance failures relevant to a task.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural-language query. Tokenized, lowercased, stopword-filtered." },
                        "k":     { "type": "integer", "description": "Top-K hits (default 5).", "minimum": 1, "maximum": 20 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_roles_list",
                "description": "List every role visible to this workspace (builtin + user + Cursor plugin personas). Returns name, display, summary, tags, source.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "maestro_role_show",
                "description": "Return one role's prelude markdown verbatim (system prompt the agent adopts).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Role id, e.g. backend_rust or ce-kieran-typescript-reviewer." }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_projects_list",
                "description": "List all projects registered under .maestro/projects.yaml with their type, role, stack, and contract paths.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "maestro_mailbox_list",
                "description": "List local mailbox messages used for cross-role and cross-project handoffs. Defaults to open messages only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "all": { "type": "boolean", "description": "Include resolved messages." },
                        "to": { "type": "string", "description": "Filter by recipient role or project." },
                        "project": { "type": "string", "description": "Filter by project name." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_mailbox_send",
                "description": "Send a local mailbox message to another role, project, or task owner.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string" },
                        "to": { "type": "string" },
                        "project": { "type": "string" },
                        "task": { "type": "string" },
                        "subject": { "type": "string" },
                        "body": { "type": "string" },
                        "blocking": { "type": "boolean", "description": "If true, the recipient must resolve this before proceeding with its task (highlighted in the delivered prompt and UI). Defaults to false." }
                    },
                    "required": ["from", "to", "subject", "body"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_mailbox_ask",
                "description": "Ask the human a structured multiple-choice question and BLOCK until they answer (it renders as answerable buttons in the Web UI). Use only for a real decision a human should make. A `default` is REQUIRED and is returned automatically if no one answers before `timeout_secs`, so an unattended run never hangs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "subject": { "type": "string", "description": "Short title for the question." },
                        "body": { "type": "string", "description": "Context the human needs to decide." },
                        "prompt": { "type": "string", "description": "The question shown above the options (defaults to subject)." },
                        "options": {
                            "type": "array",
                            "description": "2+ choices.",
                            "items": {
                                "type": "object",
                                "properties": { "key": { "type": "string" }, "label": { "type": "string" } },
                                "required": ["key", "label"]
                            }
                        },
                        "multi_select": { "type": "boolean", "description": "Allow choosing several. Defaults to false." },
                        "default": { "type": "array", "items": { "type": "string" }, "description": "Option key(s) used if no human answers before timeout. Required." },
                        "timeout_secs": { "type": "number", "description": "Max seconds to wait before falling back to default. Defaults to 300." },
                        "from": { "type": "string" },
                        "project": { "type": "string" },
                        "task": { "type": "string" }
                    },
                    "required": ["subject", "options", "default"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_mailbox_resolve",
                "description": "Mark a mailbox message resolved by its full id.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "note": { "type": "string" }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_work",
                "description": "Scope a multi-repo change: scan/register projects, synthesize a dependency-ordered PLAN.yaml, and validate it — WITHOUT running it. Returns the plan output + path so you can review it, then execute with maestro_run. Uses the deterministic (zero-token) planner by default. This is the entry point for delegating a cross-repo change to maestro's DAG orchestration.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "spec": { "type": "string", "description": "Natural-language goal, e.g. 'add an email field to the user profile across api and web'." },
                        "project": { "type": "string", "description": "Restrict planning to a single registered project." },
                        "out": { "type": "string", "description": "Write the PLAN.yaml to this path so you can pass it to maestro_run / maestro_plan_validate." },
                        "dry": { "type": "boolean", "description": "Plan + analyze only (never execute). This tool never executes regardless; set for an analysis-only pass." }
                    },
                    "required": ["spec"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_run",
                "description": "Execute a PLAN.yaml as a dependency-ordered DAG (contract gates, risk-based approval, verification). Long-running, so it starts in the BACKGROUND and returns immediately: poll maestro_runs_list (newest entry) then maestro_run_get / maestro_state for progress, and release any approval gates with maestro_approve.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "plan": { "type": "string", "description": "Path to the PLAN.yaml to run (produced by maestro_work)." },
                        "plan_hash": { "type": "string", "description": "Optional integrity guard: expected hash from `maestro plan hash <plan>`. The run is refused if the file no longer matches." },
                        "only": { "type": "string", "description": "Comma-separated task ids to run exclusively." },
                        "skip": { "type": "string", "description": "Comma-separated task ids to skip." }
                    },
                    "required": ["plan"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_rerun",
                "description": "Fork a prior run / re-run a plan, reusing what already passed and re-running only what blocked. Long-running: starts in the BACKGROUND like maestro_run; poll for progress.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "plan": { "type": "string", "description": "Path to the PLAN.yaml." },
                        "from": { "type": "string", "description": "Run id whose passed tasks should be reused as the seed." },
                        "plan_hash": { "type": "string", "description": "Optional integrity guard (see maestro_run)." }
                    },
                    "required": ["plan"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_approve",
                "description": "Release a task that is paused awaiting approval (a contract-touching / high-risk change, or an explicit plan/outcome gate). Find what's waiting via maestro_state's `approvals_pending` / `pending_gate`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task": { "type": "string", "description": "Task id awaiting approval, or a gate marker (__gate_plan__ / __gate_outcome__)." }
                    },
                    "required": ["task"],
                    "additionalProperties": false
                }
            },
            {
                "name": "maestro_plan_validate",
                "description": "Statically validate a PLAN.yaml (unknown projects, shell syntax, dependency cycles, size caps) without running it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "plan": { "type": "string", "description": "Path to the PLAN.yaml." },
                        "plan_hash": { "type": "string", "description": "Optional integrity guard (see maestro_run)." }
                    },
                    "required": ["plan"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

/// Dispatch a `tools/call` request. `params` is the raw value from the
/// JSON-RPC request; we extract `name` and `arguments`.
pub fn call(params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .context("tools/call missing `name`")?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    let payload = match name {
        "maestro_state" => tool_state()?,
        "maestro_runs_list" => tool_runs_list(&args)?,
        "maestro_run_get" => tool_run_get(&args)?,
        "maestro_memory_search" => tool_memory_search(&args)?,
        "maestro_roles_list" => tool_roles_list()?,
        "maestro_role_show" => tool_role_show(&args)?,
        "maestro_projects_list" => tool_projects_list()?,
        "maestro_mailbox_list" => tool_mailbox_list(&args)?,
        "maestro_mailbox_send" => tool_mailbox_send(&args)?,
        "maestro_mailbox_ask" => tool_mailbox_ask(&args)?,
        "maestro_mailbox_resolve" => tool_mailbox_resolve(&args)?,
        "maestro_work" => tool_work(&args)?,
        "maestro_run" => tool_run(&args)?,
        "maestro_rerun" => tool_rerun(&args)?,
        "maestro_approve" => tool_approve(&args)?,
        "maestro_plan_validate" => tool_plan_validate(&args)?,
        other => anyhow::bail!("unknown tool: {other}"),
    };

    Ok(wrap_text(&payload))
}

fn wrap_text(value: &Value) -> Value {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if text.len() > MAX_RESULT_BYTES {
        let original_bytes = text.len();
        let mut preview_len = text.len().min(MAX_RESULT_BYTES.saturating_sub(512));
        loop {
            while preview_len > 0 && !text.is_char_boundary(preview_len) {
                preview_len -= 1;
            }
            let envelope = json!({
                "truncated": true,
                "original_bytes": original_bytes,
                "preview": &text[..preview_len],
                "note": "MCP tool result exceeded the display limit; preview is truncated but this envelope is valid JSON."
            });
            let serialized =
                serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string());
            if serialized.len() <= MAX_RESULT_BYTES || preview_len == 0 {
                text = serialized;
                break;
            }
            let over_by = serialized.len().saturating_sub(MAX_RESULT_BYTES);
            preview_len = preview_len.saturating_sub(over_by + 128);
        }
    }
    json!({ "content": [ { "type": "text", "text": text } ] })
}

// ─────────────────── write tools: trigger maestro runs ───────────────────
//
// These let an MCP client (e.g. an autonomous front-end agent) delegate a
// dependency-ordered, contract-gated multi-repo change to maestro. They reuse
// the same verb→argv mapping and plan-hash integrity guard as the chat
// `maestro-action` path (crate::chat::actions::build_argv), then either run the
// subcommand to completion (quick actions) or start it detached (long DAG runs,
// so the serial MCP stdio loop isn't frozen for the whole run).

/// Coerce MCP JSON arguments into the flat string map `build_argv` expects
/// (mirrors the chat action YAML coercion: strings/bools/numbers).
fn args_to_map(args: &Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            map.insert(k.clone(), s);
        }
    }
    map
}

fn action_for(verb: ActionVerb, args: &Value) -> Action {
    let args = args_to_map(args);
    Action {
        id: String::new(),
        label: Action::label_for(verb, &args),
        verb,
        args,
        status: None,
        output: None,
    }
}

/// Run `maestro <argv>` to completion and capture combined output. For the
/// quick synchronous tools (work-plan / approve / validate).
fn run_maestro_sync(argv: &[String]) -> Result<String> {
    let exe = std::env::current_exe().context("locate maestro executable")?;
    let output = std::process::Command::new(&exe)
        .args(argv)
        .stdin(std::process::Stdio::null())
        .output()
        .context("spawn maestro subcommand")?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = String::from_utf8_lossy(&output.stderr);
    if !err.trim().is_empty() {
        combined.push_str("\n[stderr]\n");
        combined.push_str(&err);
    }
    if !output.status.success() {
        anyhow::bail!(
            "maestro {argv:?} exited {:?}\n{combined}",
            output.status.code()
        );
    }
    Ok(combined)
}

/// Start `maestro <argv>` in the background (detached, own process group so it
/// outlives this MCP server) and return immediately. Used for long-running
/// `run`/`rerun` so the serial MCP loop isn't blocked for the whole DAG; the
/// client polls maestro_runs_list / maestro_run_get for progress.
fn spawn_maestro_detached(argv: &[String]) -> Result<()> {
    let exe = std::env::current_exe().context("locate maestro executable")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().context("spawn detached maestro run")?;
    Ok(())
}

fn require_existing_plan(action: &Action) -> Result<()> {
    if let Some(plan) = action.args.get("plan") {
        if !std::path::Path::new(plan).exists() {
            anyhow::bail!("plan file not found: {plan}");
        }
    }
    Ok(())
}

fn tool_work(args: &Value) -> Result<Value> {
    let action = action_for(ActionVerb::Work, args);
    // build_argv for Work only adds --run when args["run"] == "true", which this
    // tool never sets — so this plans + validates without executing.
    let argv = build_argv(&action)?;
    let output = run_maestro_sync(&argv)?;
    Ok(json!({ "ok": true, "verb": "work", "argv": argv, "output": output }))
}

fn tool_run(args: &Value) -> Result<Value> {
    let action = action_for(ActionVerb::Run, args);
    let argv = build_argv(&action)?; // enforces plan_hash guard when provided
    require_existing_plan(&action)?; // fail fast — a detached spawn would hide it
    spawn_maestro_detached(&argv)?;
    Ok(json!({
        "ok": true,
        "verb": "run",
        "started": true,
        "argv": argv,
        "hint": "Run started in the background. Poll maestro_runs_list (newest entry), then maestro_run_get / maestro_state for progress; release any approval gates with maestro_approve."
    }))
}

fn tool_rerun(args: &Value) -> Result<Value> {
    let action = action_for(ActionVerb::Rerun, args);
    let argv = build_argv(&action)?;
    require_existing_plan(&action)?;
    spawn_maestro_detached(&argv)?;
    Ok(json!({
        "ok": true,
        "verb": "rerun",
        "started": true,
        "argv": argv,
        "hint": "Rerun started in the background. Poll maestro_runs_list / maestro_run_get for progress."
    }))
}

fn tool_approve(args: &Value) -> Result<Value> {
    let action = action_for(ActionVerb::Approve, args);
    let argv = build_argv(&action)?;
    let output = run_maestro_sync(&argv)?;
    Ok(json!({ "ok": true, "verb": "approve", "argv": argv, "output": output }))
}

fn tool_plan_validate(args: &Value) -> Result<Value> {
    let action = action_for(ActionVerb::PlanValidate, args);
    let argv = build_argv(&action)?;
    let output = run_maestro_sync(&argv)?;
    Ok(json!({ "ok": true, "verb": "plan_validate", "argv": argv, "output": output }))
}

// ──────────────────────────── tool impls ────────────────────────────

fn tool_state() -> Result<Value> {
    match paths::current_run_dir()? {
        Some(dir) => {
            let state = RunState::load(&dir).context("load current run state")?;
            Ok(serde_json::to_value(&state)?)
        }
        None => Ok(json!({})),
    }
}

fn tool_runs_list(args: &Value) -> Result<Value> {
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10)
        .min(MAX_RUNS_LISTED);

    let runs_dir = paths::runs_dir()?;
    if !runs_dir.exists() {
        return Ok(json!([]));
    }

    let mut ids: Vec<String> = std::fs::read_dir(&runs_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != "current"
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    ids.sort();
    ids.reverse(); // newest first (filenames are timestamp-prefixed)
    ids.truncate(limit);

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let dir = paths::run_dir_for_id(&id)?;
        match RunState::load(&dir) {
            Ok(state) => out.push(json!({
                "run_id": state.run_id,
                "spec": state.spec,
                "status": format!("{:?}", state.status).to_lowercase(),
                "started_at": state.started_at.to_rfc3339(),
                "ended_at": state.ended_at.map(|t| t.to_rfc3339()),
                "verified": state.verified,
                "tasks_total": state.tasks.len(),
                "acceptance_pass_fail": state.acceptance_summary(),
            })),
            Err(_) => out.push(json!({ "run_id": id, "error": "could not load state" })),
        }
    }
    Ok(json!(out))
}

fn tool_run_get(args: &Value) -> Result<Value> {
    let run_id = args
        .get("run_id")
        .and_then(|v| v.as_str())
        .context("run_get missing `run_id`")?;

    let dir = if run_id == "current" {
        paths::current_run_dir()?.context("no current run")?
    } else {
        paths::run_dir_for_id(run_id)?
    };

    if !dir.exists() {
        anyhow::bail!("run {run_id:?} does not exist");
    }
    let state = RunState::load(&dir).context("load run state")?;
    Ok(serde_json::to_value(&state)?)
}

fn tool_memory_search(args: &Value) -> Result<Value> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("memory_search missing `query`")?;
    let k = args
        .get("k")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(5)
        .min(20);

    let index = retrieval::build_index()?;
    let hits = retrieval::retrieve(&index, query, k);
    let trimmed = retrieval::fit_budget(hits, MAX_RESULT_BYTES / 2, 2_000);

    let payload: Vec<Value> = trimmed
        .into_iter()
        .map(|hit| {
            json!({
                "id": hit.chunk.id,
                "source": match hit.chunk.source {
                    retrieval::ChunkSource::L1Fact => "l1_fact",
                    retrieval::ChunkSource::L2Decision => "l2_decision",
                    retrieval::ChunkSource::Replan => "replan",
                },
                "score": hit.score,
                "content": hit.chunk.content,
            })
        })
        .collect();
    Ok(json!(payload))
}

fn tool_roles_list() -> Result<Value> {
    let all = roles::list_all()?;
    let payload: Vec<Value> = all
        .into_iter()
        .map(|r| {
            json!({
                "name": r.name,
                "display": r.display,
                "summary": r.summary,
                "tags": r.tags,
                "builtin": r.builtin,
            })
        })
        .collect();
    Ok(json!(payload))
}

fn tool_role_show(args: &Value) -> Result<Value> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("role_show missing `name`")?;
    let role = roles::load(name)?;
    Ok(json!({
        "name": role.name,
        "display": role.display,
        "summary": role.summary,
        "tags": role.tags,
        "source": role.source,
        "builtin": role.builtin,
        "prelude": role.prelude,
    }))
}

fn tool_projects_list() -> Result<Value> {
    let pf = paths::projects_file()?;
    if !pf.exists() {
        return Ok(json!([]));
    }
    let cfg = ProjectsConfig::load(&pf).context("load projects.yaml")?;
    let payload: Vec<Value> = cfg
        .projects
        .iter()
        .map(|(name, p)| {
            json!({
                "name": name,
                "path": p.path,
                "type": p.r#type,
                "stack": p.stack,
                "role": p.role,
                "memory_scope": p.memory_scope,
                "contracts": {
                    "provides": p.contracts.provides,
                    "consumes": p.contracts.consumes,
                }
            })
        })
        .collect();
    Ok(json!(payload))
}

fn tool_mailbox_list(args: &Value) -> Result<Value> {
    let store = crate::mailbox::MailboxStore::open()?;
    let filter = crate::mailbox::MailFilter {
        status: (!args.get("all").and_then(|v| v.as_bool()).unwrap_or(false))
            .then_some(crate::mailbox::MailStatus::Open),
        to: args.get("to").and_then(|v| v.as_str()).map(str::to_string),
        project: args
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };
    Ok(serde_json::to_value(store.list(&filter)?)?)
}

fn tool_mailbox_send(args: &Value) -> Result<Value> {
    let store = crate::mailbox::MailboxStore::open()?;
    let get = |key: &str| -> Result<String> {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .with_context(|| format!("mailbox_send missing `{key}`"))
    };
    let message = store.send(crate::mailbox::MailDraft {
        from: get("from")?,
        to: get("to")?,
        project: args
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        task: args
            .get("task")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        subject: get("subject")?,
        body: get("body")?,
        blocking: args
            .get("blocking")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        ask: None,
    })?;
    Ok(serde_json::to_value(message)?)
}

/// Parse the `options`/`default` args into an [`Ask`] (shared with the tests).
fn parse_ask(args: &Value) -> Result<(String, String, crate::ask::Ask)> {
    use crate::ask::{Ask, AskOption};
    let subject = args
        .get("subject")
        .and_then(|v| v.as_str())
        .context("mailbox_ask missing `subject`")?
        .to_string();
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or(&subject)
        .to_string();

    let opts = args
        .get("options")
        .and_then(|v| v.as_array())
        .context("mailbox_ask missing `options`")?;
    let mut options = Vec::new();
    for o in opts {
        if let Some(s) = o.as_str() {
            options.push(AskOption::new(s, s));
        } else {
            let key = o
                .get("key")
                .and_then(|v| v.as_str())
                .context("option missing `key`")?;
            let label = o.get("label").and_then(|v| v.as_str()).unwrap_or(key);
            options.push(AskOption::new(key, label));
        }
    }
    anyhow::ensure!(options.len() >= 2, "mailbox_ask needs at least 2 options");

    let default: Vec<String> = args
        .get("default")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    anyhow::ensure!(
        !default.is_empty() && default.iter().all(|k| options.iter().any(|o| &o.key == k)),
        "mailbox_ask needs a non-empty `default` of valid option keys"
    );

    Ok((
        subject,
        body,
        Ask {
            prompt,
            options,
            multi_select: args
                .get("multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            default,
        },
    ))
}

/// Post a structured question to the human and block until it's answered (or a
/// timeout falls back to the ask's default). The Web UI renders it as buttons.
fn tool_mailbox_ask(args: &Value) -> Result<Value> {
    use crate::mailbox::{MailDraft, MailStatus, MailboxStore};

    let (subject, body, ask) = parse_ask(args)?;
    let default = ask.default.clone();
    let timeout = args
        .get("timeout_secs")
        .and_then(|v| v.as_f64())
        .unwrap_or(300.0)
        .max(1.0);

    let store = MailboxStore::open()?;
    let msg = store.send(MailDraft {
        from: args
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("agent")
            .to_string(),
        to: "human".to_string(),
        project: args
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from),
        task: args.get("task").and_then(|v| v.as_str()).map(String::from),
        subject,
        body,
        blocking: true,
        ask: Some(ask),
    })?;

    // Block (on the blocking pool) until a human answers via the Web UI, or the
    // timeout elapses and we record the default ourselves.
    let start = std::time::Instant::now();
    loop {
        let cur = store.load(&msg.id)?;
        if cur.status == MailStatus::Resolved {
            return Ok(json!({
                "id": msg.id,
                "answered": cur.answer.unwrap_or_default(),
                "resolution": cur.resolution,
                "timed_out": false,
            }));
        }
        if start.elapsed().as_secs_f64() >= timeout {
            let settled = store.answer(&msg.id, &default)?;
            return Ok(json!({
                "id": msg.id,
                "answered": settled.answer.unwrap_or_default(),
                "resolution": settled.resolution,
                "timed_out": true,
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(800));
    }
}

fn tool_mailbox_resolve(args: &Value) -> Result<Value> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .context("mailbox_resolve missing `id`")?;
    let note = args
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let message = crate::mailbox::MailboxStore::open()?.resolve(id, note)?;
    Ok(serde_json::to_value(message)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn descriptors_advertise_every_tool() {
        let v = list_descriptors();
        let tools = v["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"maestro_state"));
        assert!(names.contains(&"maestro_runs_list"));
        assert!(names.contains(&"maestro_memory_search"));
        assert!(names.contains(&"maestro_roles_list"));
        assert!(names.contains(&"maestro_role_show"));
        assert!(names.contains(&"maestro_run_get"));
        assert!(names.contains(&"maestro_projects_list"));
        assert!(names.contains(&"maestro_mailbox_list"));
        assert!(names.contains(&"maestro_mailbox_send"));
        assert!(names.contains(&"maestro_mailbox_resolve"));
        assert!(names.contains(&"maestro_mailbox_ask"));
        // write tools (trigger runs)
        assert!(names.contains(&"maestro_work"));
        assert!(names.contains(&"maestro_run"));
        assert!(names.contains(&"maestro_rerun"));
        assert!(names.contains(&"maestro_approve"));
        assert!(names.contains(&"maestro_plan_validate"));
    }

    #[test]
    fn write_tools_validate_args_before_spawning() {
        // Missing required args must surface a clean error (from build_argv),
        // never a panic and never a spawned subprocess.
        assert!(call(&json!({ "name": "maestro_work", "arguments": {} })).is_err());
        assert!(call(&json!({ "name": "maestro_run", "arguments": {} })).is_err());
        assert!(call(&json!({ "name": "maestro_approve", "arguments": {} })).is_err());
        // A run against a non-existent plan path fails fast (would otherwise be
        // hidden behind a detached spawn).
        let err = call(&json!({
            "name": "maestro_run",
            "arguments": { "plan": "/no/such/plan-xyz.yaml" }
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("plan file not found"), "got: {err}");
    }

    #[test]
    fn parse_ask_builds_ask_and_validates_default() {
        let args = json!({
            "subject": "strategy?",
            "options": [
                {"key": "breaking", "label": "Breaking"},
                {"key": "additive", "label": "Additive"}
            ],
            "default": ["additive"],
            "multi_select": false
        });
        let (subject, _body, ask) = parse_ask(&args).unwrap();
        assert_eq!(subject, "strategy?");
        assert_eq!(ask.prompt, "strategy?"); // defaults to subject
        assert_eq!(ask.options.len(), 2);
        assert_eq!(ask.default, vec!["additive"]);

        // string options shorthand (key == label)
        let shorthand = json!({"subject":"x","options":["yes","no"],"default":["yes"]});
        let (_, _, a2) = parse_ask(&shorthand).unwrap();
        assert_eq!(a2.options[0].key, "yes");

        // a default that isn't a real option is rejected
        let bad = json!({"subject":"x","options":["yes","no"],"default":["maybe"]});
        assert!(parse_ask(&bad).is_err());

        // fewer than 2 options is rejected
        let one = json!({"subject":"x","options":["only"],"default":["only"]});
        assert!(parse_ask(&one).is_err());
    }

    #[test]
    fn descriptors_carry_input_schema_for_every_tool() {
        let v = list_descriptors();
        for t in v["tools"].as_array().unwrap() {
            assert!(
                t.get("inputSchema").is_some(),
                "tool {:?} missing inputSchema",
                t["name"]
            );
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn call_rejects_unknown_tool() {
        let err = call(&json!({ "name": "not_real" })).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn wrap_text_truncates_huge_payloads() {
        let big = json!({ "x": "a".repeat(MAX_RESULT_BYTES * 2) });
        let wrapped = wrap_text(&big);
        let text = wrapped["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert!(parsed["original_bytes"].as_u64().unwrap() > MAX_RESULT_BYTES as u64);
        assert!(parsed["preview"].as_str().unwrap().contains("\"x\""));
        assert!(text.len() <= MAX_RESULT_BYTES);
    }
}
