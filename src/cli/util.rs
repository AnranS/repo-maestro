//! Helpers shared across CLI subcommands. Lives in its own file so the
//! command modules can `use crate::cli::util::*` without re-importing each.

use anyhow::Result;

/// A one-line, length-capped title from a run's (possibly multi-sentence /
/// multi-line) spec — for compact list views where the full goal would wrap
/// unreadably. Takes the first non-empty line, capped with an ellipsis.
pub fn spec_title(spec: &str, max: usize) -> String {
    let first = spec
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.chars().count() > max {
        format!(
            "{}…",
            first
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        first.to_string()
    }
}

/// Best-effort warning if the user types a Cursor model id we don't have
/// in the local cache. Tries to suggest the closest known id when there's
/// a plausible typo. Never aborts the CLI — bad ids are pushed through to
/// `cursor-agent` which has its own validation.
pub fn warn_unknown_model(model: &str) {
    if model.trim().is_empty() {
        return;
    }
    let cached = crate::models::load_cached().unwrap_or_default();
    if cached.iter().any(|m| m.id == model) {
        return;
    }
    let suggestion = closest_model(model, &cached);
    eprintln!(
        "  ⚠ unknown model `{}` (not in cached list).{}  Run `maestro models --refresh` if you've added it recently.",
        model,
        suggestion
            .map(|s| format!(" Did you mean `{}`?", s))
            .unwrap_or_default()
    );
}

pub fn closest_model(target: &str, cached: &[crate::models::ModelInfo]) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for m in cached {
        let d = levenshtein(target, &m.id);
        let len = target.len().min(m.id.len());
        let threshold = (len / 3).max(1);
        if d <= threshold {
            match best {
                Some((d2, _)) if d >= d2 => {}
                _ => best = Some((d, &m.id)),
            }
        }
    }
    best.map(|(_, s)| s.to_string())
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

pub fn clean_tag(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
}

pub fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

/// Resolve a (possibly partial) session id to a unique full id. Errors if
/// the prefix matches zero or more-than-one session.
pub fn resolve_session_id(id_prefix: &str) -> Result<String> {
    use crate::chat::sessions;
    let list = sessions::list_sessions()?;
    let matches: Vec<_> = list
        .iter()
        .filter(|s| s.id.starts_with(id_prefix))
        .collect();
    match matches.len() {
        0 => anyhow::bail!("no session matches '{id_prefix}'"),
        1 => Ok(matches[0].id.clone()),
        n => anyhow::bail!("{n} sessions match '{id_prefix}' — use more characters"),
    }
}

pub async fn stream_to_stdout(
    session: crate::chat::Session,
    text: String,
    model_override: Option<String>,
    provider_override: Option<String>,
) -> Result<()> {
    use crate::chat::{stream::send_streaming_with_options, StreamEvent};
    use std::io::Write;
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
    let handle = tokio::spawn(async move {
        send_streaming_with_options(session, text, model_override, provider_override, tx).await
    });

    let mut stdout = std::io::stdout();
    while let Some(ev) = rx.recv().await {
        match ev {
            StreamEvent::Meta { .. } => {}
            StreamEvent::Delta { text } => {
                print!("{text}");
                stdout.flush().ok();
            }
            StreamEvent::Thinking { .. } => {
                // In a terminal we don't want to mix thinking tokens with
                // the visible answer — print a single discreet marker on
                // the first thinking chunk and ignore the rest. Users who
                // want the full trace can run with `--debug` (TBD).
                // For now we just no-op so the terminal stays clean.
            }
            StreamEvent::Done { message } => {
                println!();
                if !message.actions.is_empty() {
                    println!();
                    for a in &message.actions {
                        println!(
                            "  ▸ action [{}]  {}  (pending — confirm in dashboard or `maestro …` directly)",
                            &a.id[..8],
                            a.label
                        );
                    }
                }
            }
            StreamEvent::Error { message } => {
                eprintln!("\n[stream error] {message}");
            }
        }
    }
    handle.await??;
    Ok(())
}

/// Open the system browser pointing at `url`. Best-effort; on unsupported
/// platforms returns an error instead of crashing.
pub fn open_in_browser(url: &str) -> Result<()> {
    use anyhow::Context;
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let cmd = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let cmd = Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let cmd: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("unsupported platform"));
    cmd.context("open browser")?;
    Ok(())
}

/// kebab-case slug suitable for filenames. Keeps lowercase alphanumerics
/// and runs of non-alphanum collapse to a single `-`.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "plan".to_string()
    } else {
        out
    }
}
