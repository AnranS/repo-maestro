//! A frontend-agnostic "ask the human" primitive.
//!
//! maestro is autonomous by design — it runs headless in CI and automation —
//! so every human-decision point has to degrade to a default rather than block.
//! This module is the one shape for "present a structured choice and get an
//! answer back":
//!
//!   - typed options (stable `key` + human `label`), single- or multi-select;
//!   - a **mandatory default** returned whenever there's no interactive human
//!     (no TTY, EOF, or — later — a timeout), so a headless run never hangs;
//!   - `Serialize`/`Deserialize` so the very same `Ask` can render in the
//!     terminal here, as buttons in the Web UI, or ride the mailbox to an agent
//!     without changing shape.
//!
//! Orchestrator-initiated decision points (approval gates, the CLI's y/n
//! prompts, replan choices) call [`resolve`] directly. Agent-initiated asks
//! reuse the mailbox blocking gate and render the same `Ask` in the Web UI.

use std::io::{IsTerminal, Write};

use serde::{Deserialize, Serialize};

/// One selectable option. `key` is the stable identifier the caller switches
/// on; `label` is what the human reads. For a plain y/n the two coincide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub key: String,
    pub label: String,
}

impl AskOption {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

/// A structured question: a prompt, ≥2 options, and the keys to fall back to
/// when there is no human to answer. `multi_select` allows choosing several.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ask {
    pub prompt: String,
    pub options: Vec<AskOption>,
    #[serde(default)]
    pub multi_select: bool,
    /// Keys returned when non-interactive (headless / EOF / timeout). Invariant:
    /// every entry must be a valid option key so callers can trust the result.
    pub default: Vec<String>,
}

impl Ask {
    /// A single-select question. `default_key` is returned headless.
    pub fn single(
        prompt: impl Into<String>,
        options: Vec<AskOption>,
        default_key: impl Into<String>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            options,
            multi_select: false,
            default: vec![default_key.into()],
        }
    }

    /// A yes/no question (`yes`/`no` keys), `default_yes` chosen headless.
    pub fn yes_no(prompt: impl Into<String>, default_yes: bool) -> Self {
        Self::single(
            prompt,
            vec![AskOption::new("yes", "Yes"), AskOption::new("no", "No")],
            if default_yes { "yes" } else { "no" },
        )
    }

    fn has_key(&self, key: &str) -> bool {
        self.options.iter().any(|o| o.key == key)
    }
}

/// Resolve an [`Ask`]: prompt interactively when stdin is a TTY, otherwise
/// return the default. Always returns at least the default — never empty,
/// never blocks a headless run.
pub fn resolve(ask: &Ask) -> Vec<String> {
    resolve_from(
        ask,
        &mut std::io::stdin().lock(),
        std::io::stdin().is_terminal(),
    )
}

/// Testable core: read selection lines from `reader`. When `interactive` is
/// false (no TTY) or input ends, fall back to the default.
pub fn resolve_from(
    ask: &Ask,
    reader: &mut impl std::io::BufRead,
    interactive: bool,
) -> Vec<String> {
    let default = sanitized_default(ask);
    if !interactive {
        return default;
    }

    let mut out = std::io::stdout();
    for _attempt in 0..5 {
        let _ = render(ask, &mut out, &default);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return default, // EOF / closed → default
            Ok(_) => {}
        }
        if let Some(selected) = parse_selection(ask, &line) {
            return selected;
        }
        let _ = writeln!(out, "  please pick a listed number or key.");
    }
    default
}

fn sanitized_default(ask: &Ask) -> Vec<String> {
    let valid: Vec<String> = ask
        .default
        .iter()
        .filter(|k| ask.has_key(k))
        .cloned()
        .collect();
    if !valid.is_empty() {
        return valid;
    }
    // Defensive: a caller that forgot a valid default still gets a deterministic
    // answer (the first option) rather than an empty/ambiguous result.
    ask.options
        .first()
        .map(|o| vec![o.key.clone()])
        .unwrap_or_default()
}

fn render(ask: &Ask, out: &mut impl Write, default: &[String]) -> std::io::Result<()> {
    writeln!(out, "{}", ask.prompt)?;
    for (i, opt) in ask.options.iter().enumerate() {
        let mark = if default.contains(&opt.key) {
            " (default)"
        } else {
            ""
        };
        writeln!(out, "  {}) {}{}", i + 1, opt.label, mark)?;
    }
    let hint = if ask.multi_select {
        "select (comma-separated numbers/keys, Enter=default): "
    } else {
        "select (number or key, Enter=default): "
    };
    write!(out, "{hint}")?;
    out.flush()
}

/// Map a raw input line to selected option keys. `None` ⇒ invalid, re-prompt.
/// Empty line ⇒ the default. Accepts 1-based numbers or option keys; for
/// single-select, exactly one token; for multi-select, comma-separated.
fn parse_selection(ask: &Ask, line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Some(sanitized_default(ask));
    }
    let tokens: Vec<&str> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Some(sanitized_default(ask));
    }
    if !ask.multi_select && tokens.len() != 1 {
        return None;
    }
    let mut keys = Vec::new();
    for tok in tokens {
        let key = match tok.parse::<usize>() {
            Ok(n) if n >= 1 && n <= ask.options.len() => ask.options[n - 1].key.clone(),
            Ok(_) => return None, // number out of range
            Err(_) => {
                let lc = tok.to_ascii_lowercase();
                match ask
                    .options
                    .iter()
                    .find(|o| o.key.to_ascii_lowercase() == lc)
                {
                    Some(o) => o.key.clone(),
                    None => return None,
                }
            }
        };
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fruit() -> Ask {
        Ask::single(
            "pick a fruit",
            vec![
                AskOption::new("a", "Apple"),
                AskOption::new("b", "Banana"),
                AskOption::new("c", "Cherry"),
            ],
            "a",
        )
    }

    #[test]
    fn headless_returns_default() {
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(resolve_from(&fruit(), &mut empty, false), vec!["a"]);
    }

    #[test]
    fn picks_by_number_and_key() {
        let mut by_num = std::io::Cursor::new(b"2\n".to_vec());
        assert_eq!(resolve_from(&fruit(), &mut by_num, true), vec!["b"]);
        let mut by_key = std::io::Cursor::new(b"c\n".to_vec());
        assert_eq!(resolve_from(&fruit(), &mut by_key, true), vec!["c"]);
    }

    #[test]
    fn empty_line_is_default() {
        let mut enter = std::io::Cursor::new(b"\n".to_vec());
        assert_eq!(resolve_from(&fruit(), &mut enter, true), vec!["a"]);
    }

    #[test]
    fn invalid_then_valid_reprompts() {
        let mut input = std::io::Cursor::new(b"zzz\n9\n3\n".to_vec());
        assert_eq!(resolve_from(&fruit(), &mut input, true), vec!["c"]);
    }

    #[test]
    fn eof_without_answer_falls_back_to_default() {
        let mut input = std::io::Cursor::new(b"bogus".to_vec()); // no newline, then EOF
        assert_eq!(resolve_from(&fruit(), &mut input, true), vec!["a"]);
    }

    #[test]
    fn multi_select_accepts_several() {
        let mut ask = fruit();
        ask.multi_select = true;
        let mut input = std::io::Cursor::new(b"1,3\n".to_vec());
        assert_eq!(resolve_from(&ask, &mut input, true), vec!["a", "c"]);
    }

    #[test]
    fn single_select_rejects_multiple_tokens() {
        let mut input = std::io::Cursor::new(b"1,2\n2\n".to_vec());
        // first line invalid (single-select), second line picks Banana
        assert_eq!(resolve_from(&fruit(), &mut input, true), vec!["b"]);
    }

    #[test]
    fn yes_no_default() {
        let ask = Ask::yes_no("proceed?", false);
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(resolve_from(&ask, &mut empty, false), vec!["no"]);
    }

    #[test]
    fn invalid_default_falls_back_to_first_option() {
        let ask = Ask {
            prompt: "x".into(),
            options: vec![AskOption::new("a", "A"), AskOption::new("b", "B")],
            multi_select: false,
            default: vec!["nonexistent".into()],
        };
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(resolve_from(&ask, &mut empty, false), vec!["a"]);
    }
}
