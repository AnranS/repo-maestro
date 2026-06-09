use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSource {
    AgentTask,
    Hook,
    Verification,
    External,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub kind: String,
    pub source: ArtifactSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    #[serde(default = "crate::schema::artifact_manifest_version")]
    pub schema_version: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
}

impl ArtifactManifest {
    pub const SCHEMA_VERSION: &'static str = crate::schema::ARTIFACT_MANIFEST_V1;

    pub fn new(run_id: impl Into<String>, artifacts: Vec<ArtifactRef>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            run_id: run_id.into(),
            artifacts,
        }
    }
}

/// A run-relative path: reject Unix-absolute, a leading slash/backslash (covers
/// UNC `\\server` / `//server`), a Windows drive-letter root (`C:\`, `C:/`), and
/// any `..` traversal component. Canonical home for the rule; `scheduler::events`
/// delegates here so the event ledger and the evidence ledger never drift.
pub fn ref_path_is_unsafe(path: &str) -> bool {
    if std::path::Path::new(path).is_absolute() {
        return true;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    if path.split(['/', '\\']).any(|component| component == "..") {
        return true;
    }
    // Any Windows drive prefix `[A-Za-z]:` — drive-root (`C:\x`, `C:/x`) AND
    // drive-relative (`C:`, `C:foo`, `D:data/log`). Drive-relative resolves
    // against that drive's current directory, so it is never run-relative.
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Reject `file:` URIs (any case) — a run-local artifact uses `path`, never a
/// filesystem URI; only remote/opaque schemes belong in `uri`.
pub fn ref_uri_is_unsafe(uri: &str) -> bool {
    uri.trim_start().to_ascii_lowercase().starts_with("file:")
}

impl ArtifactRef {
    /// F-124: returns a reason string if this ref is NOT run-local — an unsafe
    /// `path` (absolute / UNC / drive-letter / `..`), a `file:` `uri`, or a
    /// `task_id` that isn't a single safe path component. Valid JSON is not
    /// enough: a persisted ledger ref that resolves outside the run dir must be
    /// rejected on read.
    pub fn run_local_violation(&self) -> Option<String> {
        if let Some(path) = self.path.as_deref() {
            if ref_path_is_unsafe(path) {
                return Some(format!(
                    "artifact ref path must be run-relative \
                     (no absolute / UNC / drive-letter / '..'): {path:?}"
                ));
            }
        }
        if let Some(uri) = self.uri.as_deref() {
            if ref_uri_is_unsafe(uri) {
                return Some(format!(
                    "artifact ref uri must not use a file: scheme: {uri:?}"
                ));
            }
        }
        if let Some(task_id) = self.task_id.as_deref() {
            if crate::paths::validate_path_component("artifact ref task id", task_id).is_err() {
                return Some(format!(
                    "artifact ref task_id must be a safe path component: {task_id:?}"
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_manifest_schema_version_is_stable() {
        assert_eq!(
            ArtifactManifest::SCHEMA_VERSION,
            "maestro.artifact_manifest.v1"
        );
    }

    #[test]
    fn artifact_source_serializes_v1_values() {
        let value = serde_json::to_value(ArtifactSource::AgentTask).unwrap();
        assert_eq!(value, "agent_task");
    }

    #[test]
    fn run_local_violation_flags_absolute_traversal_file_uri_and_unsafe_task_id() {
        let base = ArtifactRef {
            kind: "log".into(),
            source: ArtifactSource::AgentTask,
            task_id: None,
            path: None,
            uri: None,
            name: None,
            bytes: None,
        };
        // clean: run-relative path + https uri + safe task id → no violation.
        let ok = ArtifactRef {
            path: Some("trajectories/T0.ndjson".into()),
            uri: Some("https://example.invalid/pr/1".into()),
            task_id: Some("T0".into()),
            ..base.clone()
        };
        assert!(ok.run_local_violation().is_none());

        let unsafe_cases = [
            ArtifactRef {
                path: Some("/abs/run/logs/T0.log".into()),
                ..base.clone()
            },
            ArtifactRef {
                path: Some("../escape.log".into()),
                ..base.clone()
            },
            ArtifactRef {
                path: Some("C:\\windows\\x".into()),
                ..base.clone()
            },
            // Windows drive-RELATIVE (no slash after the colon) is also unsafe.
            ArtifactRef {
                path: Some("C:foo".into()),
                ..base.clone()
            },
            ArtifactRef {
                path: Some("C:".into()),
                ..base.clone()
            },
            ArtifactRef {
                uri: Some("file:///etc/passwd".into()),
                ..base.clone()
            },
            ArtifactRef {
                task_id: Some("../evil".into()),
                ..base.clone()
            },
        ];
        for c in unsafe_cases {
            assert!(
                c.run_local_violation().is_some(),
                "expected a violation for {c:?}"
            );
        }
    }
}
