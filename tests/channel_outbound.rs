use std::path::PathBuf;

use maestro::channel::{format_event, FormatOptions};
use maestro::config::channels::ChannelEntry;
use maestro::scheduler::events::RunEvent;
use maestro::schema::artifacts::ArtifactSource;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OutboundFixture {
    event: RunEvent,
    entry: ChannelEntry,
    options: FixtureOptions,
}

#[derive(Debug, Deserialize)]
struct FixtureOptions {
    dry_run: bool,
}

impl From<FixtureOptions> for FormatOptions {
    fn from(value: FixtureOptions) -> Self {
        Self {
            dry_run: value.dry_run,
        }
    }
}

#[test]
fn run_started_fixture_formats_small_payload() {
    let fixture = load_fixture("run_event_outbound_run_started.json");
    let reply = format_fixture(fixture).expect("run.started should format");

    assert_eq!(reply.title, "run.started · run-started");
    assert!(reply.body.contains(r#"payload: {"plan":"demo"}"#));
}

#[test]
fn run_completed_fixture_formats_payload() {
    let fixture = load_fixture("run_event_outbound_run_completed.json");
    let reply = format_fixture(fixture).expect("run.completed should format");

    assert_eq!(reply.title, "run.completed · run-completed");
    assert!(reply.body.contains(r#"payload: {"status":"done"}"#));
}

#[test]
fn run_failed_fixture_includes_error_message() {
    let fixture = load_fixture("run_event_outbound_run_failed.json");
    let reply = format_fixture(fixture).expect("run.failed should format");

    assert_eq!(reply.title, "run.failed · run-failed");
    assert!(reply.body.contains("message: build failed"));
}

#[test]
fn dry_prefix_fixture_formats_title() {
    let fixture = load_fixture("run_event_outbound_dry_prefix.json");
    let reply = format_fixture(fixture).expect("dry run.started should format");

    assert_eq!(reply.title, "[DRY] run.started · run-dry");
}

#[test]
fn task_approval_fixture_includes_approval_context() {
    let fixture = load_fixture("run_event_outbound_task_approval.json");
    let reply = format_fixture(fixture).expect("task approval should format");

    assert_eq!(reply.title, "task.approval_required · run-approval");
    assert!(reply.body.contains("task_id: T_approval"));
    assert!(reply.body.contains("approve --confirm"));
}

#[test]
fn evidence_fixture_formats_payload() {
    let fixture = load_fixture("run_event_outbound_evidence.json");
    let reply = format_fixture(fixture).expect("evidence should format");

    assert_eq!(reply.title, "evidence.captured · run-evidence");
    assert!(reply.body.contains(r#"payload: {"artifact":"report"}"#));
}

#[test]
fn attachments_fixture_lifts_refs_in_order() {
    let fixture = load_fixture("run_event_outbound_attachments.json");
    let reply = format_fixture(fixture).expect("attachments should format");

    assert_eq!(reply.attachments.len(), 2);
    assert_eq!(reply.attachments[0].source, ArtifactSource::External);
    assert_eq!(reply.attachments[1].source, ArtifactSource::User);
    assert!(reply.body.contains("attachments: 2"));
}

#[test]
fn kind_filtered_fixture_returns_none() {
    let fixture = load_fixture("run_event_outbound_kind_filtered.json");

    assert_eq!(format_fixture(fixture), None);
}

#[test]
fn oversized_payload_fixture_omits_raw_bytes() {
    let fixture = load_fixture("oversized_outbound_payload.json");
    let payload_bytes = serde_json::to_vec(&fixture.event.payload).unwrap().len();
    assert!(payload_bytes > 1024);

    let reply = format_fixture(fixture).expect("oversized run.started should format");

    assert!(reply.body.contains(&format!(
        "payload omitted ({payload_bytes} bytes > 1 KB cap)"
    )));
    assert!(!reply.body.contains("do-not-leak"));
}

fn format_fixture(fixture: OutboundFixture) -> Option<maestro::channel::OutboundReply> {
    let options = fixture.options.into();
    format_event(&fixture.event, "feishu", &fixture.entry, options)
}

fn load_fixture(name: &str) -> OutboundFixture {
    let path = fixture_path(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("read {}: {err}", path.display());
    });
    serde_json::from_str(&raw).unwrap_or_else(|err| {
        panic!("parse {}: {err}", path.display());
    })
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/channels/feishu")
        .join(name)
}
