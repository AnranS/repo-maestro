use std::collections::BTreeMap;

use maestro::adapter::Usage;
use maestro::scheduler::trajectory::{read_trajectory, TrajectoryEventDraft, TrajectoryWriter};
use maestro::schema::trajectory::{Redaction, TrajectoryEventKind, TrajectoryStatus};

#[test]
fn trajectory_writer_appends_schema_v1_events_with_task_local_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = TrajectoryWriter::new(temp.path(), "run-1", "T_trace", "shell").unwrap();

    let first = writer
        .append(TrajectoryEventDraft {
            kind: TrajectoryEventKind::Command,
            tool_name: Some("shell".to_string()),
            command: Some("cargo test".to_string()),
            status: Some(TrajectoryStatus::Started),
            refs: BTreeMap::new(),
            usage: None,
            redaction: Redaction::None,
        })
        .unwrap();
    let second = writer
        .append(TrajectoryEventDraft {
            kind: TrajectoryEventKind::Usage,
            tool_name: None,
            command: None,
            status: Some(TrajectoryStatus::Completed),
            refs: BTreeMap::new(),
            usage: Some(Usage {
                input_tokens: 7,
                output_tokens: 3,
                cost_usd: None,
                model: Some("gpt-5".to_string()),
            }),
            redaction: Redaction::None,
        })
        .unwrap();

    assert_eq!(first.schema_version, "maestro.trajectory_event.v1");
    assert_eq!(first.seq, 1);
    assert_eq!(second.seq, 2);

    let events = read_trajectory(&writer.path()).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, TrajectoryEventKind::Command);
    assert_eq!(events[1].usage.as_ref().unwrap().output_tokens, 3);
}
