pub mod artifacts;
pub mod channel_envelope;
pub mod monitor;
pub mod permissions;
pub mod preview;
pub mod redaction;
pub mod trajectory;

pub use monitor::{RUN_MONITOR_V1, TASK_DETAIL_V1};

pub const PROVIDER_CAPABILITY_V1: &str = "maestro.provider_capability.v1";
pub const PERMISSION_V1: &str = "maestro.permission.v1";
pub const RUN_EVENT_V1: &str = "maestro.run_event.v1";
pub const RUN_EVENT_V2: &str = "maestro.run_event.v2";
pub const ARTIFACT_MANIFEST_V1: &str = "maestro.artifact_manifest.v1";
pub const TRAJECTORY_EVENT_V1: &str = "maestro.trajectory_event.v1";
pub const CHANNEL_ENVELOPE_V1: &str = "maestro.channel_envelope.v1";
pub const FINDING_V1: &str = "maestro.finding.v1";
pub const PLAN_PREVIEW_V1: &str = "maestro.plan_preview.v1";

pub fn provider_capability_version() -> String {
    PROVIDER_CAPABILITY_V1.to_string()
}

pub fn permission_version() -> String {
    PERMISSION_V1.to_string()
}

pub fn run_event_version() -> String {
    RUN_EVENT_V2.to_string()
}

pub fn artifact_manifest_version() -> String {
    ARTIFACT_MANIFEST_V1.to_string()
}

pub fn trajectory_event_version() -> String {
    TRAJECTORY_EVENT_V1.to_string()
}

pub fn channel_envelope_version() -> String {
    CHANNEL_ENVELOPE_V1.to_string()
}

pub fn finding_version() -> String {
    FINDING_V1.to_string()
}

pub fn plan_preview_version() -> String {
    PLAN_PREVIEW_V1.to_string()
}

pub fn run_monitor_version() -> String {
    RUN_MONITOR_V1.to_string()
}

pub fn task_detail_version() -> String {
    TASK_DETAIL_V1.to_string()
}
