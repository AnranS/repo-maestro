use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;

use maestro::config::{Contracts, Defaults, Project, ProjectsConfig};

fn project(path: &str) -> Project {
    Project {
        path: path.to_string(),
        r#type: None,
        stack: Vec::new(),
        commands: BTreeMap::new(),
        contracts: Contracts::default(),
        dependencies: Vec::new(),
        memory_scope: Vec::new(),
        agent: None,
        agent_model: None,
        cursor_model: None,
        model_profile: None,
        role: None,
        copy_files: Vec::new(),
    }
}

fn registry() -> ProjectsConfig {
    let mut cfg = ProjectsConfig {
        version: 1,
        defaults: Defaults::default(),
        projects: BTreeMap::new(),
    };
    cfg.projects.insert("api".into(), project("api"));
    cfg.projects.insert("web".into(), project("web"));
    cfg
}

/// Save, retrying the *expected* writer-lock contention bail.
///
/// `save()` uses a `create_new` temp file as a single-writer lock and bails
/// after a fixed (~50ms) retry budget when a sibling holds it — a deliberate
/// "another process is writing, try again" signal, not corruption. With two
/// threads hammering 200 saves each (far past any real usage), that budget is
/// genuinely exceeded under CI load, so a bare `.unwrap()` made this test flake
/// (F-105). Retrying the contention bail keeps what the test actually proves —
/// the reader never observes an empty/partial registry — without coupling the
/// result to writer-lock timing. Real (non-contention) errors still panic, and
/// progress is guaranteed: `create_new` lets exactly one writer win at a time,
/// then it renames and releases, so the bound is never hit in practice.
fn save_tolerating_contention(cfg: &ProjectsConfig, path: &std::path::Path) {
    for _ in 0..100_000 {
        match cfg.save(path) {
            Ok(()) => return,
            Err(e) if e.to_string().contains("another maestro process is writing") => {
                thread::yield_now();
            }
            Err(e) => panic!("unexpected (non-contention) save error: {e:#}"),
        }
    }
    panic!("save did not succeed within the contention-retry bound — likely a real deadlock");
}

#[test]
fn concurrent_save_writers_and_load_never_observe_empty_registry() {
    let temp = tempfile::tempdir().unwrap();
    let path = Arc::new(temp.path().join(".maestro/projects.yaml"));
    let cfg = Arc::new(registry());
    cfg.save(&path).unwrap();

    let writers = (0..2)
        .map(|_| {
            let writer_path = Arc::clone(&path);
            let writer_cfg = Arc::clone(&cfg);
            thread::spawn(move || {
                for _ in 0..200 {
                    save_tolerating_contention(&writer_cfg, &writer_path);
                }
            })
        })
        .collect::<Vec<_>>();

    let reader_path = Arc::clone(&path);
    let reader = thread::spawn(move || {
        for _ in 0..200 {
            if let Ok(loaded) = ProjectsConfig::load(&reader_path) {
                assert_eq!(
                    loaded.projects.len(),
                    2,
                    "load must never succeed with an empty or partial registry"
                );
            }
        }
    });

    for writer in writers {
        writer.join().unwrap();
    }
    reader.join().unwrap();
}
