//! F-116 filesystem helpers for the task context manifest: where it lives, how
//! it is written (validated, best-effort), and how it is read (missing =
//! `None`, corrupt = error, never silently empty). The types + validation live
//! in [`crate::schema::context`].

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::paths;
use crate::schema::context::{
    validate_manifest, ContextLayer, ContextLayerKind, ContextLayerRef, TaskContextManifest,
};

const CONTEXT_DIR: &str = "context";

/// Collects ordered context layers during executor assembly — the "recorder, not
/// renderer" of F-116. Each `record*` call appends a layer with the next
/// contiguous `order`, so the manifest order matches the call sequence (which the
/// caller drives in real render order). It only ever stores provenance + size,
/// never raw bodies.
#[derive(Debug, Default)]
pub struct ContextLayerRecorder {
    layers: Vec<ContextLayer>,
}

impl ContextLayerRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_order(&self) -> u32 {
        self.layers.len() as u32
    }

    /// Record a present layer. `refs` should already be `ContextLayerRef::checked`
    /// (unsafe values dropped to a plain count by the caller).
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        kind: ContextLayerKind,
        id: impl Into<String>,
        label: impl Into<String>,
        source: impl Into<String>,
        item_count: u32,
        content_bytes: u64,
        refs: Vec<ContextLayerRef>,
    ) {
        let order = self.next_order();
        let mut layer =
            ContextLayer::new(order, id, kind, label, source, item_count, content_bytes);
        layer.refs = refs;
        self.layers.push(layer);
    }

    /// Record a considered-but-empty/skipped layer (kept only when useful for
    /// debugging, e.g. a memory load that failed).
    pub fn record_omitted(
        &mut self,
        kind: ContextLayerKind,
        id: impl Into<String>,
        label: impl Into<String>,
        source: impl Into<String>,
        reason: impl Into<String>,
    ) {
        let order = self.next_order();
        self.layers
            .push(ContextLayer::new(order, id, kind, label, source, 0, 0).omitted(reason));
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn into_layers(self) -> Vec<ContextLayer> {
        self.layers
    }
}

/// `.maestro/runs/<run-id>/context/`.
pub fn context_dir(run_dir: &Path) -> PathBuf {
    run_dir.join(CONTEXT_DIR)
}

/// Manifest path for a task, or `None` when the task id is not a safe path
/// component. The caller skips manifest generation and warns — it never
/// sanitizes the id into a different filename (same discipline as log/artifact
/// handlers).
pub fn manifest_path(run_dir: &Path, task_id: &str) -> Option<PathBuf> {
    if paths::validate_path_component("task id", task_id).is_err() {
        return None;
    }
    Some(context_dir(run_dir).join(format!("{task_id}.json")))
}

/// Validate + write a manifest. The caller treats failure as best-effort (warn +
/// continue the task); F-116 is observability, never a runtime gate.
pub fn write_manifest(run_dir: &Path, manifest: &TaskContextManifest) -> Result<()> {
    validate_manifest(manifest)?;
    let path = manifest_path(run_dir, &manifest.task_id).ok_or_else(|| {
        anyhow::anyhow!(
            "refusing to write context manifest for unsafe task id {:?}",
            manifest.task_id
        )
    })?;
    paths::ensure_dir(&context_dir(run_dir))?;
    let json = serde_json::to_string(manifest).context("serialize context manifest")?;
    std::fs::write(&path, json)
        .with_context(|| format!("write context manifest {}", path.display()))?;
    Ok(())
}

/// Read a task's manifest: `Ok(None)` when missing (endpoint -> 404), `Err` when
/// the file is corrupt (endpoint -> 500).
pub fn read_manifest(run_dir: &Path, task_id: &str) -> Result<Option<TaskContextManifest>> {
    let Some(path) = manifest_path(run_dir, task_id) else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let manifest: TaskContextManifest = serde_json::from_str(&text)
        .with_context(|| format!("parse context manifest {}", path.display()))?;
    // A hand-edited / drifted manifest that violates the privacy contract
    // (absolute ref, file: URI, multi-line value, ...) is corrupt — surface an
    // error so the endpoint returns 500, never serve it as valid.
    validate_manifest(&manifest)
        .with_context(|| format!("invalid context manifest {}", path.display()))?;
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::context::{ContextLayer, ContextLayerKind, ContextLayerRef};

    fn sample(task_id: &str) -> TaskContextManifest {
        let layers = vec![ContextLayer::new(
            0,
            "memory.topic_scope",
            ContextLayerKind::MemoryTopicScope,
            "topic scope",
            "memory",
            2,
            320,
        )
        .with_ref(ContextLayerRef::new(
            "memory_topic",
            "billing-service/decisions",
        ))];
        TaskContextManifest::new(
            "run-1",
            task_id,
            "billing-service",
            "agent",
            "mock",
            "2026-06-04T00:00:00Z",
            layers,
        )
    }

    #[test]
    fn write_then_read_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let m = sample("T0");
        write_manifest(temp.path(), &m).unwrap();
        let read = read_manifest(temp.path(), "T0").unwrap();
        assert_eq!(read, Some(m));
    }

    #[test]
    fn missing_manifest_reads_as_none() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_manifest(temp.path(), "T0").unwrap(), None);
    }

    #[test]
    fn corrupt_manifest_read_errors() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(context_dir(temp.path())).unwrap();
        std::fs::write(context_dir(temp.path()).join("T0.json"), "{not json").unwrap();
        assert!(read_manifest(temp.path(), "T0").is_err());
    }

    #[test]
    fn manifest_path_rejects_unsafe_task_id() {
        let temp = tempfile::tempdir().unwrap();
        for bad in ["../escape", "bad/name", "..", "a\\b"] {
            assert!(
                manifest_path(temp.path(), bad).is_none(),
                "unsafe id {bad} must yield no path"
            );
        }
        assert!(manifest_path(temp.path(), "T0").is_some());
    }

    #[test]
    fn write_rejects_unsafe_ref_and_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let mut m = sample("T0");
        m.layers[0].refs = vec![ContextLayerRef::new("artifact", "/etc/passwd")];
        assert!(write_manifest(temp.path(), &m).is_err());
        assert_eq!(read_manifest(temp.path(), "T0").unwrap(), None);
    }

    #[test]
    fn read_rejects_hand_edited_unsafe_manifest_as_corrupt() {
        // N1: a structurally-valid JSON manifest whose ref/source violates the
        // privacy contract must read as Err (endpoint 500), never Ok(Some).
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(context_dir(temp.path())).unwrap();
        let abs_ref = r#"{"schema_version":"maestro.task_context_manifest.v1","run_id":"run-1","task_id":"T0","project":"billing-service","kind":"agent","agent":"mock","total_context_bytes":0,"estimated_input_tokens":0,"created_at":"2026-06-04T00:00:00Z","layers":[{"order":0,"id":"x","kind":"task.prompt","label":"l","source":"plan","item_count":0,"content_bytes":0,"estimated_tokens":0,"refs":[{"kind":"artifact","ref":"/etc/passwd"}]}]}"#;
        std::fs::write(context_dir(temp.path()).join("T0.json"), abs_ref).unwrap();
        assert!(
            read_manifest(temp.path(), "T0").is_err(),
            "absolute ref -> corrupt"
        );

        let file_uri = r#"{"schema_version":"maestro.task_context_manifest.v1","run_id":"run-1","task_id":"T1","project":"web-frontend","kind":"agent","agent":"mock","total_context_bytes":0,"estimated_input_tokens":0,"created_at":"2026-06-04T00:00:00Z","layers":[{"order":0,"id":"x","kind":"task.prompt","label":"l","source":"file:///opt/x","item_count":0,"content_bytes":0,"estimated_tokens":0,"refs":[]}]}"#;
        std::fs::write(context_dir(temp.path()).join("T1.json"), file_uri).unwrap();
        assert!(
            read_manifest(temp.path(), "T1").is_err(),
            "file: source -> corrupt"
        );

        // a clean manifest still reads back fine
        write_manifest(temp.path(), &sample("T2")).unwrap();
        assert!(read_manifest(temp.path(), "T2").unwrap().is_some());
    }

    #[test]
    fn read_rejects_data_integrity_tampering() {
        // N3: structurally-valid JSON whose derived fields are inconsistent reads
        // as corrupt (endpoint 500).
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(context_dir(temp.path())).unwrap();
        // total_context_bytes (999) != layer content_bytes (320)
        let bad_total = r#"{"schema_version":"maestro.task_context_manifest.v1","run_id":"r","task_id":"T0","project":"billing-service","kind":"agent","agent":"mock","total_context_bytes":999,"estimated_input_tokens":250,"created_at":"2026-06-04T00:00:00Z","layers":[{"order":0,"id":"x","kind":"task.prompt","label":"l","source":"plan","item_count":0,"content_bytes":320,"estimated_tokens":80,"refs":[]}]}"#;
        std::fs::write(context_dir(temp.path()).join("T0.json"), bad_total).unwrap();
        assert!(
            read_manifest(temp.path(), "T0").is_err(),
            "bad total -> corrupt"
        );

        // layer order (3) does not match its position (0)
        let bad_order = r#"{"schema_version":"maestro.task_context_manifest.v1","run_id":"r","task_id":"T1","project":"web-frontend","kind":"agent","agent":"mock","total_context_bytes":0,"estimated_input_tokens":0,"created_at":"2026-06-04T00:00:00Z","layers":[{"order":3,"id":"x","kind":"task.prompt","label":"l","source":"plan","item_count":0,"content_bytes":0,"estimated_tokens":0,"refs":[]}]}"#;
        std::fs::write(context_dir(temp.path()).join("T1.json"), bad_order).unwrap();
        assert!(
            read_manifest(temp.path(), "T1").is_err(),
            "bad order -> corrupt"
        );
    }

    #[test]
    fn recorder_builds_body_free_manifest_and_drops_unsafe_refs() {
        // F-116 Step 2: a manifest assembled via the recorder (as dispatch does)
        // is contiguous, valid, and carries only counts/symbolic refs — no raw
        // body, and any unsafe external ref is dropped, never serialized.
        let mut rec = ContextLayerRecorder::new();
        rec.record(
            ContextLayerKind::MemoryTopicScope,
            "memory.topic_scope",
            "topic-scoped facts",
            "memory",
            1,
            100,
            vec![ContextLayerRef::new(
                "memory_topic",
                "billing-service/decisions",
            )],
        );
        let wf_refs: Vec<ContextLayerRef> = ["/etc/snapshot", "file:///opt/x", "input/alias"]
            .iter()
            .filter_map(|v| ContextLayerRef::checked("workflow_input", *v))
            .collect();
        rec.record(
            ContextLayerKind::WorkflowInputs,
            "workflow.inputs",
            "workflow outputs",
            "workflow",
            3,
            200,
            wf_refs,
        );
        rec.record(
            ContextLayerKind::TaskPrompt,
            "task.prompt",
            "task body",
            "plan",
            1,
            48,
            vec![],
        );
        let layers = rec.into_layers();
        assert_eq!(
            layers.iter().map(|l| l.order).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let manifest = TaskContextManifest::new(
            "run-1",
            "T0",
            "billing-service",
            "agent",
            "mock",
            "2026-06-04T00:00:00Z",
            layers,
        );
        assert!(validate_manifest(&manifest).is_ok());
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(!json.contains("/etc/snapshot"));
        assert!(!json.contains("file:///opt/x"));
        assert!(json.contains("input/alias"));
        assert!(json.contains("\"content_bytes\":100"));
        assert_eq!(manifest.total_context_bytes, 348);
    }
}
