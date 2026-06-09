//! Chat session CRUD, tagging, message streaming, and action execution.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{sse::Event, IntoResponse, Json, Response, Sse},
};
use std::convert::Infallible;
use std::time::Duration;

/// F-119: reject a malformed id at the boundary with a neutral `400` BEFORE any
/// filesystem access or lookup, so an invalid id never becomes a 404 (looks like
/// "not found") or a 500 (path-derived error). Returns `Some(response)` to short-
/// circuit; `None` when the id is a safe single path component.
fn reject_invalid_id(kind: &'static str, id: &str) -> Option<Response> {
    crate::paths::validate_path_component(kind, id)
        .err()
        .map(|_| (StatusCode::BAD_REQUEST, format!("invalid {kind}")).into_response())
}

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
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
    match crate::chat::sessions::load(&id) {
        Ok(s) => Json(s).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

pub async fn chat_session_delete(Path(id): Path<String>) -> Response {
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
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
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
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
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
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
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
    let Ok(mut s) = crate::chat::sessions::load(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    match crate::chat::auto_tag(&mut s).await {
        Ok(_) => Json(&s).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

pub async fn chat_session_compact(Path(id): Path<String>) -> Response {
    if let Some(r) = reject_invalid_id("chat session id", &id) {
        return r;
    }
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
    if let Some(r) = reject_invalid_id("chat session id", &body.id) {
        return r;
    }
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
    /// F-119 idempotency key: stable across a retry of the same submission until
    /// the turn reaches done/failed. A duplicate replays / refuses rather than
    /// re-sending the user message + first-turn prelude.
    #[serde(default)]
    turn_id: Option<String>,
}

pub async fn chat_messages_post(body: Json<MessagesBody>) -> Response {
    use crate::chat::turn::TurnError;
    let MessagesBody {
        text,
        session_id,
        model,
        provider,
        mode,
        turn_id,
    } = body.0;

    // F-119: go through the shared ownership guard. Guard failures (invalid id /
    // busy / turn dedup) return 400/409 BEFORE any stream starts; a duplicate done
    // turn replays the persisted assistant; the success path wraps the same SSE.
    let rx = match crate::chat::turn::start_chat_turn(crate::chat::turn::ChatTurnRequest {
        session_id,
        text,
        model,
        provider,
        mode,
        turn_id,
    })
    .await
    {
        Ok(rx) => rx,
        Err(TurnError::InvalidId) => {
            return (StatusCode::BAD_REQUEST, "invalid chat session id").into_response();
        }
        Err(TurnError::Busy) => {
            return (
                StatusCode::CONFLICT,
                "session is busy in another turn or action",
            )
                .into_response();
        }
        Err(TurnError::TurnRunning) => {
            return (StatusCode::CONFLICT, "turn already running").into_response();
        }
        Err(TurnError::TurnPayloadMismatch) => {
            return (
                StatusCode::CONFLICT,
                "turn id reused with a different request",
            )
                .into_response();
        }
        Err(TurnError::TurnNotRetriable) => {
            return (
                StatusCode::CONFLICT,
                "turn is not retriable; start a new turn",
            )
                .into_response();
        }
        Err(TurnError::Internal(e)) => {
            // The detail (control corrupt / save / lock error) can quote a path —
            // log it server-side; the HTTP body stays neutral.
            tracing::warn!(error = ?e, "chat turn failed before stream");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "chat turn failed before stream",
            )
                .into_response();
        }
    };

    let mut rx = rx;
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

    Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
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
    let decision = body
        .and_then(|b| b.0.decision)
        .unwrap_or_else(|| "approve".into());

    // F-119: actions run through the shared ownership guard (approve acquires the
    // session for the duration; a busy session refuses with 409).
    match crate::chat::turn::run_chat_action_with_owner(&session_id, &action_id, &decision).await {
        Ok(act) => Json(act).into_response(),
        Err(crate::chat::turn::ActionError::InvalidId) => {
            (StatusCode::BAD_REQUEST, "invalid chat id").into_response()
        }
        Err(crate::chat::turn::ActionError::SessionNotFound) => {
            (StatusCode::NOT_FOUND, "session not found").into_response()
        }
        Err(crate::chat::turn::ActionError::ActionNotFound) => {
            (StatusCode::NOT_FOUND, "action not found").into_response()
        }
        Err(crate::chat::turn::ActionError::Busy) => (
            StatusCode::CONFLICT,
            "session is busy in another turn or action",
        )
            .into_response(),
        Err(crate::chat::turn::ActionError::Internal(e)) => {
            tracing::warn!(error = ?e, "chat action failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "chat action failed").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_session_id_is_400_not_404_or_500() {
        // F-119 N1: an unsafe id is a 400 at the boundary (before any FS), not a
        // 404 (looks "not found") or a 500 (path-derived error). Covers
        // get / delete / current / action.
        for bad in ["../escape", "a/b", ".."] {
            assert_eq!(
                chat_session_get(Path(bad.to_string())).await.status(),
                StatusCode::BAD_REQUEST,
                "get {bad:?}"
            );
            assert_eq!(
                chat_session_delete(Path(bad.to_string())).await.status(),
                StatusCode::BAD_REQUEST,
                "delete {bad:?}"
            );
            assert_eq!(
                chat_current_put(Json(CurrentBody {
                    id: bad.to_string()
                }))
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "current {bad:?}"
            );
            assert_eq!(
                chat_action_post(Path((bad.to_string(), "a-1".to_string())), None)
                    .await
                    .status(),
                StatusCode::BAD_REQUEST,
                "action session {bad:?}"
            );
        }
        // a malformed action id is also a 400 (valid session id, bad action id).
        assert_eq!(
            chat_action_post(Path(("s-ok".to_string(), "../a".to_string())), None)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn pre_stream_internal_error_500_body_is_neutral() {
        // N3: a corrupt control file makes start_chat_turn fail before the stream;
        // the 500 body must be a fixed neutral string, never the raw error (which
        // can quote a path).
        use axum::body::to_bytes;
        let dir = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
        }
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();

        let s = crate::chat::sessions::Session::new();
        crate::chat::sessions::save(&s).unwrap();
        std::fs::write(
            crate::chat::control::control_path(&s.id).unwrap(),
            "{not json",
        )
        .unwrap();

        let resp = chat_messages_post(Json(MessagesBody {
            text: "hi".into(),
            session_id: Some(s.id.clone()),
            model: None,
            provider: None,
            mode: None,
            turn_id: None,
        }))
        .await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8_lossy(&bytes);
        assert_eq!(body, "chat turn failed before stream");
        assert!(
            !body.contains('/'),
            "neutral body must not contain a path: {body}"
        );

        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }
}
