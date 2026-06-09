//! F-119 Step 3: the privacy-safe provider-session reuse signature.
//!
//! Hashes the local inputs that, if they change, mean a provider session created
//! under the old context is no longer the right one to resume. Everything content-
//! bearing is hashed (workspace identity, project registry, memory topics, skill
//! index, prompt-assembly contract version) — NEVER a raw path / prompt / skill
//! body / `projects.yaml` / memory content / env. `status_snippet` and current run
//! state are deliberately EXCLUDED: they change every turn and are resent as fresh
//! context, so including them would force a fresh provider session on every tick.

use crate::chat::stream::{list_memory_topics_safe, load_projects_safe};
use crate::file_guard::stable_hash_bytes;
use crate::paths;
use crate::schema::session_control::{SessionMode, SessionReuseSignature};
use crate::skills::SkillScope;

/// Bump when the prompt-assembly contract changes (section order, prelude-vs-recap
/// shape, mode prefix) so a template change forces a safe provider-session reopen.
const PROMPT_CONTRACT_VERSION: &str = "f119.prompt_contract.v1";

/// Compute the reuse signature for the current local context. `provider` and
/// `model` are the EFFECTIVE values (after resolution), so the signature tracks the
/// session that will actually be used. `now` is caller-supplied RFC3339.
pub fn compute_session_signature(
    provider: &str,
    model: Option<&str>,
    mode: SessionMode,
    now: &str,
) -> SessionReuseSignature {
    let workspace_hash = paths::workspace_root()
        .map(|p| stable_hash_bytes(p.display().to_string().as_bytes()))
        .unwrap_or_else(|_| stable_hash_bytes(b""));
    let prompt_contract_hash = stable_hash_bytes(PROMPT_CONTRACT_VERSION.as_bytes());
    let project_registry_hash = stable_hash_bytes(project_registry_canonical().as_bytes());
    let memory_topic_hash = stable_hash_bytes(memory_topics_canonical().as_bytes());
    let skill_index_hash = stable_hash_bytes(skill_index_canonical().as_bytes());

    let mode_str = match mode {
        SessionMode::Plan => "plan",
        SessionMode::Exec => "exec",
    };
    let payload = format!(
        "{provider}\n{}\n{mode_str}\n{workspace_hash}\n{prompt_contract_hash}\n{project_registry_hash}\n{memory_topic_hash}\n{skill_index_hash}",
        model.unwrap_or(""),
    );
    SessionReuseSignature {
        hash: stable_hash_bytes(payload.as_bytes()),
        provider: provider.to_string(),
        model: model.map(str::to_string),
        mode,
        workspace_hash,
        prompt_contract_hash,
        project_registry_hash,
        memory_topic_hash,
        skill_index_hash,
        created_at: now.to_string(),
    }
}

/// Canonical, path-free project-registry rendering: sorted project names with their
/// type / stack / dependencies / memory scopes. Raw `path` is intentionally omitted.
fn project_registry_canonical() -> String {
    let cfg = load_projects_safe();
    let mut lines: Vec<String> = cfg
        .projects
        .iter()
        .map(|(name, p)| {
            let mut stack = p.stack.clone();
            stack.sort();
            let mut deps = p.dependencies.clone();
            deps.sort();
            let mut scope = p.memory_scope.clone();
            scope.sort();
            format!(
                "{name}|{}|{}|{}|{}",
                p.r#type.clone().unwrap_or_default(),
                stack.join(","),
                deps.join(","),
                scope.join(","),
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

fn memory_topics_canonical() -> String {
    let mut topics = list_memory_topics_safe();
    topics.sort();
    topics.join("\n")
}

/// Canonical skill index: sorted `scope/name|trigger|description-hash`. The system
/// prelude shows each skill's description, so a description edit MUST drift the
/// signature — but we include only a HASH of it, never the raw description text.
fn skill_index_canonical() -> String {
    let mut lines: Vec<String> = crate::skills::index_all()
        .iter()
        .map(|s| {
            let scope = match &s.scope {
                SkillScope::Global => "_global".to_string(),
                SkillScope::Project(p) => format!("project/{p}"),
            };
            let description_hash =
                stable_hash_bytes(s.description.as_deref().unwrap_or("").as_bytes());
            format!(
                "{scope}/{}|{}|{}",
                s.name,
                s.trigger.clone().unwrap_or_default(),
                description_hash,
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
        }
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        dir
    }
    fn clear() {
        unsafe {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    #[test]
    #[serial]
    fn deterministic_and_drifts_on_provider_or_model() {
        let _w = workspace();
        let now = "2026-06-05T00:00:00Z";
        let a = compute_session_signature("cursor", Some("m1"), SessionMode::Exec, now);
        let b = compute_session_signature("cursor", Some("m1"), SessionMode::Exec, now);
        assert_eq!(a.hash, b.hash, "same inputs → same hash");

        // provider / model / mode each drift the hash
        assert_ne!(
            a.hash,
            compute_session_signature("codex", Some("m1"), SessionMode::Exec, now).hash
        );
        assert_ne!(
            a.hash,
            compute_session_signature("cursor", Some("m2"), SessionMode::Exec, now).hash
        );
        assert_ne!(
            a.hash,
            compute_session_signature("cursor", Some("m1"), SessionMode::Plan, now).hash
        );
        clear();
    }

    #[test]
    #[serial]
    fn drifts_when_project_registry_changes() {
        let dir = workspace();
        let now = "2026-06-05T00:00:00Z";
        let before = compute_session_signature("cursor", None, SessionMode::Exec, now);
        std::fs::write(
            dir.path().join(".maestro/projects.yaml"),
            "version: 1\ndefaults:\n  agent: codex\nprojects:\n  billing-service:\n    path: .\n",
        )
        .unwrap();
        let after = compute_session_signature("cursor", None, SessionMode::Exec, now);
        assert_ne!(before.project_registry_hash, after.project_registry_hash);
        assert_ne!(before.hash, after.hash);
        clear();
    }

    #[test]
    #[serial]
    fn signature_json_leaks_no_path_or_body() {
        // a workspace whose path would be sensitive — the signature stores only hashes.
        let dir = workspace();
        std::fs::write(
            dir.path().join(".maestro/projects.yaml"),
            "version: 1\ndefaults:\n  agent: codex\nprojects:\n  web-frontend:\n    path: /secret/abs/path\n",
        )
        .unwrap();
        let sig = compute_session_signature(
            "cursor",
            Some("m1"),
            SessionMode::Exec,
            "2026-06-05T00:00:00Z",
        );
        let json = serde_json::to_string(&sig).unwrap();
        assert!(
            !json.contains("/secret/abs/path"),
            "raw project path leaked: {json}"
        );
        assert!(
            !json.contains(&dir.path().display().to_string()),
            "workspace path leaked"
        );
        clear();
    }

    #[test]
    #[serial]
    fn drifts_when_skill_description_changes_without_leaking_it() {
        // N3: the prelude shows skill descriptions, so a description edit must drift
        // the signature — but only a HASH of it is included, never the raw text.
        let dir = workspace();
        let now = "2026-06-05T00:00:00Z";
        let skill_dir = dir.path().join(".maestro/skills/_global");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("contract-first.md"),
            "---\nname: contract-first\ndescription: the original description\n---\nbody\n",
        )
        .unwrap();
        let before = compute_session_signature("cursor", None, SessionMode::Exec, now);

        std::fs::write(
            skill_dir.join("contract-first.md"),
            "---\nname: contract-first\ndescription: ZZdistinctZZ rewritten\n---\nbody\n",
        )
        .unwrap();
        let after = compute_session_signature("cursor", None, SessionMode::Exec, now);

        assert_ne!(
            before.skill_index_hash, after.skill_index_hash,
            "description must drift"
        );
        assert_ne!(before.hash, after.hash);
        let json = serde_json::to_string(&after).unwrap();
        assert!(
            !json.contains("ZZdistinctZZ"),
            "raw description leaked: {json}"
        );
        clear();
    }
}
