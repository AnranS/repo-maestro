//! F-116 task context-layer manifest (`maestro.task_context_manifest.v1`).
//!
//! A read-only, provenance-only projection of how a dispatched **agent** task's
//! prompt context was layered: which layers contributed, in what order, and
//! roughly how large each was. It deliberately records only provenance + size
//! metadata — never the raw prompt, raw layer bodies, mailbox/memory/skill/role
//! bodies, transcripts, logs, absolute paths, or content hashes. This module
//! owns the types + validation; the filesystem write/read helpers live in
//! `scheduler::context`.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Display labels / ids / omitted reasons are short single-line summaries.
const MAX_LABEL_BYTES: usize = 120;
/// Symbolic source / ref values (cap before the path/URI guard).
const MAX_REF_BYTES: usize = 256;
/// Ref kind tags are tiny — never a place to smuggle raw info.
const MAX_REF_KIND_BYTES: usize = 64;

/// Closed v1 set of prompt-context layer kinds. Strict: an unknown kind fails to
/// deserialize (a corrupt manifest), consistent with the design's "corrupt
/// manifest read is an error" rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextLayerKind {
    #[serde(rename = "task.prompt")]
    TaskPrompt,
    #[serde(rename = "project.instructions")]
    ProjectInstructions,
    #[serde(rename = "attempt.diagnosis")]
    AttemptDiagnosis,
    #[serde(rename = "contract.consumed")]
    ContractConsumed,
    #[serde(rename = "code.context")]
    CodeContext,
    #[serde(rename = "mailbox.inbox")]
    MailboxInbox,
    #[serde(rename = "mode.constraints")]
    ModeConstraints,
    #[serde(rename = "role.prelude")]
    RolePrelude,
    #[serde(rename = "skills.section")]
    SkillsSection,
    #[serde(rename = "memory.topic_scope")]
    MemoryTopicScope,
    #[serde(rename = "memory.contract_fan_in")]
    MemoryContractFanIn,
    #[serde(rename = "memory.dependency_fan_in")]
    MemoryDependencyFanIn,
    #[serde(rename = "memory.prompt_similarity")]
    MemoryPromptSimilarity,
    #[serde(rename = "workflow.inputs")]
    WorkflowInputs,
}

/// A symbolic / run-relative reference for a layer — never an absolute path,
/// `..` traversal, or `file:` URI (enforced by [`validate_manifest`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLayerRef {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

impl ContextLayerRef {
    pub fn new(kind: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            reference: reference.into(),
            count: None,
        }
    }

    pub fn count(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    /// Build a ref ONLY if both fields pass the manifest guard (non-empty,
    /// single-line, capped; the reference also symbolic/run-relative — no
    /// absolute / UNC / drive-letter / `..` / `file:`). Returns `None` for an
    /// unsafe value so a single bad external ref is dropped to a count instead of
    /// failing the whole (best-effort) manifest write.
    pub fn checked(kind: impl Into<String>, reference: impl Into<String>) -> Option<Self> {
        let kind = kind.into();
        let reference = reference.into();
        if validate_text("context ref kind", &kind, MAX_REF_KIND_BYTES, true).is_err()
            || validate_text("context ref", &reference, MAX_REF_BYTES, true).is_err()
            || ref_value_is_unsafe(&reference)
        {
            return None;
        }
        Some(Self {
            kind,
            reference,
            count: None,
        })
    }
}

/// One ordered prompt-context layer: provenance + size only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLayer {
    pub order: u32,
    pub id: String,
    pub kind: ContextLayerKind,
    pub label: String,
    /// Symbolic source (e.g. `plan`, `codegraph`, `memory`), never an absolute path.
    pub source: String,
    pub item_count: u32,
    pub content_bytes: u64,
    pub estimated_tokens: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub omitted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_reason: Option<String>,
    // Always serialized (even when empty) so consumers never distinguish
    // "no refs" from "field missing".
    #[serde(default)]
    pub refs: Vec<ContextLayerRef>,
}

impl ContextLayer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        order: u32,
        id: impl Into<String>,
        kind: ContextLayerKind,
        label: impl Into<String>,
        source: impl Into<String>,
        item_count: u32,
        content_bytes: u64,
    ) -> Self {
        Self {
            order,
            id: id.into(),
            kind,
            label: label.into(),
            source: source.into(),
            item_count,
            content_bytes,
            estimated_tokens: rough_tokens(content_bytes),
            truncated: false,
            omitted: false,
            omitted_reason: None,
            refs: Vec::new(),
        }
    }

    pub fn truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Mark the layer as considered-but-empty/skipped, with a reason.
    pub fn omitted(mut self, reason: impl Into<String>) -> Self {
        self.omitted = true;
        self.omitted_reason = Some(reason.into());
        self
    }

    pub fn with_ref(mut self, r: ContextLayerRef) -> Self {
        self.refs.push(r);
        self
    }
}

/// Per-task context manifest (`maestro.task_context_manifest.v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContextManifest {
    #[serde(default = "crate::schema::task_context_manifest_version")]
    pub schema_version: String,
    pub run_id: String,
    pub task_id: String,
    pub project: String,
    /// Task kind, expected `agent` in v1.
    pub kind: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_agent_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_review_profile: Option<String>,
    pub total_context_bytes: u64,
    /// Deterministic rough estimate (`ceil(total_context_bytes / 4)`), NOT
    /// adapter-reported usage. Real usage stays on `TaskState.usage` / F-112.
    pub estimated_input_tokens: u64,
    pub layers: Vec<ContextLayer>,
    /// RFC3339, supplied by the caller (so tests are deterministic).
    pub created_at: String,
}

impl TaskContextManifest {
    /// Build a manifest from its header + ordered layers; the byte total and the
    /// rough token estimate are derived from the layers' `content_bytes`.
    pub fn new(
        run_id: impl Into<String>,
        task_id: impl Into<String>,
        project: impl Into<String>,
        kind: impl Into<String>,
        agent: impl Into<String>,
        created_at: impl Into<String>,
        layers: Vec<ContextLayer>,
    ) -> Self {
        let total_context_bytes = layers.iter().map(|l| l.content_bytes).sum();
        Self {
            schema_version: crate::schema::task_context_manifest_version(),
            run_id: run_id.into(),
            task_id: task_id.into(),
            project: project.into(),
            kind: kind.into(),
            agent: agent.into(),
            model: None,
            role: None,
            resolved_agent_profile: None,
            resolved_review_profile: None,
            total_context_bytes,
            estimated_input_tokens: rough_tokens(total_context_bytes),
            layers,
            created_at: created_at.into(),
        }
    }

    pub fn model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    pub fn role(mut self, role: Option<String>) -> Self {
        self.role = role;
        self
    }

    pub fn profiles(
        mut self,
        agent_profile: Option<String>,
        review_profile: Option<String>,
    ) -> Self {
        self.resolved_agent_profile = agent_profile;
        self.resolved_review_profile = review_profile;
        self
    }
}

/// Deterministic rough token estimate: `ceil(content_bytes / 4)`. Explicitly an
/// estimate — never a substitute for adapter-reported usage.
pub fn rough_tokens(content_bytes: u64) -> u64 {
    content_bytes.div_ceil(4)
}

/// Privacy + shape validation, run BEFORE a write AND AFTER a read (a manifest
/// hand-edited to carry an absolute ref / `file:` URI / multi-line value must be
/// rejected as corrupt, not served as valid). Mirrors the F-110/F-115 discipline.
pub fn validate_manifest(manifest: &TaskContextManifest) -> Result<()> {
    ensure!(
        manifest.schema_version == crate::schema::TASK_CONTEXT_MANIFEST_V1,
        "context manifest schema_version {:?} is not {}",
        manifest.schema_version,
        crate::schema::TASK_CONTEXT_MANIFEST_V1
    );
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&manifest.created_at).is_ok(),
        "context manifest created_at {:?} is not RFC3339",
        manifest.created_at
    );
    let mut total: u64 = 0;
    for (index, layer) in manifest.layers.iter().enumerate() {
        // order must be contiguous 0..n-1 (stable for UI sort + write order).
        ensure!(
            layer.order as usize == index,
            "context layer order {} must equal its position {} (contiguous 0..n)",
            layer.order,
            index
        );
        // id: a stable slug, never a path.
        validate_text("context layer id", &layer.id, MAX_LABEL_BYTES, true)?;
        ensure!(
            !layer.id.contains('/') && !layer.id.contains('\\') && !layer.id.contains(".."),
            "context layer id {:?} must be a stable slug (no '/', '\\', or '..')",
            layer.id
        );
        // label / omitted_reason: single-line, capped (may be empty).
        validate_text("context layer label", &layer.label, MAX_LABEL_BYTES, false)?;
        if let Some(reason) = &layer.omitted_reason {
            validate_text(
                "context layer omitted_reason",
                reason,
                MAX_LABEL_BYTES,
                false,
            )?;
        }
        // source: non-empty, single-line, capped, then the path/URI guard.
        validate_text("context layer source", &layer.source, MAX_REF_BYTES, true)?;
        ensure!(
            !ref_value_is_unsafe(&layer.source),
            "context layer {:?} source must be symbolic (no absolute / UNC / drive-letter / '..' / file:): {:?}",
            layer.id,
            layer.source
        );
        for r in &layer.refs {
            validate_text("context ref kind", &r.kind, MAX_REF_KIND_BYTES, true)?;
            validate_text("context ref", &r.reference, MAX_REF_BYTES, true)?;
            ensure!(
                !ref_value_is_unsafe(&r.reference),
                "context layer {:?} ref {:?} must be symbolic/run-relative \
                 (no absolute / UNC / drive-letter / '..' / file:): {:?}",
                layer.id,
                r.kind,
                r.reference
            );
        }
        // derived: per-layer rough estimate must match its content_bytes.
        ensure!(
            layer.estimated_tokens == rough_tokens(layer.content_bytes),
            "context layer {:?} estimated_tokens {} != rough_tokens(content_bytes {})",
            layer.id,
            layer.estimated_tokens,
            layer.content_bytes
        );
        total = total
            .checked_add(layer.content_bytes)
            .ok_or_else(|| anyhow::anyhow!("context manifest byte total overflows u64"))?;
    }
    ensure!(
        manifest.total_context_bytes == total,
        "context manifest total_context_bytes {} != sum of layer content_bytes {}",
        manifest.total_context_bytes,
        total
    );
    ensure!(
        manifest.estimated_input_tokens == rough_tokens(manifest.total_context_bytes),
        "context manifest estimated_input_tokens {} != rough_tokens(total_context_bytes {})",
        manifest.estimated_input_tokens,
        manifest.total_context_bytes
    );
    Ok(())
}

/// Single-line + byte-capped text; optionally required non-empty (trimmed).
fn validate_text(field: &str, value: &str, max_bytes: usize, require_nonempty: bool) -> Result<()> {
    if require_nonempty {
        ensure!(!value.trim().is_empty(), "{field} must be non-empty");
    }
    ensure!(
        !value.contains(['\n', '\r']),
        "{field} must be single-line (no newline characters)"
    );
    ensure!(
        value.len() <= max_bytes,
        "{field} exceeds {max_bytes} bytes"
    );
    Ok(())
}

/// A layer `source` / ref value must be symbolic or run-relative: reject `file:`
/// URIs, Unix/Windows/UNC absolutes, a leading slash/backslash, and `..`.
fn ref_value_is_unsafe(value: &str) -> bool {
    if value.trim_start().to_ascii_lowercase().starts_with("file:") {
        return true;
    }
    if Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_layer(order: u32) -> ContextLayer {
        ContextLayer::new(
            order,
            "memory.topic_scope",
            ContextLayerKind::MemoryTopicScope,
            "topic scope memory",
            "memory",
            3,
            400,
        )
        .with_ref(ContextLayerRef::new(
            "memory_topic",
            "billing-service/decisions",
        ))
        .with_ref(ContextLayerRef::new("count", "slices").count(3))
    }

    fn sample_manifest() -> TaskContextManifest {
        let layers = vec![
            ContextLayer::new(
                0,
                "task.prompt",
                ContextLayerKind::TaskPrompt,
                "task body",
                "plan",
                1,
                200,
            ),
            sample_layer(1),
            ContextLayer::new(
                2,
                "code.context",
                ContextLayerKind::CodeContext,
                "codegraph relevant code",
                "codegraph",
                0,
                0,
            )
            .omitted("empty"),
        ];
        TaskContextManifest::new(
            "run-1",
            "T0",
            "billing-service",
            "agent",
            "mock",
            "2026-06-04T00:00:00Z",
            layers,
        )
        .model(Some("demo-model".into()))
        .role(Some("backend_rust".into()))
        .profiles(Some("backend-specialist".into()), None)
    }

    #[test]
    fn full_manifest_round_trips_and_computes_totals() {
        let m = sample_manifest();
        // 200 + 400 + 0
        assert_eq!(m.total_context_bytes, 600);
        assert_eq!(m.estimated_input_tokens, rough_tokens(600));
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"schema_version\":\"maestro.task_context_manifest.v1\""));
        assert!(json.contains("\"kind\":\"memory.topic_scope\""));
        let back: TaskContextManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn minimal_manifest_round_trips() {
        let m = TaskContextManifest::new(
            "run-1",
            "T0",
            "web-frontend",
            "agent",
            "mock",
            "2026-06-04T00:00:00Z",
            vec![],
        );
        assert_eq!(m.total_context_bytes, 0);
        assert_eq!(m.estimated_input_tokens, 0);
        let json = serde_json::to_string(&m).unwrap();
        // optional header fields are omitted when None
        assert!(!json.contains("\"model\""));
        let back: TaskContextManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn rough_tokens_is_deterministic_ceil_div_4() {
        assert_eq!(rough_tokens(0), 0);
        assert_eq!(rough_tokens(1), 1);
        assert_eq!(rough_tokens(4), 1);
        assert_eq!(rough_tokens(5), 2);
        assert_eq!(rough_tokens(600), 150);
    }

    #[test]
    fn unknown_layer_kind_fails_to_deserialize() {
        let line = r#"{"order":0,"id":"x","kind":"usage.sampled","label":"l","source":"s","item_count":0,"content_bytes":0,"estimated_tokens":0}"#;
        assert!(serde_json::from_str::<ContextLayer>(line).is_err());
    }

    #[test]
    fn label_validation_rejects_newline_and_oversize() {
        let mut m = sample_manifest();
        m.layers[0].label = "bad\nlabel".to_string();
        assert!(validate_manifest(&m).is_err());

        let mut m = sample_manifest();
        m.layers[0].label = "x".repeat(MAX_LABEL_BYTES + 1);
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn refs_and_source_reject_unsafe_values() {
        for bad in [
            "/etc/passwd",
            "../escape/x",
            r"C:\Users\x",
            "C:/data/x",
            r"\\server\share",
            "//server/share",
            "file:///opt/a",
        ] {
            let mut m = sample_manifest();
            m.layers[1].refs = vec![ContextLayerRef::new("artifact", bad)];
            assert!(
                validate_manifest(&m).is_err(),
                "ref should be rejected: {bad}"
            );

            let mut m = sample_manifest();
            m.layers[1].source = bad.to_string();
            assert!(
                validate_manifest(&m).is_err(),
                "source should be rejected: {bad}"
            );
        }
        // symbolic / run-relative values pass
        for ok in [
            "_global/verify-before-done",
            "billing-service/decisions",
            "context/T0.json",
            "backend_rust",
        ] {
            let mut m = sample_manifest();
            m.layers[1].refs = vec![ContextLayerRef::new("ref", ok)];
            assert!(validate_manifest(&m).is_ok(), "ref should pass: {ok}");
        }
    }

    #[test]
    fn created_at_must_be_rfc3339() {
        let mut m = sample_manifest();
        m.created_at = "not-a-date".to_string();
        assert!(validate_manifest(&m).is_err());
        m.created_at = "2026-06-04T00:00:00Z".to_string();
        assert!(validate_manifest(&m).is_ok());
    }

    #[test]
    fn source_and_ref_reject_empty_multiline_and_oversize() {
        let rejected = |mutate: &dyn Fn(&mut TaskContextManifest)| {
            let mut m = sample_manifest();
            mutate(&mut m);
            validate_manifest(&m).is_err()
        };
        assert!(
            rejected(&|m| m.layers[1].source = "   ".into()),
            "blank source"
        );
        assert!(
            rejected(&|m| m.layers[1].source = "a\nb".into()),
            "multiline source"
        );
        assert!(
            rejected(&|m| m.layers[1].source = "x".repeat(257)),
            "oversize source"
        );
        assert!(
            rejected(&|m| m.layers[1].refs = vec![ContextLayerRef::new("artifact", "")]),
            "empty ref"
        );
        assert!(
            rejected(
                &|m| m.layers[1].refs = vec![ContextLayerRef::new("artifact", "x".repeat(257))]
            ),
            "oversize ref"
        );
        assert!(
            rejected(&|m| m.layers[1].refs = vec![ContextLayerRef::new("k\nx", "ok")]),
            "multiline ref kind"
        );
        assert!(
            rejected(&|m| m.layers[1].refs = vec![ContextLayerRef::new("x".repeat(65), "ok")]),
            "oversize ref kind"
        );
    }

    #[test]
    fn layer_id_must_be_stable_slug() {
        for bad in ["bad/id", r"bad\id", "a..b", "", "  "] {
            let mut m = sample_manifest();
            m.layers[0].id = bad.to_string();
            assert!(validate_manifest(&m).is_err(), "bad layer id {bad:?}");
        }
    }

    #[test]
    fn omitted_reason_must_be_single_line() {
        let mut m = sample_manifest();
        m.layers[2].omitted_reason = Some("multi\nline".to_string());
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn data_integrity_guard_rejects_tampering() {
        // the clean sample passes (contiguous order, consistent totals/estimates)
        assert!(validate_manifest(&sample_manifest()).is_ok());
        let rejected = |mutate: &dyn Fn(&mut TaskContextManifest)| {
            let mut m = sample_manifest();
            mutate(&mut m);
            validate_manifest(&m).is_err()
        };
        assert!(
            rejected(&|m| m.schema_version = "maestro.task_context_manifest.v2".into()),
            "wrong schema_version"
        );
        assert!(rejected(&|m| m.total_context_bytes += 1), "tampered total");
        assert!(
            rejected(&|m| m.estimated_input_tokens += 1),
            "tampered manifest estimate"
        );
        assert!(
            rejected(&|m| m.layers[0].estimated_tokens += 1),
            "tampered layer estimate"
        );
        assert!(rejected(&|m| m.layers[1].order = 5), "skipped order");
        assert!(rejected(&|m| m.layers[1].order = 0), "duplicate order");
    }
}
