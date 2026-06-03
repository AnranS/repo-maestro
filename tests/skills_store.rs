//! Skills: frontmatter parsing, scope visibility, trigger matching, IDE
//! mirroring. Uses MAESTRO_WORKSPACE_ROOT to avoid polluting the real cwd.

use maestro::cli::samples::{update_bundled_skills, SkillUpdateStatus};
use maestro::skills::{
    delete, list_all, load, parse_skill, render_task_section, resolve_for_task, save, sync_to_ide,
    trigger_match, SkillScope,
};
use serial_test::serial;
use tempfile::TempDir;

fn fresh_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", dir.path());
    }
    dir
}
fn clear() {
    unsafe {
        std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
    }
}

// trigger uses `|` as the alternative separator (see skills::trigger_match).
const PYTEST_SKILL: &str = "---
description: write FastAPI pytest cases
trigger: write tests|add coverage|pytest
---

# pytest playbook

1. Use TestClient.
2. Mock external IO.
";

#[test]
#[serial]
fn save_then_load_preserves_frontmatter_and_body() {
    let _dir = fresh_workspace();

    save(
        &SkillScope::Project("api".into()),
        "pytest-playbook",
        PYTEST_SKILL,
    )
    .unwrap();

    let s = load(&SkillScope::Project("api".into()), "pytest-playbook").unwrap();
    assert_eq!(s.description.as_deref(), Some("write FastAPI pytest cases"));
    assert!(s.trigger.unwrap().contains("pytest"));
    assert!(s.content.contains("# pytest playbook"));
    assert!(!s.content.contains("---"), "frontmatter not in body");

    clear();
}

#[test]
#[serial]
fn parse_handles_skill_without_frontmatter() {
    let _dir = fresh_workspace();
    save(
        &SkillScope::Global,
        "no-fm",
        "# Hello\n\nNo frontmatter here.",
    )
    .unwrap();

    let s = load(&SkillScope::Global, "no-fm").unwrap();
    assert!(s.description.is_none());
    assert!(s.trigger.is_none());
    assert!(s.content.contains("# Hello"));

    clear();
}

#[test]
#[serial]
fn trigger_match_is_case_insensitive_and_or_joined() {
    let _dir = fresh_workspace();
    save(&SkillScope::Project("api".into()), "pytest", PYTEST_SKILL).unwrap();

    // Match on "Pytest" (case mismatch)
    let hits = trigger_match("Please run Pytest on auth.", Some("api")).unwrap();
    assert_eq!(hits.len(), 1, "should match via `pytest` trigger");

    // Match on the second phrase
    let hits2 = trigger_match("could you add coverage to auth?", Some("api")).unwrap();
    assert_eq!(hits2.len(), 1);

    // Unrelated text → no hit
    let none = trigger_match("just deploy", Some("api")).unwrap();
    assert!(none.is_empty());

    clear();
}

#[test]
#[serial]
fn project_scope_is_invisible_to_other_projects() {
    let _dir = fresh_workspace();
    save(&SkillScope::Project("api".into()), "api-only", PYTEST_SKILL).unwrap();

    let in_api = trigger_match("pytest please", Some("api")).unwrap();
    let in_web = trigger_match("pytest please", Some("web")).unwrap();

    assert_eq!(in_api.len(), 1);
    assert!(
        in_web.is_empty(),
        "web project should not see api-scoped skill"
    );

    clear();
}

#[test]
#[serial]
fn explicit_task_skills_resolve_project_before_global_and_render_body() {
    let _dir = fresh_workspace();
    save(&SkillScope::Global, "pytest", PYTEST_SKILL).unwrap();
    save(
        &SkillScope::Project("api".into()),
        "pytest",
        "---
description: project-specific pytest
trigger: nope
---

# api pytest
",
    )
    .unwrap();

    let skills = resolve_for_task("no trigger", Some("api"), &["pytest".into()]).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].scope, SkillScope::Project("api".into()));
    assert!(skills[0].content.contains("api pytest"));

    let rendered = render_task_section(&skills).unwrap();
    assert!(rendered.contains("Maestro skills"));
    assert!(rendered.contains("api pytest"));

    clear();
}

#[test]
#[serial]
fn explicit_task_skills_fail_fast_when_missing() {
    let _dir = fresh_workspace();
    let err = resolve_for_task("no trigger", Some("api"), &["missing-skill".into()])
        .expect_err("missing explicit skill should fail");
    assert!(format!("{err:#}").contains("missing-skill"));

    clear();
}

#[test]
#[serial]
fn list_all_groups_by_scope_directory_name() {
    let _dir = fresh_workspace();
    save(&SkillScope::Global, "globe", PYTEST_SKILL).unwrap();
    save(&SkillScope::Project("api".into()), "a", PYTEST_SKILL).unwrap();
    save(&SkillScope::Project("web".into()), "w", PYTEST_SKILL).unwrap();

    let all = list_all().unwrap();
    assert!(all.contains_key("_global"));
    assert!(all.contains_key("api"));
    assert!(all.contains_key("web"));
    assert_eq!(all["api"].len(), 1);

    clear();
}

#[test]
#[serial]
fn sync_to_ide_writes_cursor_and_claude_mirrors() {
    let dir = fresh_workspace();
    sync_to_ide(&SkillScope::Project("api".into()), "pytest", PYTEST_SKILL).unwrap();

    // Mirrors live at the workspace root, prefixed with maestro-project-<name>
    // (or maestro-global-<name>) — they're flat, not nested under per-project dirs.
    let cursor_mdc = dir
        .path()
        .join(".cursor")
        .join("rules")
        .join("maestro-project-api-pytest.mdc");
    let claude = dir
        .path()
        .join(".claude")
        .join("skills")
        .join("maestro-project-api-pytest")
        .join("SKILL.md");
    assert!(cursor_mdc.exists(), "missing {cursor_mdc:?}");
    assert!(claude.exists(), "missing {claude:?}");

    // Body content makes it through verbatim
    let mdc = std::fs::read_to_string(&cursor_mdc).unwrap();
    assert!(mdc.contains("pytest playbook"));

    clear();
}

#[test]
#[serial]
fn delete_removes_disk_file() {
    let _dir = fresh_workspace();
    save(&SkillScope::Global, "x", PYTEST_SKILL).unwrap();
    assert!(load(&SkillScope::Global, "x").is_ok());

    delete(&SkillScope::Global, "x").unwrap();
    assert!(load(&SkillScope::Global, "x").is_err());

    clear();
}

#[test]
#[serial]
fn bundled_skill_update_installs_and_respects_local_edits() {
    let _dir = fresh_workspace();

    let added = update_bundled_skills(Some("qa-web-flow"), false).unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].status, SkillUpdateStatus::Added);
    let qa = load(&SkillScope::Global, "qa-web-flow").unwrap();
    assert!(qa.content.contains("real headless browser"));

    save(
        &SkillScope::Global,
        "qa-web-flow",
        "---\ndescription: custom qa\ntrigger: qa\n---\n\n# Local QA\n",
    )
    .unwrap();

    let skipped = update_bundled_skills(Some("_global/qa-web-flow"), false).unwrap();
    assert_eq!(skipped[0].status, SkillUpdateStatus::Skipped);
    let local = load(&SkillScope::Global, "qa-web-flow").unwrap();
    assert!(local.content.contains("Local QA"));

    let updated = update_bundled_skills(Some("qa-web-flow"), true).unwrap();
    assert_eq!(updated[0].status, SkillUpdateStatus::Updated);
    let restored = load(&SkillScope::Global, "qa-web-flow").unwrap();
    assert!(restored.content.contains("real headless browser"));

    clear();
}

#[test]
#[serial]
fn parse_skill_direct_handles_malformed_frontmatter() {
    let _dir = fresh_workspace();
    let path = std::env::var("MAESTRO_WORKSPACE_ROOT").unwrap();
    let bad = std::path::Path::new(&path).join("bad.md");
    std::fs::write(
        &bad,
        "---\nthis: is not(closed)\n\n# body without closing fence",
    )
    .unwrap();

    // Should not panic; description/trigger are None and body has the raw.
    let s = parse_skill(SkillScope::Global, &bad).unwrap();
    assert!(s.description.is_none());
    assert!(s.trigger.is_none());

    clear();
}
