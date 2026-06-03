//! End-to-end integration tests for the axum router via `axum-test`. We
//! don't bind a real TCP port — TestServer hits the router in-process.
//! Every test points `MAESTRO_WORKSPACE_ROOT` at a fresh tempdir so the
//! handlers' filesystem ops are isolated.

use axum_test::TestServer;
use maestro::server::{build_app, ServerState};
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn fresh_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
    }
    // Bootstrap `.maestro/` so memory/skills/etc. endpoints don't error
    // on "directory not found".
    std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
    std::fs::create_dir_all(dir.path().join(".maestro").join("runs")).unwrap();
    dir
}
fn clear() {
    unsafe {
        std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
    }
}

fn server() -> TestServer {
    let (tx, _) = broadcast::channel::<()>(8);
    TestServer::new(build_app(ServerState { tx })).unwrap()
}

// ─── state / runs ───────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn state_returns_404_when_no_current_run() {
    let _dir = fresh_workspace();
    let r = server().get("/api/state").await;
    r.assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

#[tokio::test]
#[serial]
async fn runs_list_is_empty_on_a_fresh_workspace() {
    let _dir = fresh_workspace();
    let r = server().get("/api/runs").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert!(v.is_array());
    assert_eq!(v.as_array().unwrap().len(), 0);
    clear();
}

#[tokio::test]
#[serial]
async fn run_evidence_endpoint_serves_summary_json() {
    let dir = fresh_workspace();
    let evidence_dir = dir
        .path()
        .join(".maestro")
        .join("runs")
        .join("run-1")
        .join("evidence");
    std::fs::create_dir_all(&evidence_dir).unwrap();
    std::fs::write(
        evidence_dir.join("summary.json"),
        r#"{"run_id":"run-1","max_observed_parallelism":2}"#,
    )
    .unwrap();

    let v: serde_json::Value = server().get("/api/runs/run-1/evidence").await.json();
    assert_eq!(v["run_id"], "run-1");
    assert_eq!(v["max_observed_parallelism"], 2);
    clear();
}

// ─── projects ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn projects_create_lists_and_deletes_roundtrip() {
    let dir = fresh_workspace();
    let proj_dir = dir.path().join("api");
    std::fs::create_dir_all(&proj_dir).unwrap();

    let srv = server();

    // 1. Empty registry
    let r = srv.get("/api/projects").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["projects"], json!([]));

    // 2. POST a new project
    let body = json!({
        "name": "api",
        "path": proj_dir.to_string_lossy(),
        "type": "backend",
        "stack": ["python", "fastapi"],
        "provides": "schemas/openapi.yaml",
        "agent_model": "gpt-5.2"
    });
    let created: serde_json::Value = srv.post("/api/projects").json(&body).await.json();
    assert_eq!(created["agent_model"], "gpt-5.2");
    assert!(created.get("cursor_model").is_none());

    // 3. It now appears in the listing
    let v: serde_json::Value = srv.get("/api/projects").await.json();
    assert_eq!(v["projects"], json!(["api"]));

    // 4. Architecture endpoint surfaces it as a module
    let arch: serde_json::Value = srv.get("/api/architecture").await.json();
    assert_eq!(arch["modules"].as_array().unwrap().len(), 1);
    assert_eq!(arch["modules"][0]["name"], "api");

    // 5. Duplicate POST is a 409
    srv.post("/api/projects")
        .json(&body)
        .await
        .assert_status(axum::http::StatusCode::CONFLICT);

    // 6. DELETE removes it
    srv.delete("/api/projects/api")
        .await
        .assert_status(axum::http::StatusCode::NO_CONTENT);
    let v: serde_json::Value = srv.get("/api/projects").await.json();
    assert_eq!(v["projects"], json!([]));

    clear();
}

#[tokio::test]
#[serial]
async fn projects_post_accepts_legacy_cursor_model_alias() {
    let dir = fresh_workspace();
    let proj_dir = dir.path().join("legacy");
    std::fs::create_dir_all(&proj_dir).unwrap();

    let created: serde_json::Value = server()
        .post("/api/projects")
        .json(&json!({
            "name": "legacy",
            "path": proj_dir.to_string_lossy(),
            "cursor_model": "legacy-model"
        }))
        .await
        .json();

    assert_eq!(created["agent_model"], "legacy-model");
    assert!(created.get("cursor_model").is_none());
    clear();
}

#[tokio::test]
#[serial]
async fn projects_post_rejects_missing_path() {
    let _dir = fresh_workspace();
    let r = server()
        .post("/api/projects")
        .json(&json!({
            "name": "ghost",
            "path": "/this/path/does/not/exist/123",
        }))
        .await;
    r.assert_status(axum::http::StatusCode::BAD_REQUEST);
    clear();
}

#[tokio::test]
#[serial]
async fn architecture_surfaces_declared_project_dependencies() {
    let dir = fresh_workspace();
    let core_dir = dir.path().join("core");
    let web_dir = dir.path().join("web");
    std::fs::create_dir_all(&core_dir).unwrap();
    std::fs::create_dir_all(&web_dir).unwrap();

    let srv = server();
    srv.post("/api/projects")
        .json(&json!({
            "name": "core",
            "path": core_dir.to_string_lossy(),
            "type": "library"
        }))
        .await
        .assert_status_ok();
    srv.post("/api/projects")
        .json(&json!({
            "name": "web",
            "path": web_dir.to_string_lossy(),
            "type": "frontend",
            "dependencies": ["core"]
        }))
        .await
        .assert_status_ok();

    let arch: serde_json::Value = srv.get("/api/architecture").await.json();
    let edges = arch["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["from"], "core");
    assert_eq!(edges[0]["to"], "web");
    assert_eq!(edges[0]["kind"], "dependency");
    assert_eq!(edges[0]["file"], "depends_on");

    clear();
}

// ─── memory ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn memory_put_get_list_delete_roundtrip() {
    let _dir = fresh_workspace();
    let srv = server();

    // List is empty
    let v: serde_json::Value = srv.get("/api/memory/l1").await.json();
    assert!(v.as_object().unwrap().is_empty());

    // PUT a fact
    srv.put("/api/memory/l1/api/auth.md")
        .json(&json!({ "content": "use HS256" }))
        .await
        .assert_status_ok();

    // GET it back
    let r = srv.get("/api/memory/l1/api/auth.md").await;
    r.assert_status_ok();
    assert_eq!(r.text(), "use HS256");

    // List groups it under "api"
    let v: serde_json::Value = srv.get("/api/memory/l1").await.json();
    assert_eq!(v["api"], json!(["auth.md"]));

    // DELETE clears it
    srv.delete("/api/memory/l1/api/auth.md")
        .await
        .assert_status(axum::http::StatusCode::NO_CONTENT);
    srv.get("/api/memory/l1/api/auth.md")
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

// ─── skills ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn skills_global_put_get_delete_roundtrip() {
    let _dir = fresh_workspace();
    let srv = server();

    let body = json!({
        "content": "---\ndescription: test skill\ntrigger: x|y\n---\n\n# body"
    });
    srv.put("/api/skills/_global/test-skill")
        .json(&body)
        .await
        .assert_status_ok();

    let r = srv.get("/api/skills/_global/test-skill").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["name"], "test-skill");
    assert_eq!(v["description"], "test skill");

    let listing: serde_json::Value = srv.get("/api/skills").await.json();
    assert!(!listing["_global"].as_array().unwrap().is_empty());

    srv.delete("/api/skills/_global/test-skill")
        .await
        .assert_status(axum::http::StatusCode::NO_CONTENT);
    srv.get("/api/skills/_global/test-skill")
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

// ─── docs ───────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn docs_index_serves_en_by_default_and_zh_when_asked() {
    let _dir = fresh_workspace();
    let srv = server();

    let v_en: serde_json::Value = srv.get("/api/docs/index").await.json();
    assert_eq!(v_en["lang"], "en");
    assert!(!v_en["groups"].as_array().unwrap().is_empty());

    let v_zh: serde_json::Value = srv.get("/api/docs/index?lang=zh").await.json();
    assert_eq!(v_zh["lang"], "zh");
    clear();
}

#[tokio::test]
#[serial]
async fn docs_page_rejects_path_traversal() {
    let _dir = fresh_workspace();
    let r = server().get("/api/docs/page?file=../Cargo.toml").await;
    r.assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

#[tokio::test]
#[serial]
async fn docs_page_serves_real_pages() {
    let _dir = fresh_workspace();
    let r = server().get("/api/docs/page?file=00-overview.md").await;
    r.assert_status_ok();
    // Header is set to text/markdown
    let ct = r.header("content-type");
    assert!(
        ct.to_str().unwrap().contains("text/markdown"),
        "content-type was {ct:?}"
    );
    assert!(r.text().contains("maestro"));
    clear();
}

// ─── chat sessions ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn chat_sessions_create_get_patch_delete_roundtrip() {
    let _dir = fresh_workspace();
    let srv = server();

    // Empty list
    let v: serde_json::Value = srv.get("/api/chat/sessions").await.json();
    assert_eq!(v.as_array().unwrap().len(), 0);

    // Create one
    let r = srv.post("/api/chat/sessions").await;
    r.assert_status_ok();
    let s: serde_json::Value = r.json();
    let id = s["id"].as_str().unwrap().to_string();

    // GET it
    srv.get(&format!("/api/chat/sessions/{id}"))
        .await
        .assert_status_ok();

    // PATCH title + provider/model pins
    let patched: serde_json::Value = srv
        .patch(&format!("/api/chat/sessions/{id}"))
        .json(&json!({
            "title": "renamed",
            "chat_provider": "codex",
            "cursor_model": "gpt-5.2"
        }))
        .await
        .json();
    assert_eq!(patched["title"], "renamed");
    assert_eq!(patched["chat_provider"], "codex");
    assert_eq!(patched["cursor_model"], "gpt-5.2");

    srv.patch(&format!("/api/chat/sessions/{id}"))
        .json(&json!({ "chat_provider": "unknown" }))
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);

    // DELETE
    srv.delete(&format!("/api/chat/sessions/{id}"))
        .await
        .assert_status(axum::http::StatusCode::NO_CONTENT);
    srv.get(&format!("/api/chat/sessions/{id}"))
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

#[tokio::test]
#[serial]
async fn models_endpoint_supports_provider_query() {
    let _dir = fresh_workspace();
    let srv = server();

    let codex: serde_json::Value = srv.get("/api/models?provider=codex").await.json();
    assert!(codex
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["provider"] == "codex"));

    let claude: serde_json::Value = srv.post("/api/models/refresh?provider=claude").await.json();
    assert!(claude
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["provider"] == "claude"));

    srv.get("/api/models?provider=nope")
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);
    clear();
}

// ─── defaults ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn defaults_get_returns_baseline_and_put_persists() {
    let _dir = fresh_workspace();
    let srv = server();

    let v: serde_json::Value = srv.get("/api/settings/defaults").await.json();
    assert!(v.get("agent").is_some());
    assert!(v.get("branch_prefix").is_some());

    // Need a projects.yaml on disk for the writer; minimum init.
    let pfile = std::env::var("MAESTRO_WORKSPACE_ROOT").unwrap();
    let dot_mux = std::path::Path::new(&pfile).join(".maestro");
    std::fs::write(
        dot_mux.join("projects.yaml"),
        "version: 1\ndefaults:\n  agent: cursor\n  branch_prefix: feat/\n  max_parallel: 4\nprojects: {}\n",
    )
    .unwrap();

    let patched: serde_json::Value = srv
        .put("/api/settings/defaults")
        .json(&json!({ "branch_prefix": "wip/", "agent_model": "gpt-5.2" }))
        .await
        .json();
    assert_eq!(patched["branch_prefix"], "wip/");
    assert_eq!(patched["agent_model"], "gpt-5.2");
    assert!(patched.get("cursor_model").is_none());

    // Persisted to disk
    let yaml = std::fs::read_to_string(dot_mux.join("projects.yaml")).unwrap();
    assert!(yaml.contains("branch_prefix: wip/"));
    assert!(yaml.contains("agent_model: gpt-5.2"));
    assert!(!yaml.contains("cursor_model:"));

    let patched: serde_json::Value = srv
        .put("/api/settings/defaults")
        .json(&json!({ "cursor_model": "legacy-model" }))
        .await
        .json();
    assert_eq!(patched["agent_model"], "legacy-model");
    assert!(patched.get("cursor_model").is_none());

    let yaml = std::fs::read_to_string(dot_mux.join("projects.yaml")).unwrap();
    assert!(yaml.contains("agent_model: legacy-model"));
    assert!(!yaml.contains("cursor_model:"));
    clear();
}
