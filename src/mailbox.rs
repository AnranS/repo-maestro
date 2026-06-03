use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::paths;

pub const MAILBOX_DIR: &str = "mailbox";
pub const MESSAGES_DIR: &str = "messages";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MailStatus {
    Open,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessage {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub subject: String,
    pub body: String,
    /// When true, the recipient is expected to resolve this before proceeding
    /// with its task — surfaced prominently in the delivered prompt and the UI.
    #[serde(default)]
    pub blocking: bool,
    pub status: MailStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// When present, this message is a structured question to the recipient
    /// (typically a human): the Web UI renders it as answerable option buttons,
    /// and an agent reads it as a choice to make. Pairs with `answer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<crate::ask::Ask>,
    /// The selected option key(s) once answered. Set alongside `Resolved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct MailDraft {
    pub from: String,
    pub to: String,
    pub project: Option<String>,
    pub task: Option<String>,
    pub subject: String,
    pub body: String,
    pub blocking: bool,
    /// Optional structured question carried by this message.
    pub ask: Option<crate::ask::Ask>,
}

#[derive(Debug, Clone, Default)]
pub struct MailFilter {
    pub status: Option<MailStatus>,
    pub to: Option<String>,
    pub project: Option<String>,
}

pub struct MailboxStore {
    root: PathBuf,
}

impl MailboxStore {
    pub fn open() -> Result<Self> {
        Self::open_at(paths::maestro_dir()?.join(MAILBOX_DIR))
    }

    pub fn open_at(root: PathBuf) -> Result<Self> {
        paths::ensure_dir(&root)?;
        paths::ensure_dir(&root.join(MESSAGES_DIR))?;
        Ok(Self { root })
    }

    pub fn messages_root(&self) -> PathBuf {
        self.root.join(MESSAGES_DIR)
    }

    pub fn send(&self, draft: MailDraft) -> Result<MailMessage> {
        validate_draft(&draft)?;
        let now = Utc::now();
        let message = MailMessage {
            id: format!(
                "msg-{}-{}",
                now.format("%Y%m%d-%H%M%S"),
                &Uuid::new_v4().to_string()[..8]
            ),
            created_at: now,
            updated_at: now,
            from: draft.from.trim().to_string(),
            to: draft.to.trim().to_string(),
            project: clean_optional(draft.project),
            task: clean_optional(draft.task),
            subject: draft.subject.trim().to_string(),
            body: draft.body.trim().to_string(),
            blocking: draft.blocking,
            status: MailStatus::Open,
            resolution: None,
            ask: draft.ask,
            answer: None,
        };
        self.save(&message)?;
        Ok(message)
    }

    pub fn list(&self, filter: &MailFilter) -> Result<Vec<MailMessage>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.messages_root())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let Ok(message) = self.load_path(&path) else {
                    continue;
                };
                if matches_filter(&message, filter) {
                    out.push(message);
                }
            }
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    pub fn load(&self, id_prefix: &str) -> Result<MailMessage> {
        let path = self.resolve_path(id_prefix)?;
        self.load_path(&path)
    }

    pub fn resolve(&self, id_prefix: &str, note: Option<String>) -> Result<MailMessage> {
        let mut message = self.load(id_prefix)?;
        message.status = MailStatus::Resolved;
        message.updated_at = Utc::now();
        message.resolution = clean_optional(note);
        self.save(&message)?;
        Ok(message)
    }

    /// Answer an `ask` message: record the selected option key(s), set a
    /// human-readable resolution from their labels, and mark it resolved. This
    /// is what unblocks a task waiting on a human decision.
    pub fn answer(&self, id_prefix: &str, keys: &[String]) -> Result<MailMessage> {
        let mut message = self.load(id_prefix)?;
        let resolution = match &message.ask {
            Some(ask) => {
                // Keep only keys that are real options; map to labels for the note.
                let valid: Vec<String> = keys
                    .iter()
                    .filter(|k| ask.options.iter().any(|o| &o.key == *k))
                    .cloned()
                    .collect();
                let labels: Vec<String> = valid
                    .iter()
                    .map(|k| {
                        ask.options
                            .iter()
                            .find(|o| &o.key == k)
                            .map(|o| o.label.clone())
                            .unwrap_or_else(|| k.clone())
                    })
                    .collect();
                message.answer = Some(valid);
                labels.join(", ")
            }
            None => {
                message.answer = Some(keys.to_vec());
                keys.join(", ")
            }
        };
        message.status = MailStatus::Resolved;
        message.updated_at = Utc::now();
        message.resolution = clean_optional(Some(resolution));
        self.save(&message)?;
        Ok(message)
    }

    /// Open messages addressed to any of `identities` (case-insensitive match
    /// on `to`), excluding ones sent *by* one of those identities so an agent
    /// never reads back its own broadcast. `all` / `*` / `everyone` in `to` is
    /// treated as a broadcast to everyone. Returned oldest-first so the reader
    /// follows the order the conversation happened in.
    ///
    /// This is what turns the mailbox from a passive store into a coordination
    /// channel: the scheduler calls it to deliver peer messages into a task's
    /// agent prompt.
    pub fn inbox_for(&self, identities: &[&str]) -> Result<Vec<MailMessage>> {
        let ids: Vec<String> = identities
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = self
            .list(&MailFilter {
                status: Some(MailStatus::Open),
                ..Default::default()
            })?
            .into_iter()
            .filter(|m| {
                let to = m.to.trim().to_lowercase();
                let broadcast = matches!(to.as_str(), "all" | "*" | "everyone");
                let addressed = broadcast || ids.contains(&to);
                let from_self = ids.contains(&m.from.trim().to_lowercase());
                addressed && !from_self
            })
            .collect::<Vec<_>>();
        // `list` returns newest-first; flip to oldest-first for reading order.
        out.reverse();
        Ok(out)
    }

    fn load_path(&self, path: &Path) -> Result<MailMessage> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {:?}", path))?;
        Ok(serde_json::from_str(&text)?)
    }

    fn save(&self, message: &MailMessage) -> Result<()> {
        let path = self.messages_root().join(format!("{}.json", message.id));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(message)?)
            .with_context(|| format!("write {:?}", tmp))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename {:?}", path))?;
        Ok(())
    }

    fn resolve_path(&self, id_prefix: &str) -> Result<PathBuf> {
        let prefix = id_prefix.trim();
        if prefix.is_empty() {
            anyhow::bail!("mailbox id is empty");
        }
        let mut matches = Vec::new();
        for entry in std::fs::read_dir(self.messages_root())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem == prefix || stem.starts_with(prefix) {
                matches.push(path);
            }
        }
        match matches.len() {
            0 => anyhow::bail!("mailbox message {prefix:?} not found"),
            1 => Ok(matches.remove(0)),
            n => anyhow::bail!("mailbox id {prefix:?} is ambiguous ({n} matches)"),
        }
    }
}

/// Render delivered inbox messages as a prompt prelude section, or `None` when
/// there's nothing to deliver (so the caller omits the section entirely).
pub fn render_inbox(messages: &[MailMessage], audience: &str) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let blocking = messages.iter().filter(|m| m.blocking).count();
    let mut s = String::from("## Inbox · messages from other agents\n\n");
    s.push_str(&format!(
        "Unresolved messages other agents addressed to you ({audience}). Read them, act on anything relevant to your task, then close each with the `maestro_mailbox_resolve` tool (pass the message's full id — a truncated prefix can match two same-second ids and error).\n",
    ));
    if blocking > 0 {
        s.push_str(&format!(
            "\n**{blocking} of these are BLOCKING — resolve them before you proceed with your task.**\n",
        ));
    }
    for m in messages {
        let tag = if m.blocking { "⚠ BLOCKING — " } else { "" };
        s.push_str(&format!(
            "\n- [{}] {}from {} — {}\n",
            m.id, tag, m.from, m.subject
        ));
        for line in m.body.lines() {
            s.push_str("  ");
            s.push_str(line);
            s.push('\n');
        }
    }
    Some(s)
}

fn validate_draft(draft: &MailDraft) -> Result<()> {
    if draft.from.trim().is_empty() {
        anyhow::bail!("mailbox send requires --from");
    }
    if draft.to.trim().is_empty() {
        anyhow::bail!("mailbox send requires --to");
    }
    if draft.subject.trim().is_empty() {
        anyhow::bail!("mailbox send requires --subject");
    }
    if draft.body.trim().is_empty() {
        anyhow::bail!("mailbox send requires --body or --file");
    }
    Ok(())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn matches_filter(message: &MailMessage, filter: &MailFilter) -> bool {
    if let Some(status) = filter.status {
        if message.status != status {
            return false;
        }
    }
    if let Some(to) = filter.to.as_deref() {
        if message.to != to {
            return false;
        }
    }
    if let Some(project) = filter.project.as_deref() {
        if message.project.as_deref() != Some(project) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_list_resolve_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = MailboxStore::open_at(dir.path().join("mailbox")).unwrap();
        let message = store
            .send(MailDraft {
                from: "architect".into(),
                to: "frontend".into(),
                project: Some("web".into()),
                task: Some("T_web".into()),
                subject: "Contract ready".into(),
                body: "Use schemas/openapi.yaml".into(),
                blocking: false,
                ask: None,
            })
            .unwrap();

        let open = store
            .list(&MailFilter {
                status: Some(MailStatus::Open),
                to: Some("frontend".into()),
                project: Some("web".into()),
            })
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, message.id);

        let resolved = store
            .resolve(&message.id[..12], Some("Consumed by T_web".into()))
            .unwrap();
        assert_eq!(resolved.status, MailStatus::Resolved);
        assert_eq!(resolved.resolution.as_deref(), Some("Consumed by T_web"));
    }

    #[test]
    fn answer_records_keys_and_label_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let store = MailboxStore::open_at(dir.path().join("mailbox")).unwrap();
        let ask = crate::ask::Ask::single(
            "pick strategy",
            vec![
                crate::ask::AskOption::new("breaking", "Breaking upgrade"),
                crate::ask::AskOption::new("additive", "Additive"),
            ],
            "additive",
        );
        let msg = store
            .send(MailDraft {
                from: "T_core".into(),
                to: "human".into(),
                project: None,
                task: None,
                subject: "strategy?".into(),
                body: "decide".into(),
                blocking: true,
                ask: Some(ask),
            })
            .unwrap();

        let answered = store.answer(&msg.id, &["breaking".to_string()]).unwrap();
        assert_eq!(answered.status, MailStatus::Resolved);
        assert_eq!(
            answered.answer.as_deref(),
            Some(&["breaking".to_string()][..])
        );
        // resolution carries the human-readable label, not the key
        assert_eq!(answered.resolution.as_deref(), Some("Breaking upgrade"));

        // unknown keys are dropped (defensive)
        let msg2 = store
            .send(MailDraft {
                from: "T_core".into(),
                to: "human".into(),
                project: None,
                task: None,
                subject: "again?".into(),
                body: "decide".into(),
                blocking: true,
                ask: Some(crate::ask::Ask::yes_no("ok?", true)),
            })
            .unwrap();
        let a2 = store
            .answer(&msg2.id, &["bogus".to_string(), "yes".to_string()])
            .unwrap();
        assert_eq!(a2.answer.as_deref(), Some(&["yes".to_string()][..]));
    }

    #[test]
    fn empty_body_is_rejected() {
        let err = validate_draft(&MailDraft {
            from: "a".into(),
            to: "b".into(),
            project: None,
            task: None,
            subject: "s".into(),
            body: "  ".into(),
            blocking: false,
            ask: None,
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("--body"));
    }

    fn draft(from: &str, to: &str, subject: &str) -> MailDraft {
        MailDraft {
            from: from.into(),
            to: to.into(),
            project: None,
            task: None,
            subject: subject.into(),
            body: "body".into(),
            blocking: false,
            ask: None,
        }
    }

    #[test]
    fn inbox_delivers_addressed_and_broadcast_but_not_self() {
        let dir = tempfile::tempdir().unwrap();
        let store = MailboxStore::open_at(dir.path().join("mailbox")).unwrap();
        store
            .send(draft("architect", "frontend", "contract ready"))
            .unwrap();
        store
            .send(draft("architect", "all", "code freeze"))
            .unwrap();
        store.send(draft("qa", "backend", "not for you")).unwrap();
        // Sent BY one of our identities — must not echo back to us.
        store
            .send(draft("frontend", "web", "my own broadcast"))
            .unwrap();

        let inbox = store.inbox_for(&["web", "frontend", "T_web"]).unwrap();
        let subjects: Vec<&str> = inbox.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(inbox.len(), 2, "got {subjects:?}");
        assert!(subjects.contains(&"contract ready"));
        assert!(subjects.contains(&"code freeze"));
        assert!(!subjects.contains(&"not for you"));
        assert!(!subjects.contains(&"my own broadcast"));
    }

    #[test]
    fn inbox_skips_resolved_and_renders_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = MailboxStore::open_at(dir.path().join("mailbox")).unwrap();
        let m = store.send(draft("a", "frontend", "s")).unwrap();
        store.resolve(&m.id, None).unwrap();
        assert!(store.inbox_for(&["frontend"]).unwrap().is_empty());
        assert!(render_inbox(&[], "project `web`").is_none());
    }

    #[test]
    fn render_inbox_flags_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let store = MailboxStore::open_at(dir.path().join("mailbox")).unwrap();
        let mut d = draft("architect", "frontend", "freeze");
        d.blocking = true;
        store.send(d).unwrap();
        let inbox = store.inbox_for(&["frontend"]).unwrap();
        let rendered = render_inbox(&inbox, "project `web`").unwrap();
        assert!(rendered.contains("BLOCKING"), "{rendered}");
    }
}
