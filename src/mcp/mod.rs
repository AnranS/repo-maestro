//! Minimal Model Context Protocol (MCP) server, transport = stdio.
//!
//! Implements just enough of the JSON-RPC 2.0 surface to make maestro
//! itself a tool the user's Cursor (or any MCP client) can query during a
//! chat:
//!
//! Read tools:
//!   - `maestro_state`              — current run state JSON
//!   - `maestro_runs_list`          — recent run summaries
//!   - `maestro_run_get`            — full state + report path for one run
//!   - `maestro_memory_search`      — TF-IDF query over `.maestro/memory/**`
//!   - `maestro_roles_list`         — all roles (builtin + workspace + CE)
//!   - `maestro_role_show`          — one role's prelude markdown
//!   - `maestro_projects_list`      — registered projects + contracts
//!   - `maestro_mailbox_*`          — list / send / ask / resolve handoffs
//!
//! Write tools (let an MCP client delegate a multi-repo change to maestro's
//! DAG orchestration — reuse the chat `maestro-action` verb→argv mapping +
//! plan-hash guard):
//!   - `maestro_work`               — scope + plan + validate (no execution)
//!   - `maestro_run` / `_rerun`     — execute a plan (starts in the background)
//!   - `maestro_approve`            — release a gated/awaiting-approval task
//!   - `maestro_plan_validate`      — static plan validation
//!
//! The point is **bidirectional**: your chat sessions can read what
//! maestro knows about prior runs / decisions / acceptance failures
//! without you typing or recalling — and an autonomous agent can drive
//! maestro to plan, run, and approve cross-repo work.
//!
//! Wire it into Cursor by adding to your `~/.cursor/mcp.json`:
//!
//! ```json
//! { "mcpServers": { "maestro": { "command": "maestro", "args": ["mcp"] } } }
//! ```
//!
//! Or per-workspace `.cursor/mcp.json` next to your project.

pub mod tools;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP protocol version we negotiate against. This is the spec date
/// stamp the protocol uses for versioning.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response<'a> {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError<'a>>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError<'a> {
    code: i32,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

const ERR_PARSE: i32 = -32700;
const ERR_INVALID_REQUEST: i32 = -32600;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INTERNAL: i32 = -32603;

/// Run the MCP server loop on stdio. Returns when stdin closes.
pub async fn run() -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let parsed: Result<Request, _> = serde_json::from_str(&line);
        let req = match parsed {
            Ok(r) => r,
            Err(e) => {
                let resp = error_response(
                    Value::Null,
                    ERR_PARSE,
                    "parse error",
                    Some(json!({ "detail": e.to_string() })),
                );
                write_line(&mut stdout, &resp).await?;
                continue;
            }
        };

        if req.jsonrpc != "2.0" {
            let resp = error_response(
                req.id.unwrap_or(Value::Null),
                ERR_INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
                None,
            );
            write_line(&mut stdout, &resp).await?;
            continue;
        }

        // Notifications (id absent) get processed but no response sent.
        let result = dispatch(&req.method, &req.params).await;
        let Some(id) = req.id.clone() else {
            continue;
        };
        match result {
            Ok(value) => {
                let resp = ok_response(id, value);
                write_line(&mut stdout, &resp).await?;
            }
            Err(DispatchError::MethodNotFound(method)) => {
                let resp = error_response(
                    id,
                    ERR_METHOD_NOT_FOUND,
                    "method not found",
                    Some(json!({ "method": method })),
                );
                write_line(&mut stdout, &resp).await?;
            }
            Err(DispatchError::Other(e)) => {
                let resp = error_response(
                    id,
                    ERR_INTERNAL,
                    "internal error",
                    Some(json!({ "detail": format!("{e:#}") })),
                );
                write_line(&mut stdout, &resp).await?;
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
enum DispatchError {
    MethodNotFound(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for DispatchError {
    fn from(value: anyhow::Error) -> Self {
        DispatchError::Other(value)
    }
}

async fn dispatch(method: &str, params: &Value) -> Result<Value, DispatchError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list_descriptors()),
        // Tool calls run on the blocking pool: most are quick, but some (e.g.
        // `maestro_mailbox_ask`) block until a human answers, and we must not
        // stall the async stdio loop while they wait.
        "tools/call" => {
            let params = params.clone();
            tokio::task::spawn_blocking(move || tools::call(&params))
                .await
                .map_err(|e| DispatchError::Other(anyhow::anyhow!("tool task join error: {e}")))?
                .map_err(Into::into)
        }
        other => Err(DispatchError::MethodNotFound(other.to_string())),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            // We don't advertise resources/prompts yet — just tools.
            "tools": {}
        },
        "serverInfo": {
            "name": "maestro",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn ok_response(id: Value, result: Value) -> Response<'static> {
    Response {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(
    id: Value,
    code: i32,
    message: &'static str,
    data: Option<Value>,
) -> Response<'static> {
    Response {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data,
        }),
    }
}

async fn write_line(out: &mut tokio::io::Stdout, resp: &Response<'_>) -> Result<()> {
    let s = serde_json::to_string(resp).context("serialize MCP response")?;
    out.write_all(s.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_tools_capability() {
        let v = initialize_result();
        assert_eq!(v["protocolVersion"], PROTOCOL_VERSION);
        assert!(v["capabilities"]["tools"].is_object());
        assert_eq!(v["serverInfo"]["name"], "maestro");
    }

    #[test]
    fn ok_response_round_trips_id() {
        let resp = ok_response(json!(42), json!({"x": 1}));
        let s = serde_json::to_value(&resp).unwrap();
        assert_eq!(s["id"], 42);
        assert_eq!(s["result"]["x"], 1);
        assert!(s.get("error").is_none() || s["error"].is_null());
    }

    #[test]
    fn error_response_uses_invariant_strings() {
        let resp = error_response(json!(1), ERR_METHOD_NOT_FOUND, "method not found", None);
        let s = serde_json::to_value(&resp).unwrap();
        assert_eq!(s["error"]["code"], ERR_METHOD_NOT_FOUND);
        assert_eq!(s["error"]["message"], "method not found");
    }
}
