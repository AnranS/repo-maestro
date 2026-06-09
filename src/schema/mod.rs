pub mod artifacts;
pub mod channel_envelope;
pub mod context;
pub mod delivery;
pub mod event_delivery;
pub mod monitor;
pub mod permissions;
pub mod preview;
pub mod redaction;
pub mod resume;
pub mod runtime_health;
pub mod runtime_profile;
pub mod session_control;
pub mod skill_inventory;
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
pub const PLAN_PREVIEW_SNAPSHOT_V1: &str = "maestro.plan_preview_snapshot.v1";
pub const DELIVERY_SPEC_V1: &str = "maestro.delivery_spec.v1";
pub const TASK_CONTEXT_MANIFEST_V1: &str = "maestro.task_context_manifest.v1";
pub const RESUME_DESCRIPTOR_V1: &str = "maestro.resume_descriptor.v1";
pub const RUNTIME_HEALTH_V1: &str = "maestro.runtime_health.v1";
pub const SESSION_CONTROL_V1: &str = "maestro.session_control.v1";
pub const RUN_EVENT_ACK_V1: &str = "maestro.run_event_ack.v1";
pub const RUN_EVENT_GAP_V1: &str = "maestro.run_event_gap.v1";
pub const SKILL_INVENTORY_V1: &str = "maestro.skill_inventory.v1";

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

pub fn plan_preview_snapshot_version() -> String {
    PLAN_PREVIEW_SNAPSHOT_V1.to_string()
}

pub fn delivery_spec_version() -> String {
    DELIVERY_SPEC_V1.to_string()
}

pub fn task_context_manifest_version() -> String {
    TASK_CONTEXT_MANIFEST_V1.to_string()
}

pub fn resume_descriptor_version() -> String {
    RESUME_DESCRIPTOR_V1.to_string()
}

pub fn runtime_health_version() -> String {
    RUNTIME_HEALTH_V1.to_string()
}

pub fn session_control_version() -> String {
    SESSION_CONTROL_V1.to_string()
}

pub fn run_event_ack_version() -> String {
    RUN_EVENT_ACK_V1.to_string()
}

pub fn run_event_gap_version() -> String {
    RUN_EVENT_GAP_V1.to_string()
}

pub fn skill_inventory_version() -> String {
    SKILL_INVENTORY_V1.to_string()
}

pub fn run_monitor_version() -> String {
    RUN_MONITOR_V1.to_string()
}

pub fn task_detail_version() -> String {
    TASK_DETAIL_V1.to_string()
}
