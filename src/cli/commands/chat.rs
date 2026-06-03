//! `maestro chat <subcommand>` — operate on chat sessions from the terminal.

use anyhow::Result;

use crate::cli::util::{
    clean_tag, resolve_session_id, shorten, stream_to_stdout, warn_unknown_model,
};
use crate::cli::ChatCmd;

pub async fn run(c: ChatCmd) -> Result<()> {
    use crate::chat::sessions::{self, Role};

    match c {
        ChatCmd::Ls => {
            let list = sessions::list_sessions()?;
            if list.is_empty() {
                println!("(no sessions — `maestro chat send \"hello\"` to start)");
                return Ok(());
            }
            for s in list {
                let marker = if s.is_current { "*" } else { " " };
                println!(
                    "{} {}  {:<28}  {} msg  {}",
                    marker,
                    &s.id[..8],
                    s.title,
                    s.message_count,
                    s.updated_at.format("%m-%d %H:%M")
                );
            }
            Ok(())
        }
        ChatCmd::New => {
            let s = sessions::Session::new();
            sessions::save(&s)?;
            sessions::set_current(&s.id)?;
            println!("created and switched to {} (\"{}\")", &s.id[..8], s.title);
            Ok(())
        }
        ChatCmd::Switch { id } => {
            let full = resolve_session_id(&id)?;
            sessions::set_current(&full)?;
            let s = sessions::load(&full)?;
            println!("switched to {} (\"{}\")", &full[..8], s.title);
            Ok(())
        }
        ChatCmd::Rm { id } => {
            let full = resolve_session_id(&id)?;
            sessions::delete(&full)?;
            println!("deleted {}", &full[..8]);
            Ok(())
        }
        ChatCmd::Send {
            text,
            model,
            provider,
        } => {
            if let Some(m) = model.as_deref() {
                warn_unknown_model(m);
            }
            let session = sessions::ensure_current()?;
            stream_to_stdout(session, text, model, provider).await
        }
        ChatCmd::Model { id, model } => {
            let full = resolve_session_id(&id)?;
            let mut s = sessions::load(&full)?;
            if !model.trim().is_empty() {
                warn_unknown_model(&model);
            }
            s.cursor_model = if model.trim().is_empty() {
                None
            } else {
                Some(model.trim().to_string())
            };
            s.updated_at = chrono::Utc::now();
            sessions::save(&s)?;
            match &s.cursor_model {
                Some(m) => println!("session {} pinned to {}", &full[..8], m),
                None => println!("session {} cleared model override", &full[..8]),
            }
            Ok(())
        }
        ChatCmd::Show { tail } => {
            let id =
                sessions::read_current()?.ok_or_else(|| anyhow::anyhow!("no current session"))?;
            let session = sessions::load(&id)?;
            if session.messages.is_empty() {
                println!("(no messages — `maestro chat send \"hello\"` to start)");
                return Ok(());
            }
            let start = match tail {
                Some(n) => session.messages.len().saturating_sub(n),
                None => 0,
            };
            println!("# {}  ({})\n", session.title, &session.id[..8]);
            for m in &session.messages[start..] {
                let role = match m.role {
                    Role::User => "you",
                    Role::Assistant => "agent",
                    Role::System => "system",
                };
                let stamp = m.timestamp.format("%H:%M:%S");
                println!("\n── [{role} @ {stamp}] ──");
                println!("{}", m.content);
                for a in &m.actions {
                    println!(
                        "\n  ▸ action [{:?}] {} — {}",
                        a.status.unwrap_or(crate::chat::ActionStatus::Pending),
                        &a.id[..8],
                        a.label
                    );
                }
            }
            Ok(())
        }
        ChatCmd::Brief { id, out, stdout } => {
            let full = match id {
                Some(prefix) => resolve_session_id(&prefix)?,
                None => sessions::read_current()?
                    .ok_or_else(|| anyhow::anyhow!("no current session"))?,
            };
            let session = sessions::load(&full)?;
            if stdout {
                print!("{}", crate::chat::render_continuation_brief(&session));
            } else {
                let path = crate::chat::write_brief(&session, out)?;
                println!("wrote {}", path.display());
            }
            Ok(())
        }
        ChatCmd::Tag { id, tag } => {
            let full = resolve_session_id(&id)?;
            let mut s = sessions::load(&full)?;
            let clean = clean_tag(&tag);
            if clean.is_empty() {
                anyhow::bail!("tag is empty after normalization");
            }
            if !s.tags.iter().any(|t| t == &clean) {
                s.tags.push(clean.clone());
            }
            s.updated_at = chrono::Utc::now();
            sessions::save(&s)?;
            println!("tagged {} #{}", &full[..8], clean);
            Ok(())
        }
        ChatCmd::Untag { id, tag } => {
            let full = resolve_session_id(&id)?;
            let mut s = sessions::load(&full)?;
            let clean = clean_tag(&tag);
            let before = s.tags.len();
            s.tags.retain(|t| t != &clean);
            if s.tags.len() == before {
                println!("(no tag '{}' on {} — nothing to do)", clean, &full[..8]);
            } else {
                s.updated_at = chrono::Utc::now();
                sessions::save(&s)?;
                println!("removed #{} from {}", clean, &full[..8]);
            }
            Ok(())
        }
        ChatCmd::AutoTag { id, all } => {
            let targets: Vec<String> = if all {
                sessions::list_sessions()?
                    .into_iter()
                    .filter(|s| s.tags.is_empty())
                    .map(|s| s.id)
                    .collect()
            } else {
                match id {
                    Some(prefix) => vec![resolve_session_id(&prefix)?],
                    None => {
                        let cur = sessions::read_current()?.ok_or_else(|| {
                            anyhow::anyhow!("no session id given and no current session")
                        })?;
                        vec![cur]
                    }
                }
            };
            if targets.is_empty() {
                println!("(nothing to tag — all sessions already have tags)");
                return Ok(());
            }
            println!(
                "→ tagging {} session(s) (user messages only)",
                targets.len()
            );
            for tid in targets {
                let mut s = sessions::load(&tid)?;
                let title = s.title.clone();
                match crate::chat::auto_tag(&mut s).await {
                    Ok(new) if !new.is_empty() => {
                        println!(
                            "  ✓ {}  \"{}\"  → {}",
                            &tid[..8],
                            shorten(&title, 32),
                            new.iter()
                                .map(|t| format!("#{t}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    Ok(_) => println!("  · {}  no tags returned", &tid[..8]),
                    Err(e) => println!("  ✗ {}  {e}", &tid[..8]),
                }
            }
            Ok(())
        }
        ChatCmd::Repl => {
            use std::io::{BufRead, Write};
            println!("maestro chat REPL — Ctrl-D to exit, blank line to send");
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            loop {
                print!("> ");
                stdout.flush().ok();
                let mut buf = String::new();
                let mut handle = stdin.lock();
                let mut user_msg = String::new();
                while handle.read_line(&mut buf).map(|n| n > 0).unwrap_or(false) {
                    if buf.trim().is_empty() && !user_msg.is_empty() {
                        break;
                    }
                    user_msg.push_str(&buf);
                    buf.clear();
                    print!("| ");
                    stdout.flush().ok();
                }
                if user_msg.trim().is_empty() {
                    return Ok(());
                }
                let session = sessions::ensure_current()?;
                if let Err(e) =
                    stream_to_stdout(session, user_msg.trim().to_string(), None, None).await
                {
                    eprintln!("\n[error: {e:#}]\n");
                }
            }
        }
    }
}
