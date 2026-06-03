//! Round-trip tests for the L1 memory store.

use maestro::memory::MemoryStore;
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

#[test]
#[serial]
fn put_get_round_trip_preserves_content() {
    let _dir = fresh_workspace();
    let store = MemoryStore::open().unwrap();

    store.add("api", "auth.md", "use HS256\n").unwrap();
    let got = store.read("api", "auth.md").unwrap();
    assert_eq!(got, "use HS256\n");

    clear();
}

#[test]
#[serial]
fn list_l1_groups_by_topic() {
    let _dir = fresh_workspace();
    let store = MemoryStore::open().unwrap();

    store.add("api", "a.md", "x").unwrap();
    store.add("api", "b.md", "y").unwrap();
    store.add("design", "tokens.md", "z").unwrap();

    let listing = store.list_l1().unwrap();
    assert_eq!(listing["api"].len(), 2);
    assert_eq!(listing["design"], vec!["tokens.md".to_string()]);

    clear()
}

#[test]
#[serial]
fn load_for_topics_returns_only_requested_topics() {
    let _dir = fresh_workspace();
    let store = MemoryStore::open().unwrap();

    store.add("api", "auth.md", "secret").unwrap();
    store.add("ui", "design.md", "inter").unwrap();

    let slices = store.load_for_topics(&["api".into()]).unwrap();
    assert_eq!(slices.len(), 1);
    assert!(slices[0].content.contains("secret"));

    // Empty topic list returns nothing
    assert!(store.load_for_topics(&[]).unwrap().is_empty());

    // Unknown topic silently ignored, not an error
    assert!(store.load_for_topics(&["bogus".into()]).unwrap().is_empty());

    clear();
}

#[test]
#[serial]
fn delete_removes_file_and_collapses_empty_topic_dir() {
    let _dir = fresh_workspace();
    let store = MemoryStore::open().unwrap();

    store.add("api", "x.md", "1").unwrap();
    let topic_dir = store.l1_root().join("api");
    assert!(topic_dir.exists());

    store.delete("api", "x.md").unwrap();
    assert!(
        !topic_dir.exists(),
        "empty topic directory should be cleaned up"
    );

    // Deleting again is a no-op (not an error)
    store.delete("api", "x.md").unwrap();

    clear();
}
