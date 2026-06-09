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
    // F-124: the endpoint now parses/validates the summary (no raw passthrough),
    // so the persisted file must be a valid RunEvidence.
    std::fs::write(
        evidence_dir.join("summary.json"),
        r#"{"run_id":"run-1","spec":"s","status":"done","verified":true,"max_parallel":2,"max_observed_parallelism":2,"task_count":0,"tasks":[],"parallel_windows":[],"acceptance":[]}"#,
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
    // The per-skill editor route is the one full-body path.
    assert!(v["content"].as_str().unwrap().contains("# body"));

    let listing: serde_json::Value = srv.get("/api/skills").await.json();
    assert!(!listing["_global"].as_array().unwrap().is_empty());
    // F-121 N1: the sidebar listing is metadata only — no skill body on the wire.
    assert!(
        listing["_global"][0].get("content").is_none(),
        "skills list must not carry content"
    );
    let listing_str = serde_json::to_string(&listing).unwrap();
    assert!(
        !listing_str.contains("# body"),
        "skill body leaked into the list: {listing_str}"
    );

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

// ─── F-118 runtime health endpoint ──────────────────────────────────────

#[tokio::test]
#[serial]
async fn runtime_health_endpoint_returns_validated_schema_without_leaking_paths() {
    let dir = fresh_workspace();
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        "version: 1\ndefaults:\n  agent: codex\nprojects: {}\n",
    )
    .unwrap();

    let r = server().get("/api/runtime/health").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    // the validated v1 contract: stable schema + exactly the six readiness rows.
    assert_eq!(v["schema_version"], "maestro.runtime_health.v1");
    assert_eq!(v["checks"].as_array().unwrap().len(), 6);

    // the response carries no absolute workspace path or env value.
    let body = serde_json::to_string(&v).unwrap();
    assert!(!body.contains("/Users/"), "home path leaked: {body}");
    assert!(!body.contains("MAESTRO_"), "env name leaked: {body}");
    assert!(
        !body.contains(&dir.path().display().to_string()),
        "workspace path leaked: {body}"
    );
    clear();
}

// ─── F-121 skill inventory ───────────────────────────────────────────────

fn write_skill_file(dir: &TempDir, scope: &str, file: &str, body: &str) {
    let d = dir.path().join(".maestro/skills").join(scope);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(file), body).unwrap();
}

#[tokio::test]
#[serial]
async fn skill_inventory_returns_v1_schema_without_body_or_paths() {
    let dir = fresh_workspace();
    write_skill_file(
        &dir,
        "_global",
        "contract-reviewer.md",
        "---\ndescription: review contracts\ntrigger: review\n---\nINVENTORY_BODY_MARKER body text\n",
    );
    let r = server().get("/api/skills/inventory").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["schema_version"], "maestro.skill_inventory.v1");
    assert_eq!(v["visible_skills"].as_array().unwrap().len(), 1);
    assert_eq!(v["visible_skills"][0]["description"], "review contracts");

    let body = serde_json::to_string(&v).unwrap();
    assert!(
        !body.contains("INVENTORY_BODY_MARKER"),
        "skill body leaked: {body}"
    );
    assert!(!body.contains("/Users/"), "home path leaked: {body}");
    assert!(
        !body.contains(&dir.path().display().to_string()),
        "workspace path leaked: {body}"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_missing_root_is_200_empty() {
    let _dir = fresh_workspace(); // .maestro exists, but no .maestro/skills
    let r = server().get("/api/skills/inventory").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["schema_version"], "maestro.skill_inventory.v1");
    assert_eq!(v["visible_skills"].as_array().unwrap().len(), 0);
    assert_eq!(v["summary"]["skill_count"], 0);
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_unknown_project_is_404() {
    let dir = fresh_workspace();
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        "version: 1\ndefaults:\n  agent: codex\nprojects: {}\n",
    )
    .unwrap();
    let r = server().get("/api/skills/inventory?project=ghost").await;
    r.assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_unknown_profile_is_404() {
    let dir = fresh_workspace();
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        "version: 1\ndefaults:\n  agent: codex\nprojects: {}\n",
    )
    .unwrap();
    let r = server().get("/api/skills/inventory?profile=ghost").await;
    r.assert_status(axum::http::StatusCode::NOT_FOUND);
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_path_unsafe_query_is_400_with_neutral_body() {
    let _dir = fresh_workspace();
    // The 400 body must NOT echo the raw (path-unsafe) query value.
    let r = server()
        .get("/api/skills/inventory?project=/abs/secret")
        .await;
    r.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let t = r.text();
    assert!(
        !t.contains("/abs") && !t.contains("secret"),
        "raw project query leaked into body: {t}"
    );

    let r2 = server()
        .get("/api/skills/inventory?profile=../secret")
        .await;
    r2.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let t2 = r2.text();
    assert!(
        !t2.contains("secret") && !t2.contains(".."),
        "raw profile query leaked into body: {t2}"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_route_does_not_collide_with_editor() {
    let dir = fresh_workspace();
    write_skill_file(
        &dir,
        "_global",
        "contract-reviewer.md",
        "---\ndescription: d\n---\nEDITOR_BODY\n",
    );
    // The single-segment `inventory` path hits the inventory handler...
    let inv: serde_json::Value = server().get("/api/skills/inventory").await.json();
    assert_eq!(inv["schema_version"], "maestro.skill_inventory.v1");
    // ...while the two-segment editor route still serves the full skill (body).
    let r = server().get("/api/skills/_global/contract-reviewer").await;
    r.assert_status_ok();
    let skill: serde_json::Value = r.json();
    assert_eq!(skill["name"], "contract-reviewer");
    assert!(skill["content"].as_str().unwrap().contains("EDITOR_BODY"));
    clear();
}

#[tokio::test]
#[serial]
async fn skill_inventory_resolves_profile_skill_refs() {
    let dir = fresh_workspace();
    write_skill_file(
        &dir,
        "_global",
        "contract-reviewer.md",
        "---\ndescription: d\n---\nb\n",
    );
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        "version: 1\ndefaults:\n  agent: codex\n  agent_profiles:\n    reviewer:\n      role: backend_rust\n      skills:\n        - contract-reviewer\nprojects: {}\n",
    )
    .unwrap();
    let v: serde_json::Value = server()
        .get("/api/skills/inventory?profile=reviewer")
        .await
        .json();
    assert_eq!(v["profile"], "reviewer");
    let refs = v["profile_resolution"]["declared_skills"]
        .as_array()
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["resolution"], "resolved");
    assert_eq!(refs[0]["resolved_scope"], "_global");
    clear();
}

// ─── F-128 delivery endpoints ───────────────────────────────────────────

fn write_delivery(dir: &TempDir, id: &str, body: &str) {
    let p = dir.path().join(".maestro").join("deliveries").join(id);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("DELIVERY.json"), body).unwrap();
}

fn ok_delivery(id: &str, stage: &str) -> String {
    format!(
        r#"{{"schema_version":"maestro.delivery_spec.v1","delivery_id":"{id}","stage":"{stage}","created_at":"2026-06-08T00:00:00Z","intake":{{"objective":"ship {id}"}}}}"#
    )
}

#[tokio::test]
#[serial]
async fn deliveries_list_empty_on_fresh_workspace() {
    let _dir = fresh_workspace();
    let r = server().get("/api/deliveries").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert!(v.is_array() && v.as_array().unwrap().is_empty());
    clear();
}

#[tokio::test]
#[serial]
async fn deliveries_list_io_failure_is_500_not_empty() {
    let dir = fresh_workspace();
    // `.maestro/deliveries` exists but is a FILE, not a directory → read_dir fails.
    // A real storage problem must surface as 500, never a silent empty list.
    std::fs::write(dir.path().join(".maestro").join("deliveries"), "not a dir").unwrap();
    let r = server().get("/api/deliveries").await;
    assert_eq!(r.status_code(), 500);
    clear();
}

#[tokio::test]
#[serial]
async fn deliveries_list_returns_ok_envelopes() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d-aaa", &ok_delivery("d-aaa", "intake"));
    write_delivery(&dir, "d-bbb", &ok_delivery("d-bbb", "intake"));
    let v: serde_json::Value = server().get("/api/deliveries").await.json();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // ordered by delivery_id asc; ok envelopes carry the DeliveryView.
    assert_eq!(arr[0]["status"], "ok");
    assert_eq!(arr[0]["delivery_id"], "d-aaa");
    assert_eq!(arr[0]["view"]["delivery_id"], "d-aaa");
    assert_eq!(arr[0]["view"]["stage"], "intake");
    clear();
}

#[tokio::test]
#[serial]
async fn deliveries_list_shows_corrupt_stub_not_silent() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d-ok", &ok_delivery("d-ok", "intake"));
    write_delivery(&dir, "d-bad", "not json {{{");
    let v: serde_json::Value = server().get("/api/deliveries").await.json();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2, "the corrupt one is shown, not dropped");
    let bad = arr.iter().find(|e| e["delivery_id"] == "d-bad").unwrap();
    assert_eq!(bad["status"], "corrupt");
    assert!(bad.get("error").is_some(), "corrupt stub carries a reason");
    let ok = arr.iter().find(|e| e["delivery_id"] == "d-ok").unwrap();
    assert_eq!(ok["status"], "ok");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_detail_present_projects_view() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d1", &ok_delivery("d1", "intake"));
    let r = server().get("/api/deliveries/d1").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["delivery_id"], "d1");
    assert_eq!(v["stage"], "intake");
    // blocked_on is the serde externally-tagged shape (unit variant → string).
    assert_eq!(v["blocked_on"], "nothing");
    assert_eq!(v["schema_version"], "maestro.delivery_view.v1");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_detail_missing_is_404() {
    let _dir = fresh_workspace();
    let r = server().get("/api/deliveries/d-nope").await;
    assert_eq!(r.status_code(), 404);
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_detail_corrupt_is_500() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d-bad", "not json {{{");
    let r = server().get("/api/deliveries/d-bad").await;
    assert_eq!(
        r.status_code(),
        500,
        "corrupt detail → 500, never silent empty"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_detail_bad_id_is_400() {
    let _dir = fresh_workspace();
    // An encoded backslash survives URL normalization (it isn't a path separator)
    // and reaches the handler as a bad path component → validate_path_component 400.
    let r = server().get("/api/deliveries/a%5cb").await;
    assert_eq!(r.status_code(), 400);
    clear();
}

// ── F-129 delivery mutation endpoints ───────────────────────────────────────
// NOTE: the actual `run` spawn + real run + linkage is NOT asserted here — the
// handler spawns `current_exe delivery run <id>`, which in-process would be the
// TEST binary (libtest recursion). server_api covers every GUARD before the spawn
// (the 409s + three-state); the 202 spawn + real run is verified by the manual
// smoke (real `maestro ui`) and F-127b's delivery_cli run-linkage test.

fn f129_projects(dir: &TempDir) {
    std::fs::create_dir_all(dir.path().join("proj-a")).unwrap();
    std::fs::write(
        dir.path().join(".maestro").join("projects.yaml"),
        "projects:\n  proj-a:\n    path: ./proj-a\n    agent: mock\ndefaults:\n  agent: mock\n  max_parallel: 1\n",
    )
    .unwrap();
}

fn f129_intake_to_spec(id: &str, projects: Vec<&str>, confirmed: bool) {
    let now = "2026-06-08T00:00:00Z".to_string();
    let src = maestro::schema::delivery::DeliveryRef {
        kind: "intake_doc".into(),
        path: None,
        uri: None,
        name: Some("r".into()),
    };
    let spec = maestro::delivery::parse_intake(
        id,
        "## Objective\nX\n## Target Users\nY\n## Acceptance\n- ok\n",
        src,
        now.clone(),
        None,
    );
    maestro::delivery::create(&spec).unwrap();
    maestro::delivery::set_spec(
        id,
        "ship it".into(),
        projects.iter().map(|s| s.to_string()).collect(),
        vec![maestro::schema::delivery::AcceptanceCriterion {
            describe: "ok".into(),
            check: Some("test 1 = 1".into()),
        }],
        now.clone(),
        None,
    )
    .unwrap();
    if confirmed {
        maestro::delivery::confirm_spec(id, Some("t".into()), now).unwrap();
    }
}

fn f129_intake_to_plan(id: &str) {
    f129_intake_to_spec(id, vec!["proj-a"], true);
    maestro::delivery::generate_plan(id, "2026-06-08T00:00:00Z".to_string(), None, false).unwrap();
}

#[tokio::test]
#[serial]
async fn delivery_confirm_spec_ok_and_dup_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d1", vec!["proj-a"], false);
    let r = server()
        .post("/api/deliveries/d1/confirm-spec")
        .json(&json!({"by":"alice"}))
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "spec"); // confirm keeps stage Spec
    assert_eq!(v["pm_accepted_by"], serde_json::Value::Null);
    let body =
        std::fs::read_to_string(dir.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    assert!(body.contains("spec_confirm"), "spec_confirm recorded");
    // A second confirm → 409 (no duplicate audit row).
    assert_eq!(
        server()
            .post("/api/deliveries/d1/confirm-spec")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_confirm_spec_incomplete_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    // an unknown target project → incomplete spec → confirm refused
    f129_intake_to_spec("d1", vec!["ghost"], false);
    let r = server()
        .post("/api/deliveries/d1/confirm-spec")
        .json(&json!({}))
        .await;
    assert_eq!(r.status_code(), 409);
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_plan_ok_not_confirmed_409_and_already_plan_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d-unconf", vec!["proj-a"], false);
    assert_eq!(
        server()
            .post("/api/deliveries/d-unconf/plan")
            .json(&json!({}))
            .await
            .status_code(),
        409,
        "plan before confirm → 409"
    );
    f129_intake_to_spec("d-conf", vec!["proj-a"], true);
    let r = server()
        .post("/api/deliveries/d-conf/plan")
        .json(&json!({}))
        .await;
    r.assert_status_ok();
    assert_eq!(r.json::<serde_json::Value>()["stage"], "plan");
    assert_eq!(
        server()
            .post("/api/deliveries/d-conf/plan")
            .json(&json!({}))
            .await
            .status_code(),
        409,
        "re-plan (already has plan) → 409"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_preflight_refusals_409() {
    // wrong stage (no plan).
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d-spec", vec!["proj-a"], true);
    assert_eq!(
        server()
            .post("/api/deliveries/d-spec/run")
            .json(&json!({}))
            .await
            .status_code(),
        409,
        "run at stage spec → 409"
    );
    // plan_hash drift (tamper the generated plan).
    f129_intake_to_plan("d-drift");
    let p = dir.path().join("plans/delivery-d-drift.yaml");
    let t = std::fs::read_to_string(&p)
        .unwrap()
        .replace("ship it", "tampered objective");
    std::fs::write(&p, t).unwrap();
    assert_eq!(
        server()
            .post("/api/deliveries/d-drift/run")
            .json(&json!({}))
            .await
            .status_code(),
        409,
        "drifted plan → 409"
    );
    // project drift (remove the target project).
    f129_intake_to_plan("d-proj");
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        "projects: {}\ndefaults:\n  agent: mock\n  max_parallel: 1\n",
    )
    .unwrap();
    assert_eq!(
        server()
            .post("/api/deliveries/d-proj/run")
            .json(&json!({}))
            .await
            .status_code(),
        409,
        "project drift → 409"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_duplicate_launch_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_plan("d1");
    // Simulate an in-flight run: a launch marker held by a LIVE pid (this process).
    std::fs::write(
        dir.path().join(".maestro/deliveries/d1/.run-launch"),
        std::process::id().to_string(),
    )
    .unwrap();
    let r = server()
        .post("/api/deliveries/d1/run")
        .json(&json!({}))
        .await;
    assert_eq!(
        r.status_code(),
        409,
        "a 2nd launch while a run is in flight must 409"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_action_bad_id_and_missing_three_state() {
    let _dir = fresh_workspace();
    assert_eq!(
        server()
            .post("/api/deliveries/a%5cb/confirm-spec")
            .json(&json!({}))
            .await
            .status_code(),
        400
    );
    assert_eq!(
        server()
            .post("/api/deliveries/nope/plan")
            .json(&json!({}))
            .await
            .status_code(),
        404
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_action_corrupt_is_500() {
    // A corrupt DELIVERY.json → the action read_for_action three-state → 500 (the
    // same read-first path as the GET detail).
    let dir = fresh_workspace();
    let p = dir.path().join(".maestro").join("deliveries").join("d-bad");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("DELIVERY.json"), "not json {{{").unwrap();
    assert_eq!(
        server()
            .post("/api/deliveries/d-bad/confirm-spec")
            .json(&json!({}))
            .await
            .status_code(),
        500
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_task_cap_409() {
    // A two-project plan (≥2 tasks) with max_total_tasks lowered to 1 → run_preflight
    // task-cap bail → 409 (before any spawn).
    let dir = fresh_workspace();
    std::fs::create_dir_all(dir.path().join("proj-a")).unwrap();
    std::fs::create_dir_all(dir.path().join("proj-b")).unwrap();
    let two = "projects:\n  proj-a:\n    path: ./proj-a\n    agent: mock\n  proj-b:\n    path: ./proj-b\n    agent: mock\ndefaults:\n  agent: mock\n  max_parallel: 1\n";
    std::fs::write(dir.path().join(".maestro/projects.yaml"), two).unwrap();
    f129_intake_to_spec("d1", vec!["proj-a", "proj-b"], true);
    maestro::delivery::generate_plan("d1", "2026-06-08T00:00:00Z".to_string(), None, false)
        .unwrap();
    std::fs::write(
        dir.path().join(".maestro/projects.yaml"),
        format!("{two}  max_total_tasks: 1\n"),
    )
    .unwrap();
    assert_eq!(
        server()
            .post("/api/deliveries/d1/run")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );
    clear();
}

// ── F-130 delivery accept / closeout endpoints ──────────────────────────────
// accept/closeout are SYNC store fns (no detached spawn): accept reads the run
// outcome for the 5-state gate; closeout's write-back is an in-process append. So
// unlike `run`, these CAN be driven end-to-end here — including a real (mock) run
// via the lib `start_run` (which runs the plan directly, never spawning current_exe,
// so there is no libtest recursion). `set_run_state` then forces the outcome.

/// Overwrite a run's RUN_STATE status/verified (clearing pending_gate) so the
/// accept-source classification is exercised without a really-verified run.
fn set_run_state(run_id: &str, status: &str, verified: bool) {
    let ws = std::env::var("MAESTRO_WORKSPACE_ROOT").unwrap();
    let p = std::path::Path::new(&ws).join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("status".into(), json!(status));
    o.insert("verified".into(), json!(verified));
    o.remove("pending_gate");
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
}

fn f130_git_init() {
    let ws = std::env::var("MAESTRO_WORKSPACE_ROOT").unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&ws)
        .status()
        .ok();
}

/// Drive a delivery to Execute in-process (real mock run on proj-a) and return its
/// run_id. Optional `source_uri` records a doc uri on the intake source (the
/// write-back target).
async fn f130_to_execute(id: &str, source_uri: Option<&str>) -> String {
    let now = "2026-06-08T00:00:00Z".to_string();
    let src = maestro::schema::delivery::DeliveryRef {
        kind: "intake_doc".into(),
        path: None,
        uri: source_uri.map(|s| s.to_string()),
        name: Some("r".into()),
    };
    let spec = maestro::delivery::parse_intake(
        id,
        "## Objective\nX\n## Target Users\nY\n## Acceptance\n- ok\n",
        src,
        now.clone(),
        None,
    );
    maestro::delivery::create(&spec).unwrap();
    maestro::delivery::set_spec(
        id,
        "ship it".into(),
        vec!["proj-a".to_string()],
        vec![maestro::schema::delivery::AcceptanceCriterion {
            describe: "ok".into(),
            check: Some("test 1 = 1".into()),
        }],
        now.clone(),
        None,
    )
    .unwrap();
    maestro::delivery::confirm_spec(id, Some("t".into()), now.clone()).unwrap();
    maestro::delivery::generate_plan(id, now.clone(), None, false).unwrap();
    maestro::delivery::start_run(id, now, Some("runner".into()))
        .await
        .unwrap();
    let spec = maestro::delivery::read(id).unwrap().unwrap();
    spec.execute.unwrap().run_id.unwrap()
}

/// Drive to Accept (verdict accepted, verified clean run) for closeout tests.
async fn f130_to_accepted(id: &str, source_uri: Option<&str>) {
    let run_id = f130_to_execute(id, source_uri).await;
    set_run_state(&run_id, "done", true);
    maestro::delivery::accept(
        id,
        maestro::delivery::parse_verdict("accepted").unwrap(),
        "pm".to_string(),
        None,
        vec![],
        false,
        "2026-06-08T00:00:00Z".to_string(),
    )
    .unwrap();
}

#[tokio::test]
#[serial]
async fn delivery_accept_three_state_and_invalid_verdict() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    // bad id (encoded backslash survives normalization) → 400
    assert_eq!(
        server()
            .post("/api/deliveries/a%5cb/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        400
    );
    // missing delivery → 404
    assert_eq!(
        server()
            .post("/api/deliveries/d-nope/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        404
    );
    // corrupt delivery → 500
    write_delivery(&dir, "d-bad", "not json {{{");
    assert_eq!(
        server()
            .post("/api/deliveries/d-bad/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        500
    );
    // invalid verdict on a valid delivery → 400 (before the store stage check)
    f129_intake_to_spec("d-spec", vec!["proj-a"], false);
    assert_eq!(
        server()
            .post("/api/deliveries/d-spec/accept")
            .json(&json!({"verdict":"bogus"}))
            .await
            .status_code(),
        400
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_accept_wrong_stage_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    // stage spec (no run yet) — accept requires execute
    f129_intake_to_spec("d1", vec!["proj-a"], false);
    assert_eq!(
        server()
            .post("/api/deliveries/d1/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_accept_failed_run_verdict_rules() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "failed", false);
    // a failed run: accepted is refused, only changes_requested/rejected allowed
    assert_eq!(
        server()
            .post("/api/deliveries/d1/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        409
    );
    let r = server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"rejected"}))
        .await;
    r.assert_status_ok();
    assert_eq!(r.json::<serde_json::Value>()["stage"], "rejected");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_accept_unverified_requires_flag_and_debt() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "done", false); // finished but acceptance did not pass
                                           // accepted without the override flag/debt → refused
    assert_eq!(
        server()
            .post("/api/deliveries/d1/accept")
            .json(&json!({"verdict":"accepted"}))
            .await
            .status_code(),
        409
    );
    // partial without debt → refused
    assert_eq!(
        server()
            .post("/api/deliveries/d1/accept")
            .json(&json!({"verdict":"partial"}))
            .await
            .status_code(),
        409
    );
    // accepted WITH the override flag + non-empty debt → ok, by defaults to web-ui
    let r = server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"accepted","accept_failed_with_debt":true,"debt":["flaky perf"]}))
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "accept");
    assert_eq!(v["pm_accepted_by"], "web-ui");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_accept_clean_run_records_web_ui() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "done", true); // verified clean pass
    let r = server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"accepted"})) // no `by` → web-ui
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "accept");
    assert_eq!(v["accept_verdict"], "accepted");
    assert_eq!(v["pm_accepted_by"], "web-ui");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_accept_changes_requested_lands_stage() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "done", true);
    let r = server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"changes_requested"}))
        .await;
    r.assert_status_ok();
    assert_eq!(r.json::<serde_json::Value>()["stage"], "changes_requested");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_wrong_stage_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    // stage plan (not accept) — closeout requires accept
    f129_intake_to_plan("d1");
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["evidence/out.txt"]}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_empty_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", None).await;
    // no evidence-bearing field → empty closeout refused
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"writeback":false}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_unsafe_evidence_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", None).await;
    // absolute path → rejected by the store's run-local validation
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["/etc/passwd"]}))
            .await
            .status_code(),
        409
    );
    // a `file:` string must NOT masquerade as a relative path — also rejected
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["file:///etc/passwd"]}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_ok_and_re_closeout_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", None).await;
    let r = server()
        .post("/api/deliveries/d1/closeout")
        .json(&json!({"commits":["abc123"],"evidence":["evidence/out.txt"]}))
        .await;
    r.assert_status_ok();
    assert_eq!(r.json::<serde_json::Value>()["stage"], "closeout");
    // re-closeout → 409 (stage already closeout)
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["evidence/again.txt"]}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_writeback_requires_doc_uri() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    // accepted but NO intake doc uri → writeback cannot locate a target
    f130_to_accepted("d1", None).await;
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["evidence/out.txt"],"writeback":true}))
            .await
            .status_code(),
        409
    );
    // stage must NOT have advanced (no half-written closeout, no intent)
    let v: serde_json::Value = server().get("/api/deliveries/d1").await.json();
    assert_eq!(v["stage"], "accept");
    assert_eq!(v["writeback_status"], serde_json::Value::Null);
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_writeback_emits_intent() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", Some("https://example.feishu.cn/docx/abc")).await;
    let r = server()
        .post("/api/deliveries/d1/closeout")
        .json(&json!({"evidence":["evidence/out.txt"],"writeback":true}))
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "closeout");
    assert_eq!(v["writeback_status"], "intent_emitted");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_rejected_delivery_refused() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "failed", false);
    // land at rejected
    server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"rejected"}))
        .await
        .assert_status_ok();
    // closeout on a rejected delivery → 409 (stage rejected, not accept)
    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["evidence/out.txt"]}))
            .await
            .status_code(),
        409
    );
    clear();
}

// ── F-136a1 RuntimeProfile projection ───────────────────────────────────────

#[tokio::test]
#[serial]
async fn delivery_view_carries_runtime_profile_summary() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_execute("d1", None).await;
    let v: serde_json::Value = server().get("/api/deliveries/d1").await.json();
    let summary = &v["runtime_profile_summary"];
    assert!(!summary.is_null(), "linked run → summary present: {v}");
    let total = summary["task_total"].as_u64().unwrap();
    assert!(total >= 1, "at least one task");
    assert!(summary["worst"].is_string(), "worst label present");
    let counts = summary["counts"].as_array().unwrap();
    assert!(!counts.is_empty(), "per-profile breakdown present");
    // pin 5: counts keep the full breakdown — they sum back to the task total.
    let sum: u64 = counts.iter().map(|c| c["count"].as_u64().unwrap()).sum();
    assert_eq!(sum, total, "counts sum to task_total");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_view_no_run_has_no_runtime_profile_summary() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d1", vec!["proj-a"], false); // stage spec, never executed
    let v: serde_json::Value = server().get("/api/deliveries/d1").await.json();
    assert!(
        v["runtime_profile_summary"].is_null(),
        "no linked run → no summary (never fabricated): {v}"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_view_linked_run_corrupt_is_not_silent_null() {
    // F-136a1 B1: a delivery WITH execute.run_id whose RUN_STATE is corrupt is
    // "runtime profile unavailable" — NOT "no run". Detail → 500; list → corrupt stub
    // for that delivery (never a silent null, never a whole-list 500).
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::write(&rs, "not json {{{").unwrap();

    // detail → 500
    assert_eq!(
        server().get("/api/deliveries/d1").await.status_code(),
        500,
        "linked run corrupt → 500, never silent null"
    );

    // list → still 200, but d1 is a corrupt stub
    let r = server().get("/api/deliveries").await;
    assert_eq!(
        r.status_code(),
        200,
        "one bad delivery does not 500 the list"
    );
    let entries: serde_json::Value = r.json();
    let d1 = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["delivery_id"] == "d1")
        .expect("d1 present");
    assert_eq!(
        d1["status"], "corrupt",
        "linked-run-unavailable → corrupt stub"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_view_linked_run_missing_is_not_silent_null() {
    // The missing-RUN_STATE variant of B1 → detail 500 (not null).
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::remove_file(&rs).unwrap();
    assert_eq!(
        server().get("/api/deliveries/d1").await.status_code(),
        500,
        "linked run RUN_STATE missing → 500, never silent null"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_closeout_corrupt_linked_run_500_before_mutation() {
    // F-136a1 B2: a view-returning mutation must PREFLIGHT projectability — closeout
    // (which doesn't read the run) must NOT write stage=Closeout and THEN 500 on the
    // post-mutation projection. Expect 500 AND the delivery unchanged (accept / no
    // closeout), proving the store mutation never ran.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", None).await; // stage=accept, execute.run_id present
    let spec = maestro::delivery::read("d1").unwrap().unwrap();
    let run_id = spec.execute.unwrap().run_id.unwrap();
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::write(&rs, "not json {{{").unwrap(); // corrupt AFTER accept

    assert_eq!(
        server()
            .post("/api/deliveries/d1/closeout")
            .json(&json!({"evidence":["evidence/out.txt"]}))
            .await
            .status_code(),
        500,
        "closeout preflight on a corrupt linked run → 500"
    );

    // Read the record straight from disk (a GET would also 500 by B1) — it must NOT
    // have advanced: no side-effect-then-fail.
    let body =
        std::fs::read_to_string(dir.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    let d: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(d["stage"], "accept", "stage must not advance to closeout");
    assert!(d["closeout"].is_null(), "no closeout was written");
    clear();
}

// ── F-136a2 read-only provider enforcement matrix ───────────────────────────

#[tokio::test]
#[serial]
async fn provider_profiles_returns_enforcement_matrix() {
    let _dir = fresh_workspace();
    let r = server().get("/api/providers/profiles").await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 4, "shell/codex/cursor/mock");
    let codex = arr
        .iter()
        .find(|p| p["provider_id"] == "codex")
        .expect("codex present");
    // honest matrix: codex hard-enforces network; git_write is advisory (soft).
    assert_eq!(codex["network"], "hard");
    assert_eq!(codex["git_write"], "soft");
    let shell = arr.iter().find(|p| p["provider_id"] == "shell").unwrap();
    assert_eq!(shell["shell"], "hard");
    assert_eq!(shell["git_write"], "soft");
    clear();
}

// ── F-131 delivery run-status (inline live status) ──────────────────────────

#[tokio::test]
#[serial]
async fn delivery_run_status_linked_run() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_execute("d1", None).await; // execute.run_id present
    let v: serde_json::Value = server().get("/api/deliveries/d1/run-status").await.json();
    let run = &v["run"];
    assert!(!run.is_null(), "linked run resolved: {v}");
    assert_eq!(run["linked"], true);
    assert!(run["status"].is_string());
    assert!(run["progress"]["total"].as_u64().unwrap() >= 1);
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_status_inflight_unlinked_run() {
    // A detached run mid-flight: the RUN_STATE has delivery_id but start_run hasn't
    // written execute.run_id back yet. The resolver must find it via the back-ref.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "running", false);
    // strip the delivery's execute linkage to simulate the not-yet-linked state
    let p = dir.path().join(".maestro/deliveries/d1/DELIVERY.json");
    let mut dv: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let o = dv.as_object_mut().unwrap();
    o.remove("execute");
    o.insert("stage".into(), json!("plan"));
    std::fs::write(&p, serde_json::to_string(&dv).unwrap()).unwrap();

    let v: serde_json::Value = server().get("/api/deliveries/d1/run-status").await.json();
    let run = &v["run"];
    assert!(
        !run.is_null(),
        "in-flight run found via delivery_id back-ref: {v}"
    );
    assert_eq!(run["linked"], false);
    assert_eq!(run["status"], "running");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_status_no_run_is_null() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d1", vec!["proj-a"], false); // stage spec, never executed
    let v: serde_json::Value = server().get("/api/deliveries/d1/run-status").await.json();
    assert!(
        v["run"].is_null(),
        "no run → null (idle), not a fabricated status: {v}"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_status_corrupt_linked_run_500() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::write(&rs, "not json {{{").unwrap();
    assert_eq!(
        server()
            .get("/api/deliveries/d1/run-status")
            .await
            .status_code(),
        500,
        "corrupt linked run → 500, never silently shown as idle"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_status_bad_id_400_and_missing_404() {
    let _dir = fresh_workspace();
    assert_eq!(
        server()
            .get("/api/deliveries/a%5cb/run-status")
            .await
            .status_code(),
        400
    );
    assert_eq!(
        server()
            .get("/api/deliveries/d-nope/run-status")
            .await
            .status_code(),
        404
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_run_status_orphan_nonrunning_back_ref_is_null() {
    // F-131 B1: a back-ref run that is NOT running (orphan: done/failed/cancelled) must
    // NOT masquerade as in-flight — the resolver returns run:null. The back-ref path is
    // only for the "detached run actively running, execute not written back" window.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "done", true); // matched delivery_id but DONE, not running
                                          // strip the delivery's execute linkage → only the back-ref remains
    let p = dir.path().join(".maestro/deliveries/d1/DELIVERY.json");
    let mut dv: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let o = dv.as_object_mut().unwrap();
    o.remove("execute");
    o.insert("stage".into(), json!("plan"));
    std::fs::write(&p, serde_json::to_string(&dv).unwrap()).unwrap();

    let v: serde_json::Value = server().get("/api/deliveries/d1/run-status").await.json();
    assert!(
        v["run"].is_null(),
        "a non-running back-ref orphan is null, not in-flight: {v}"
    );
    clear();
}

// ── F-132 delivery audit timeline ───────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delivery_timeline_full_lifecycle_ascending_refs_first() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    f130_to_accepted("d1", None).await; // intake→spec→confirm→plan→run→accept(accepted)
    maestro::delivery::closeout(
        "d1",
        vec!["abc123".into()],
        vec![],
        vec![],
        vec![],
        maestro::delivery::parse_evidence_refs(&["evidence/out.txt".to_string()]),
        false,
        "2026-06-09T00:00:00Z".to_string(),
        Some("alice".into()),
    )
    .unwrap();

    let v: serde_json::Value = server().get("/api/deliveries/d1/timeline").await.json();
    let events = v["events"].as_array().unwrap();
    assert!(
        events.len() >= 5,
        "full lifecycle has many events: {}",
        events.len()
    );

    // ascending by `at`
    let ats: Vec<&str> = events.iter().map(|e| e["at"].as_str().unwrap()).collect();
    let mut sorted = ats.clone();
    sorted.sort();
    assert_eq!(ats, sorted, "events ascending by at");

    let stages: Vec<&str> = events
        .iter()
        .map(|e| e["stage"].as_str().unwrap())
        .collect();
    for s in ["plan", "execute", "accept", "closeout"] {
        assert!(stages.contains(&s), "timeline has a {s} event: {stages:?}");
    }

    // refs-first: no RUN_STATE task list / monitor blob smuggled into the timeline.
    let body = serde_json::to_string(&v).unwrap();
    assert!(
        !body.contains("\"tasks\""),
        "refs-first: no RUN_STATE in timeline: {body}"
    );

    // the plan event carries a refs-first plan ref (path), the execute event a run id.
    let plan_ev = events.iter().find(|e| e["stage"] == "plan").unwrap();
    let plan_ref = plan_ev["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "plan")
        .unwrap();
    assert!(plan_ref["value"].as_str().unwrap().contains("plans/"));
    let exec_ev = events.iter().find(|e| e["stage"] == "execute").unwrap();
    assert!(exec_ev["refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "run"));
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_timeline_same_timestamp_keeps_append_order() {
    // 大力 scrutiny: accept on a failed run pushes Accept@T then Rejected@T (same `at`).
    // The (at, index) sort must keep accept BEFORE rejected.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f130_to_execute("d1", None).await;
    set_run_state(&run_id, "failed", false);
    server()
        .post("/api/deliveries/d1/accept")
        .json(&json!({"verdict":"rejected"}))
        .await
        .assert_status_ok();

    let v: serde_json::Value = server().get("/api/deliveries/d1/timeline").await.json();
    let events = v["events"].as_array().unwrap();
    let ai = events.iter().position(|e| e["stage"] == "accept").unwrap();
    let ri = events
        .iter()
        .position(|e| e["stage"] == "rejected")
        .unwrap();
    assert!(ai < ri, "accept must precede rejected (append order)");
    assert_eq!(
        events[ai]["at"], events[ri]["at"],
        "the two rows share a timestamp"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_timeline_audit_node_inconsistency_is_500() {
    // 大力 scrutiny: an audit Plan transition with NO plan ref is an inconsistent record
    // → 500, never a fabricated text-only 200 events.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f129_intake_to_spec("d1", vec!["proj-a"], false); // stage spec, no plan
    let p = dir.path().join(".maestro/deliveries/d1/DELIVERY.json");
    let mut dv: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    dv["audit"].as_array_mut().unwrap().push(json!({
        "stage": "plan", "at": "2026-06-08T05:00:00Z", "by": "forger", "reason": "forged"
    }));
    std::fs::write(&p, serde_json::to_string(&dv).unwrap()).unwrap();

    assert_eq!(
        server()
            .get("/api/deliveries/d1/timeline")
            .await
            .status_code(),
        500,
        "audit says Plan but plan ref missing → 500, not 200 events"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_timeline_three_state() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d-bad", "not json {{{");
    assert_eq!(
        server()
            .get("/api/deliveries/d-bad/timeline")
            .await
            .status_code(),
        500
    );
    assert_eq!(
        server()
            .get("/api/deliveries/a%5cb/timeline")
            .await
            .status_code(),
        400
    );
    assert_eq!(
        server()
            .get("/api/deliveries/d-nope/timeline")
            .await
            .status_code(),
        404
    );
    clear();
}

// ── F-133 delivery reopen loop ──────────────────────────────────────────────

/// Drive a delivery to `changes_requested` (failed run + accept changes_requested).
async fn f133_to_changes_requested(id: &str) -> String {
    let run_id = f130_to_execute(id, None).await;
    set_run_state(&run_id, "failed", false);
    server()
        .post(&format!("/api/deliveries/{id}/accept"))
        .json(&json!({"verdict":"changes_requested"}))
        .await
        .assert_status_ok();
    run_id
}

#[tokio::test]
#[serial]
async fn delivery_reopen_resets_live_and_archives_prior_round() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f133_to_changes_requested("d1").await;

    let r = server()
        .post("/api/deliveries/d1/reopen")
        .json(&json!({"by":"pm","reason":"tweak the acceptance"}))
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "spec", "reopen lands at spec");
    assert_eq!(v["round"], 2);
    assert_eq!(v["superseded_count"], 1);

    // on disk: live fields reset, prior round archived with the OLD refs.
    let body =
        std::fs::read_to_string(dir.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    let d: serde_json::Value = serde_json::from_str(&body).unwrap();
    for f in [
        "plan",
        "execute",
        "accept",
        "pm_accept",
        "closeout",
        "spec_confirm",
    ] {
        assert!(d[f].is_null(), "live {f} reset after reopen: {body}");
    }
    let sr = &d["superseded_rounds"][0];
    assert_eq!(sr["round"], 1);
    assert_eq!(
        sr["execute"]["run_id"], run_id,
        "old run_id preserved in archive"
    );
    assert!(sr["plan"]["plan_path"].as_str().unwrap().contains("plans/"));
    assert_eq!(sr["accept"]["verdict"], "changes_requested");
    // the reopen audit row is readable (F-132 timeline).
    let tl: serde_json::Value = server().get("/api/deliveries/d1/timeline").await.json();
    assert!(tl["events"].as_array().unwrap().iter().any(|e| e["reason"]
        .as_str()
        .unwrap_or("")
        .contains("reopened for rework")));
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_reopen_rework_loop_new_run_id_old_preserved() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let old_run = f133_to_changes_requested("d1").await;
    server()
        .post("/api/deliveries/d1/reopen")
        .json(&json!({}))
        .await
        .assert_status_ok();

    // the new round MUST re-confirm + re-plan + re-run (no auto-crossing thresholds).
    server()
        .post("/api/deliveries/d1/confirm-spec")
        .json(&json!({}))
        .await
        .assert_status_ok();
    server()
        .post("/api/deliveries/d1/plan")
        .json(&json!({}))
        .await
        .assert_status_ok();
    maestro::delivery::start_run(
        "d1",
        "2026-06-10T00:00:00Z".to_string(),
        Some("runner".into()),
    )
    .await
    .unwrap();

    let d = maestro::delivery::read("d1").unwrap().unwrap();
    let new_run = d.execute.unwrap().run_id.unwrap();
    assert_ne!(new_run, old_run, "the new round produced a NEW run_id");
    assert_eq!(
        d.superseded_rounds[0]
            .execute
            .as_ref()
            .unwrap()
            .run_id
            .as_deref(),
        Some(old_run.as_str()),
        "the old run_id is still in the archive, un-polluted"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_reopen_wrong_stage_and_twice_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();

    // spec stage → 409
    f129_intake_to_spec("d-spec", vec!["proj-a"], false);
    assert_eq!(
        server()
            .post("/api/deliveries/d-spec/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );
    // accept stage → 409
    f130_to_accepted("d-acc", None).await;
    assert_eq!(
        server()
            .post("/api/deliveries/d-acc/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );
    // rejected stage → 409
    let run_id = f130_to_execute("d-rej", None).await;
    set_run_state(&run_id, "failed", false);
    server()
        .post("/api/deliveries/d-rej/accept")
        .json(&json!({"verdict":"rejected"}))
        .await
        .assert_status_ok();
    assert_eq!(
        server()
            .post("/api/deliveries/d-rej/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );

    // reopen twice: the first lands at spec, the second is wrong-stage → 409.
    f133_to_changes_requested("d1").await;
    server()
        .post("/api/deliveries/d1/reopen")
        .json(&json!({}))
        .await
        .assert_status_ok();
    assert_eq!(
        server()
            .post("/api/deliveries/d1/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        409
    );

    // three-state
    write_delivery(&dir, "d-bad", "not json {{{");
    assert_eq!(
        server()
            .post("/api/deliveries/d-bad/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        500
    );
    assert_eq!(
        server()
            .post("/api/deliveries/a%5cb/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        400
    );
    assert_eq!(
        server()
            .post("/api/deliveries/d-nope/reopen")
            .json(&json!({}))
            .await
            .status_code(),
        404
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_reopen_timeline_round_aware_shows_both_runs() {
    // After a rework loop the timeline must NOT 500 (the old Execute row's node is in the
    // archive now) — and the round-1 execute event shows the OLD run via the archive while
    // the round-2 event shows the NEW (live) run.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let old_run = f133_to_changes_requested("d1").await;
    server()
        .post("/api/deliveries/d1/reopen")
        .json(&json!({}))
        .await
        .assert_status_ok();
    server()
        .post("/api/deliveries/d1/confirm-spec")
        .json(&json!({}))
        .await
        .assert_status_ok();
    server()
        .post("/api/deliveries/d1/plan")
        .json(&json!({}))
        .await
        .assert_status_ok();
    maestro::delivery::start_run(
        "d1",
        "2026-06-10T00:00:00Z".to_string(),
        Some("runner".into()),
    )
    .await
    .unwrap();
    let new_run = maestro::delivery::read("d1")
        .unwrap()
        .unwrap()
        .execute
        .unwrap()
        .run_id
        .unwrap();

    let r = server().get("/api/deliveries/d1/timeline").await;
    r.assert_status_ok(); // round-aware: no false inconsistency 500
    let tl: serde_json::Value = r.json();
    let run_refs: Vec<String> = tl["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["stage"] == "execute")
        .flat_map(|e| e["refs"].as_array().unwrap().iter())
        .filter(|rf| rf["kind"] == "run")
        .filter_map(|rf| rf["value"].as_str().map(str::to_string))
        .collect();
    assert!(
        run_refs.contains(&old_run),
        "round-1 execute shows the archived OLD run: {run_refs:?}"
    );
    assert!(
        run_refs.contains(&new_run),
        "round-2 execute shows the live NEW run: {run_refs:?}"
    );
    clear();
}

// ── F-134 Feishu write-back receipt ─────────────────────────────────────────

const F134_DOC: &str = "https://example.feishu.cn/docx/abc";

/// Drive to a closeout with a write-back intent emitted; return the idempotency_key.
async fn f134_intent_emitted(id: &str) -> String {
    f130_to_accepted(id, Some(F134_DOC)).await;
    maestro::delivery::closeout(
        id,
        vec!["abc123".into()],
        vec![],
        vec![],
        vec![],
        maestro::delivery::parse_evidence_refs(&["evidence/out.txt".to_string()]),
        true, // writeback
        "2026-06-09T00:00:00Z".to_string(),
        Some("alice".into()),
    )
    .unwrap();
    maestro::delivery::read(id)
        .unwrap()
        .unwrap()
        .closeout
        .unwrap()
        .writeback
        .unwrap()
        .idempotency_key
        .unwrap()
}

#[tokio::test]
#[serial]
async fn delivery_writeback_intent_emits_dual_key() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let key = f134_intent_emitted("d1").await;
    assert!(!key.is_empty());
    // the OutboundReply in the run's queue carries the SAME key + the delivery_id.
    let spec = maestro::delivery::read("d1").unwrap().unwrap();
    let run_id = spec.execute.unwrap().run_id.unwrap();
    let ndjson = std::fs::read_to_string(
        dir.path()
            .join(format!(".maestro/runs/{run_id}/outbound_replies.ndjson")),
    )
    .unwrap();
    let line = ndjson
        .lines()
        .find(|l| l.contains("delivery.closeout"))
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        reply["idempotency_key"], key,
        "OutboundReply carries the same key"
    );
    assert_eq!(
        reply["delivery_id"], "d1",
        "OutboundReply carries the delivery_id"
    );
    // never copies the doc body — refs only
    assert!(!ndjson.contains("\"feishu_body\""));
    let v: serde_json::Value = server().get("/api/deliveries/d1").await.json();
    assert_eq!(v["writeback_status"], "intent_emitted");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_writeback_posted_and_failed_receipts() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let key = f134_intent_emitted("d1").await;
    let r = server()
        .post("/api/deliveries/d1/writeback-receipt")
        .json(&json!({"idempotency_key": key, "status":"posted", "message_ref":"om_msg123", "doc_revision":"rev-7"}))
        .await;
    r.assert_status_ok();
    let v: serde_json::Value = r.json();
    assert_eq!(v["writeback_status"], "posted");
    assert_eq!(v["writeback_receipt"]["message_ref"], "om_msg123");
    assert_eq!(v["writeback_receipt"]["doc_revision"], "rev-7");

    // a separate failed delivery
    let key2 = f134_intent_emitted("d2").await;
    let v2: serde_json::Value = server()
        .post("/api/deliveries/d2/writeback-receipt")
        .json(&json!({"idempotency_key": key2, "status":"failed", "error":"feishu 403 forbidden"}))
        .await
        .json();
    assert_eq!(v2["writeback_status"], "failed");
    assert_eq!(v2["writeback_receipt"]["error"], "feishu 403 forbidden");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_writeback_reconcile() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let key = f134_intent_emitted("d1").await;
    let post =
        |k: &str, m: &str| json!({"idempotency_key": k, "status":"posted", "message_ref": m});
    // post once
    server()
        .post("/api/deliveries/d1/writeback-receipt")
        .json(&post(&key, "om_A"))
        .await
        .assert_status_ok();
    // same posted re-report → 200 no-op
    server()
        .post("/api/deliveries/d1/writeback-receipt")
        .json(&post(&key, "om_A"))
        .await
        .assert_status_ok();
    // posted is immutable: a DIFFERENT message_ref → 409
    assert_eq!(
        server()
            .post("/api/deliveries/d1/writeback-receipt")
            .json(&post(&key, "om_B"))
            .await
            .status_code(),
        409
    );

    // failed → posted is allowed (a retry that finally succeeded)
    let key2 = f134_intent_emitted("d2").await;
    server()
        .post("/api/deliveries/d2/writeback-receipt")
        .json(&json!({"idempotency_key": key2, "status":"failed", "error":"transient"}))
        .await
        .assert_status_ok();
    let v: serde_json::Value = server()
        .post("/api/deliveries/d2/writeback-receipt")
        .json(&json!({"idempotency_key": key2, "status":"posted", "message_ref":"om_retry"}))
        .await
        .json();
    assert_eq!(v["writeback_status"], "posted");
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_writeback_receipt_validation_409() {
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let key = f134_intent_emitted("d1").await;
    let path = "/api/deliveries/d1/writeback-receipt";
    // key mismatch → 409
    assert_eq!(
        server()
            .post(path)
            .json(
                &json!({"idempotency_key":"fnv1a64:deadbeef","status":"posted","message_ref":"x"})
            )
            .await
            .status_code(),
        409
    );
    // posted without any durable ref → 409
    assert_eq!(
        server()
            .post(path)
            .json(&json!({"idempotency_key": key, "status":"posted"}))
            .await
            .status_code(),
        409
    );
    // failed without error → 409
    assert_eq!(
        server()
            .post(path)
            .json(&json!({"idempotency_key": key, "status":"failed"}))
            .await
            .status_code(),
        409
    );
    // invalid status → 400
    assert_eq!(
        server()
            .post(path)
            .json(&json!({"idempotency_key": key, "status":"posting"}))
            .await
            .status_code(),
        400
    );
    // no write-back intent (a delivery at accept, no closeout) → 409
    f130_to_accepted("d-noc", None).await;
    assert_eq!(
        server()
            .post("/api/deliveries/d-noc/writeback-receipt")
            .json(&json!({"idempotency_key":"k","status":"posted","message_ref":"x"}))
            .await
            .status_code(),
        409
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_writeback_receipt_three_state() {
    let dir = fresh_workspace();
    write_delivery(&dir, "d-bad", "not json {{{");
    let body = json!({"idempotency_key":"k","status":"posted","message_ref":"x"});
    assert_eq!(
        server()
            .post("/api/deliveries/d-bad/writeback-receipt")
            .json(&body)
            .await
            .status_code(),
        500
    );
    assert_eq!(
        server()
            .post("/api/deliveries/a%5cb/writeback-receipt")
            .json(&body)
            .await
            .status_code(),
        400
    );
    assert_eq!(
        server()
            .post("/api/deliveries/d-nope/writeback-receipt")
            .json(&body)
            .await
            .status_code(),
        404
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_writeback_receipt_corrupt_linked_run_500_before_mutation() {
    // F-134 B1: a corrupt linked RUN_STATE must 500 BEFORE recording the receipt — the
    // drainer callback must not see a 500 while the receipt was already persisted.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let key = f134_intent_emitted("d1").await;
    let spec = maestro::delivery::read("d1").unwrap().unwrap();
    let run_id = spec.execute.unwrap().run_id.unwrap();
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::write(&rs, "not json {{{").unwrap(); // corrupt the linked run

    assert_eq!(
        server()
            .post("/api/deliveries/d1/writeback-receipt")
            .json(&json!({"idempotency_key": key, "status":"posted", "message_ref":"om_x"}))
            .await
            .status_code(),
        500,
        "preflight 500 before recording the receipt"
    );

    // read the record straight from disk (a GET would also 500 by B1) — the receipt must
    // NOT have been written: still intent_emitted, no receipt.
    let body =
        std::fs::read_to_string(dir.path().join(".maestro/deliveries/d1/DELIVERY.json")).unwrap();
    let d: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(d["closeout"]["writeback"]["status"], "intent_emitted");
    assert!(
        d["closeout"]["writeback"]["receipt"].is_null(),
        "no receipt written"
    );
    clear();
}

#[tokio::test]
#[serial]
async fn delivery_reopen_with_corrupt_old_run_succeeds_archiving_ref() {
    // reopen is EXEMPT from the B1 preflight by design: it clears execute (into the
    // archive), so the post-mutation projection loads no run. A corrupt OLD run must NOT
    // block reopen — reopen only keeps the run_id ref.
    let dir = fresh_workspace();
    f129_projects(&dir);
    f130_git_init();
    let run_id = f133_to_changes_requested("d1").await;
    let rs = dir
        .path()
        .join(format!(".maestro/runs/{run_id}/RUN_STATE.json"));
    std::fs::write(&rs, "not json {{{").unwrap(); // corrupt the (old) linked run

    let r = server()
        .post("/api/deliveries/d1/reopen")
        .json(&json!({}))
        .await;
    r.assert_status_ok(); // reopen succeeds even though the old run is corrupt
    let v: serde_json::Value = r.json();
    assert_eq!(v["stage"], "spec");
    // the corrupt run's ref is preserved in the archive (the ref, not the state).
    let d = maestro::delivery::read("d1").unwrap().unwrap();
    assert_eq!(
        d.superseded_rounds[0]
            .execute
            .as_ref()
            .unwrap()
            .run_id
            .as_deref(),
        Some(run_id.as_str())
    );
    clear();
}
