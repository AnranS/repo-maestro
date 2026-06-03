use anyhow::{Context, Result};
use std::path::PathBuf;

use super::sessions::{self, Message, Role, Session};

const DEFAULT_RECENT_MESSAGES: usize = 8;
const MAX_MESSAGE_CHARS: usize = 1_800;

pub fn render_brief(session: &Session) -> String {
    let mut out = String::new();
    out.push_str("# Continuation brief\n\n");
    out.push_str(&format!("- session: {}\n", session.id));
    out.push_str(&format!("- title: {}\n", session.title));
    out.push_str(&format!(
        "- updated_at: {}\n",
        session.updated_at.to_rfc3339()
    ));
    if !session.tags.is_empty() {
        out.push_str(&format!("- tags: {}\n", session.tags.join(", ")));
    }
    if let Some(model) = &session.cursor_model {
        out.push_str(&format!("- model: {model}\n"));
    }
    out.push('\n');

    if let Some(summary) = latest_compaction_summary(session) {
        out.push_str("## Existing Summary\n\n");
        out.push_str(summary.content.trim());
        out.push_str("\n\n");
    }

    let pending = pending_actions(session);
    if !pending.is_empty() {
        out.push_str("## Pending Actions\n\n");
        for action in pending {
            out.push_str(&format!(
                "- [{:?}] {} `{}`\n",
                action
                    .status
                    .unwrap_or(super::actions::ActionStatus::Pending),
                action.label,
                action.id
            ));
        }
        out.push('\n');
    }

    out.push_str("## Recent Messages\n\n");
    for message in recent_messages(session, DEFAULT_RECENT_MESSAGES) {
        out.push_str(&format!(
            "### {} @ {}\n\n",
            role_label(message.role),
            message.timestamp.to_rfc3339()
        ));
        out.push_str(&truncate(&message.content, MAX_MESSAGE_CHARS));
        out.push_str("\n\n");
    }

    out.push_str("## Continue From Here\n\n");
    out.push_str("- Preserve decisions already captured above unless the user explicitly changes direction.\n");
    out.push_str("- Re-check any pending action output before claiming that work has run.\n");
    out.push_str("- Prefer `maestro-action` blocks for commands the user should confirm.\n");
    out
}

pub fn write_brief(session: &Session, out: Option<PathBuf>) -> Result<PathBuf> {
    let path = match out {
        Some(path) => path,
        None => {
            let dir = sessions::chat_dir()?.join("continuations");
            crate::paths::ensure_dir(&dir)?;
            dir.join(format!("{}.brief.md", session.id))
        }
    };
    if let Some(parent) = path.parent() {
        crate::paths::ensure_dir(parent)?;
    }
    std::fs::write(&path, render_brief(session)).with_context(|| format!("write {:?}", path))?;
    Ok(path)
}

fn latest_compaction_summary(session: &Session) -> Option<&Message> {
    session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::System && m.content.contains("Conversation summary"))
}

fn pending_actions(session: &Session) -> Vec<&super::actions::Action> {
    session
        .messages
        .iter()
        .flat_map(|m| m.actions.iter())
        .filter(|a| {
            matches!(
                a.status.unwrap_or(super::actions::ActionStatus::Pending),
                super::actions::ActionStatus::Pending
                    | super::actions::ActionStatus::Running
                    | super::actions::ActionStatus::Failed
            )
        })
        .collect()
}

fn recent_messages(session: &Session, limit: usize) -> &[Message] {
    let start = session.messages.len().saturating_sub(limit);
    &session.messages[start..]
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::System => "System",
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.trim().to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("\n\n[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::actions::{Action, ActionStatus, ActionVerb};
    use std::collections::BTreeMap;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            content: content.into(),
            timestamp: chrono::Utc::now(),
            actions: vec![],
            thinking: None,
        }
    }

    #[test]
    fn brief_includes_summary_recent_messages_and_pending_actions() {
        let mut session = Session::new();
        session.title = "Implement search".into();
        session.messages.push(msg(
            Role::System,
            "## Conversation summary (compacted 10 earlier messages)\n\n- Search uses ripgrep.",
        ));
        session.messages.push(msg(Role::User, "please continue"));
        let mut assistant = msg(Role::Assistant, "ready");
        assistant.actions.push(Action {
            id: "act-1".into(),
            verb: ActionVerb::Run,
            args: BTreeMap::new(),
            status: Some(ActionStatus::Pending),
            label: "maestro run plans/search.yaml".into(),
            output: None,
        });
        session.messages.push(assistant);

        let brief = render_brief(&session);
        assert!(brief.contains("Implement search"));
        assert!(brief.contains("Search uses ripgrep"));
        assert!(brief.contains("maestro run plans/search.yaml"));
        assert!(brief.contains("please continue"));
    }
}
