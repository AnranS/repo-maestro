use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::adapter::Usage;
use crate::paths;
use crate::schema::artifacts::ArtifactRef;
use crate::schema::trajectory::{
    Redaction, TrajectoryEvent, TrajectoryEventKind, TrajectoryStatus,
};

pub fn trajectory_dir(run_dir: &Path) -> PathBuf {
    run_dir.join(paths::TRAJECTORIES_DIR)
}

pub fn trajectory_path(run_dir: &Path, task_id: &str) -> PathBuf {
    trajectory_dir(run_dir).join(format!("{task_id}.ndjson"))
}

#[derive(Debug, Clone)]
pub struct TrajectoryWriter {
    path: PathBuf,
    run_id: String,
    task_id: String,
    provider_id: String,
    next_seq: u64,
}

#[derive(Debug, Clone)]
pub struct TrajectoryEventDraft {
    pub kind: TrajectoryEventKind,
    pub tool_name: Option<String>,
    pub command: Option<String>,
    pub status: Option<TrajectoryStatus>,
    pub refs: BTreeMap<String, ArtifactRef>,
    pub usage: Option<Usage>,
    pub redaction: Redaction,
}

impl TrajectoryEventDraft {
    pub fn new(kind: TrajectoryEventKind) -> Self {
        Self {
            kind,
            tool_name: None,
            command: None,
            status: None,
            refs: BTreeMap::new(),
            usage: None,
            redaction: Redaction::None,
        }
    }
}

impl TrajectoryWriter {
    pub fn new(
        run_dir: &Path,
        run_id: impl Into<String>,
        task_id: impl Into<String>,
        provider_id: impl Into<String>,
    ) -> Result<Self> {
        let task_id = task_id.into();
        let path = trajectory_path(run_dir, &task_id);
        Self::from_path(path, run_id, task_id, provider_id)
    }

    pub fn from_path(
        path: impl Into<PathBuf>,
        run_id: impl Into<String>,
        task_id: impl Into<String>,
        provider_id: impl Into<String>,
    ) -> Result<Self> {
        let path = path.into();
        let task_id = task_id.into();
        paths::validate_path_component("task id", &task_id)?;
        if let Some(parent) = path.parent() {
            paths::ensure_dir(parent)?;
        }
        let next_seq = read_trajectory(&path)?
            .last()
            .map(|event| event.seq.saturating_add(1))
            .unwrap_or(1);
        Ok(Self {
            path,
            run_id: run_id.into(),
            task_id,
            provider_id: provider_id.into(),
            next_seq,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn append(&mut self, draft: TrajectoryEventDraft) -> Result<TrajectoryEvent> {
        let event = TrajectoryEvent {
            schema_version: TrajectoryEvent::SCHEMA_VERSION.to_string(),
            run_id: self.run_id.clone(),
            task_id: self.task_id.clone(),
            seq: self.next_seq,
            timestamp: Utc::now(),
            provider_id: self.provider_id.clone(),
            kind: draft.kind,
            tool_name: draft.tool_name,
            command: draft.command,
            status: draft.status,
            refs: draft.refs,
            usage: draft.usage,
            redaction: draft.redaction,
        };
        self.next_seq = self.next_seq.saturating_add(1);
        let line = serde_json::to_string(&event).context("serialize trajectory event")?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open trajectory file {}", self.path.display()))?;
        writeln!(file, "{line}")
            .with_context(|| format!("append trajectory event {}", self.path.display()))?;
        Ok(event)
    }
}

pub fn read_trajectory(path: &Path) -> Result<Vec<TrajectoryEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut events = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let event: TrajectoryEvent = serde_json::from_str(raw)
            .with_context(|| format!("parse {} line {}", path.display(), idx + 1))?;
        events.push(event);
    }
    Ok(events)
}
