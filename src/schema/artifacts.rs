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
}
