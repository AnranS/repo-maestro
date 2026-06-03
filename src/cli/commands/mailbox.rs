//! Local async mailbox for cross-role / cross-project handoffs.

use anyhow::{Context, Result};

use crate::cli::MailboxCmd;
use crate::mailbox::{MailDraft, MailFilter, MailStatus, MailboxStore};

pub fn run(cmd: MailboxCmd) -> Result<()> {
    let store = MailboxStore::open()?;
    match cmd {
        MailboxCmd::Send {
            from,
            to,
            project,
            task,
            subject,
            body,
            file,
            blocking,
        } => {
            let body = read_body(body, file)?;
            let message = store.send(MailDraft {
                from,
                to,
                project,
                task,
                subject,
                body,
                blocking,
                ask: None,
            })?;
            println!("sent {} -> {}  {}", message.from, message.to, message.id);
            Ok(())
        }
        MailboxCmd::Ls { all, to, project } => {
            let filter = MailFilter {
                status: (!all).then_some(MailStatus::Open),
                to,
                project,
            };
            let messages = store.list(&filter)?;
            if messages.is_empty() {
                println!("(mailbox empty)");
                return Ok(());
            }
            println!(
                "{:<24} {:<9} {:<14} {:<14} {:<12} SUBJECT",
                "ID", "STATUS", "FROM", "TO", "PROJECT"
            );
            for message in messages {
                println!(
                    "{:<24} {:<9} {:<14} {:<14} {:<12} {}",
                    shorten_id(&message.id),
                    format!("{:?}", message.status).to_ascii_lowercase(),
                    message.from,
                    message.to,
                    message.project.as_deref().unwrap_or("-"),
                    message.subject
                );
            }
            Ok(())
        }
        MailboxCmd::Show { id } => {
            let message = store.load(&id)?;
            println!("# {}", message.subject);
            println!();
            println!("- id: {}", message.id);
            println!("- status: {:?}", message.status);
            println!("- from: {}", message.from);
            println!("- to: {}", message.to);
            if let Some(project) = &message.project {
                println!("- project: {project}");
            }
            if let Some(task) = &message.task {
                println!("- task: {task}");
            }
            println!("- created_at: {}", message.created_at.to_rfc3339());
            println!("- updated_at: {}", message.updated_at.to_rfc3339());
            if let Some(resolution) = &message.resolution {
                println!("- resolution: {resolution}");
            }
            println!();
            println!("{}", message.body);
            Ok(())
        }
        MailboxCmd::Resolve { id, note } => {
            let message = store.resolve(&id, note)?;
            println!("resolved {}  {}", message.id, message.subject);
            Ok(())
        }
    }
}

fn read_body(body: Option<String>, file: Option<std::path::PathBuf>) -> Result<String> {
    match (body, file) {
        (Some(body), None) => Ok(body),
        (None, Some(file)) => {
            std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))
        }
        (Some(_), Some(_)) => anyhow::bail!("use either --body or --file, not both"),
        (None, None) => anyhow::bail!("mailbox send requires --body or --file"),
    }
}

fn shorten_id(id: &str) -> String {
    id.chars().take(24).collect()
}
