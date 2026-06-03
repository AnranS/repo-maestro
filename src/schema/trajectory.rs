use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::adapter::Usage;
use crate::schema::artifacts::ArtifactRef;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryEventKind {
    ToolCall,
    Command,
    Output,
    Error,
    Final,
    Usage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryStatus {
    Started,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Redaction {
    None,
    Partial,
    SecretStripped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryEvent {
    #[serde(default = "crate::schema::trajectory_event_version")]
    pub schema_version: String,
    pub run_id: String,
    pub task_id: String,
    pub seq: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub provider_id: String,
    pub kind: TrajectoryEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TrajectoryStatus>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<String, ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub redaction: Redaction,
}

impl TrajectoryEvent {
    pub const SCHEMA_VERSION: &'static str = crate::schema::TRAJECTORY_EVENT_V1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trajectory_event_schema_version_is_stable() {
        assert_eq!(
            TrajectoryEvent::SCHEMA_VERSION,
            "maestro.trajectory_event.v1"
        );
    }

    #[test]
    fn trajectory_event_kinds_serialize_as_v1_values() {
        assert_eq!(
            serde_json::to_value(TrajectoryEventKind::ToolCall).unwrap(),
            "tool_call"
        );
        assert_eq!(
            serde_json::to_value(Redaction::SecretStripped).unwrap(),
            "secret_stripped"
        );
    }
}
