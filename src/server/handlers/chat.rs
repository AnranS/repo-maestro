//! Chat session CRUD, tagging, message streaming, and action execution.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{sse::Event, IntoResponse, Json, Response, Sse},
};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;

pub async fn chat_sessions_list() -> Response {
    match crate::chat::sessions::list_sessions() {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn chat_sessions_create() -> Response {
    let s = crate::chat::sessions::Session::new();
    if let Err(e) = crate::chat::sessions::save(&s) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    let _ = crate::chat::sessions::set_current(&s.id);
    Json(s).into_response()
}

pub async fn chat_session_get(Path(id): Path<String>) -> Response {
    match crate::chat::sessions::load(&id) {
        Ok(s) => Json(s).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

pub async fn chat_session_delete(Path(id): Path<String>) -> Response {
    match crate::chat::sessions::delete(&id) {
        Ok(()) => (StatusCode::NO_CONTENT, "").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct PatchSession {
    #[serde(default)]
    title: Option<String>,
    /// `Some("")` or `Some(null)` clears the per-session pin and falls back
    /// to the global default.
    #[serde(default)]
    cursor_model: Option<Option<String>>,
    /// Per-session chat provider pin. `null` or "" clears the pin and falls
    /// back to env/settings/default provider resolution.
    #[serde(default)]
    chat_provider: Option<Option<String>>,
}

pub async fn chat_session_patch(Path(id): Path<String>, body: Json<PatchSession>) -> Response {
    let Ok(mut s) = crate::chat::sessions::load(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    let mut changed = false;
    if let Some(t) = &body.title {
        s.title = t.clone();
        changed = true;
    }
    if let Some(cm) = &body.cursor_model {
        s.cursor_model = match cm {
            Some(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
            _ => None,
        };
        changed = true;
    }
    if let Some(provider) = &body.chat_provider {
        s.chat_provider = match provider {
            Some(v) if !v.trim().is_empty() => {
                let id = v.trim().to_string();
                if crate::chat::providers::resolve(&id).is_none() {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("unknown chat provider `{id}`"),
                    )
                        .into_response();
                }
                Some(id)
            }
            _ => None,
        };
        changed = true;
    }
    if changed {
        s.updated_at = chrono::Utc::now();
    }
    if let Err(e) = crate::chat::sessions::save(&s) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    Json(s).into_response()
}

#[derive(serde::Deserialize)]
pub struct TagsPut {
    tags: Vec<String>,
}

pub async fn chat_session_tags_put(Path(id): Path<String>, body: Json<TagsPut>) -> Response {
    let Ok(mut s) = crate::chat::sessions::load(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    s.tags = normalize_tags(&body.tags);
    s.updated_at = chrono::Utc::now();
    if let Err(e) = crate::chat::sessions::save(&s) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    Json(&s).into_response()
}

pub async fn chat_session_auto_tag(Path(id): Path<String>) -> Response {
    let Ok(mut s) = crate::chat::sessions::load(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    match crate::chat::auto_tag(&mut s).await {
        Ok(_) => Json(&s).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

pub async fn chat_session_compact(Path(id): Path<String>) -> Response {
    let Ok(mut s) = crate::chat::sessions::load(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    match crate::chat::compact_session(&mut s).await {
        Ok(n) => Json(serde_json::json!({
            "session": &s,
            "messages_summarized": n,
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

/// Mirrors `cli::util::clean_tag` but for batch input. Filters and dedupes.
fn normalize_tags(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in raw {
        let cleaned: String = t
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !cleaned.is_empty() && cleaned.len() <= 32 && !out.contains(&cleaned) {
            out.push(cleaned);
        }
    }
    out
}

#[derive(serde::Deserialize, Default)]
pub struct ExternalQuery {
    source: Option<String>,
}

pub async fn external_sessions_list(Query(params): Query<ExternalQuery>) -> Response {
    match crate::external::list_sessions(params.source.as_deref()).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

pub async fn chat_current_get() -> Response {
    let id = crate::chat::sessions::read_current().ok().flatten();
    Json(serde_json::json!({ "id": id })).into_response()
}

#[derive(serde::Deserialize)]
pub struct CurrentBody {
    id: String,
}

pub async fn chat_current_put(body: Json<CurrentBody>) -> Response {
    if crate::chat::sessions::load(&body.id).is_err() {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    }
    if let Err(e) = crate::chat::sessions::set_current(&body.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    (StatusCode::NO_CONTENT, "").into_response()
}

#[derive(serde::Deserialize)]
pub struct MessagesBody {
    text: String,
    #[serde(default)]
    session_id: Option<String>,
    /// Per-message Cursor model override.
    #[serde(default)]
    model: Option<String>,
    /// Per-message chat provider override.
    #[serde(default)]
    provider: Option<String>,
    /// `plan` = read-only analysis turn (model is told NOT to emit
    /// maestro-action blocks). `exec` = full agency (default).
    #[serde(default)]
    mode: Option<String>,
}

pub async fn chat_messages_post(
    body: Json<MessagesBody>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    use tokio::sync::mpsc;

    let MessagesBody {
        text,
        session_id,
        model,
        provider,
        mode,
    } = body.0;

    // In plan mode, prepend a tight constraint to the user's message. We
    // do this rather than threading through a new parameter to stream.rs
    // because the constraint should appear in the message history (so the
    // model remembers we asked for plan-only) — which is exactly what a
    // user prefix does.
    let text = match mode.as_deref() {
        Some("plan") => format!(
            "[plan mode] Please analyse and propose what you would do. Do NOT emit any `maestro-action` blocks; describe the actions in prose instead. The user will switch to exec mode when ready.\n\n{text}"
        ),
        _ => text,
    };

    let session = match session_id {
        Some(id) => crate::chat::sessions::load(&id).unwrap_or_else(|_| {
            let s = crate::chat::sessions::Session::new();
            let _ = crate::chat::sessions::save(&s);
            let _ = crate::chat::sessions::set_current(&s.id);
            s
        }),
        None => crate::chat::sessions::ensure_current()
            .unwrap_or_else(|_| crate::chat::sessions::Session::new()),
    };
    let _ = crate::chat::sessions::set_current(&session.id);

    let (tx, mut rx) = mpsc::channel::<crate::chat::StreamEvent>(64);

    tokio::spawn(async move {
        if let Err(e) = crate::chat::stream::send_streaming_with_options(
            session,
            text,
            model,
            provider,
            tx.clone(),
        )
        .await
        {
            let _ = tx
                .send(crate::chat::StreamEvent::Error {
                    message: format!("{e:#}"),
                })
                .await;
        }
    });

    let stream = async_stream::stream! {
        while let Some(ev) = rx.recv().await {
            let (event_name, data) = match &ev {
                crate::chat::StreamEvent::Meta { .. }     => ("meta",     serde_json::to_string(&ev).unwrap_or_default()),
                crate::chat::StreamEvent::Delta { .. }    => ("delta",    serde_json::to_string(&ev).unwrap_or_default()),
                crate::chat::StreamEvent::Thinking { .. } => ("thinking", serde_json::to_string(&ev).unwrap_or_default()),
                crate::chat::StreamEvent::Done  { .. }    => ("done",     serde_json::to_string(&ev).unwrap_or_default()),
                crate::chat::StreamEvent::Error { .. }    => ("error",    serde_json::to_string(&ev).unwrap_or_default()),
            };
            yield Ok::<_, Infallible>(Event::default().event(event_name).data(data));
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

#[derive(serde::Deserialize)]
pub struct ActionDecision {
    #[serde(default)]
    decision: Option<String>,
}

pub async fn chat_action_post(
    Path((session_id, action_id)): Path<(String, String)>,
    body: Option<Json<ActionDecision>>,
) -> Response {
    let Ok(mut session) = crate::chat::sessions::load(&session_id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };

    let decision = body
        .and_then(|b| b.0.decision)
        .unwrap_or_else(|| "approve".into());

    let mut found: Option<(usize, usize)> = None;
    for (mi, m) in session.messages.iter().enumerate() {
        for (ai, a) in m.actions.iter().enumerate() {
            if a.id == action_id {
                found = Some((mi, ai));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let Some((mi, ai)) = found else {
        return (StatusCode::NOT_FOUND, "action not found").into_response();
    };

    if decision == "reject" {
        session.messages[mi].actions[ai].status = Some(crate::chat::ActionStatus::Rejected);
        let _ = crate::chat::sessions::save(&session);
        return Json(&session.messages[mi].actions[ai]).into_response();
    }

    session.messages[mi].actions[ai].status = Some(crate::chat::ActionStatus::Running);
    let _ = crate::chat::sessions::save(&session);
    let action = session.messages[mi].actions[ai].clone();
    let sid_owned = session_id.clone();

    let res = crate::chat::actions::execute_action_with_session(&action, Some(&sid_owned)).await;

    let mut session = crate::chat::sessions::load(&session_id).unwrap_or(session);
    if let Some(act) = session
        .messages
        .iter_mut()
        .flat_map(|m| m.actions.iter_mut())
        .find(|a| a.id == action_id)
    {
        match &res {
            Ok(out) => {
                act.status = Some(crate::chat::ActionStatus::Done);
                act.output = Some(out.clone());
            }
            Err(e) => {
                act.status = Some(crate::chat::ActionStatus::Failed);
                act.output = Some(format!("{e:#}"));
            }
        }
    }
    let _ = crate::chat::sessions::save(&session);

    let act = session
        .messages
        .iter()
        .flat_map(|m| m.actions.iter())
        .find(|a| a.id == action_id)
        .cloned()
        .unwrap_or(action);
    Json(act).into_response()
}
