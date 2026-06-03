use crate::schema::trajectory::Redaction;

pub fn redact_secret_text(input: &str) -> (String, Redaction) {
    let mut changed = false;
    let mut redact_next = false;
    let mut out = Vec::new();

    for token in input.split_whitespace() {
        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            changed = true;
            continue;
        }

        if let Some(eq) = token.find('=') {
            let key = token[..eq].trim_start_matches('-');
            if is_secret_key(key) {
                out.push(format!("{}=[REDACTED]", &token[..eq]));
                changed = true;
                continue;
            }
        }

        // Only a genuine `--flag` should blank the FOLLOWING token. Without the
        // `starts_with('-')` guard any token that merely *contains* a secret
        // marker (e.g. a URL with `?token=…`) would wrongly blank the next,
        // unrelated token while leaving the real secret intact.
        let flag = token.trim_start_matches('-');
        if token.starts_with('-') && is_secret_key(flag) {
            out.push(token.to_string());
            redact_next = true;
            continue;
        }

        out.push(token.to_string());
    }

    let redaction = if changed {
        Redaction::SecretStripped
    } else {
        Redaction::None
    };
    (out.join(" "), redaction)
}

/// Redact secrets from free-form command **output** before persisting it to a
/// log. Broader than [`redact_secret_text`] (which is tuned for argv shapes):
/// in addition to `KEY=VALUE` and `--flag value`, it blanks credentials
/// embedded in URLs and tokens matching well-known secret prefixes. Unlike
/// `redact_secret_text` it preserves the original whitespace and line structure
/// so logs stay readable.
pub fn redact_secret_blob(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut redact_next = false;
    let mut token_start: Option<usize> = None;
    for (idx, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                let (red, next) = redact_blob_token(&input[start..idx], redact_next);
                out.push_str(&red);
                redact_next = next;
            }
            out.push(ch);
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        let (red, _) = redact_blob_token(&input[start..], redact_next);
        out.push_str(&red);
    }
    out
}

/// Redact a single whitespace-delimited token. Returns the (possibly redacted)
/// text and whether the FOLLOWING token should be blanked (this token was a
/// secret-bearing `--flag`).
fn redact_blob_token(token: &str, redact_prev_flag: bool) -> (String, bool) {
    if redact_prev_flag {
        return ("[REDACTED]".to_string(), false);
    }
    if let Some(eq) = token.find('=') {
        let key = token[..eq].trim_start_matches('-');
        if is_secret_key(key) {
            return (format!("{}=[REDACTED]", &token[..eq]), false);
        }
    }
    if let Some(red) = redact_url_userinfo(token) {
        return (red, false);
    }
    if looks_like_secret_token(token) {
        return ("[REDACTED]".to_string(), false);
    }
    if token.starts_with('-') && is_secret_key(token.trim_start_matches('-')) {
        return (token.to_string(), true);
    }
    (token.to_string(), false)
}

/// `scheme://user:pass@host/…` → `scheme://[REDACTED]@host/…`. Returns `None`
/// when the token has no URL userinfo with a password component.
fn redact_url_userinfo(token: &str) -> Option<String> {
    let scheme_end = token.find("://")?;
    let after = scheme_end + 3;
    let authority_end = token[after..]
        .find(['/', '?', '#'])
        .map(|i| after + i)
        .unwrap_or(token.len());
    let authority = &token[after..authority_end];
    let at = authority.find('@')?;
    if !authority[..at].contains(':') {
        return None;
    }
    let mut out = String::with_capacity(token.len());
    out.push_str(&token[..after]);
    out.push_str("[REDACTED]");
    out.push_str(&token[after + at..]);
    Some(out)
}

/// Heuristic match for opaque secret tokens by well-known prefix (GitHub PATs,
/// Slack, AWS access keys, OpenAI/Anthropic, GitLab, Google API keys).
fn looks_like_secret_token(token: &str) -> bool {
    let t = token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    });
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "xoxs-",
        "glpat-",
        "sk-ant-",
        "sk-",
        "AKIA",
        "ASIA",
        "AIza",
    ];
    PREFIXES
        .iter()
        .any(|p| t.starts_with(p) && t.len() > p.len() + 8)
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "auth_token",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
        "aws_secret_access_key",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_assignment_values() {
        let (redacted, marker) =
            redact_secret_text("AWS_SECRET_ACCESS_KEY=supersecret cargo test --token=abc123");
        assert_eq!(marker, Redaction::SecretStripped);
        assert!(!redacted.contains("supersecret"));
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("AWS_SECRET_ACCESS_KEY=[REDACTED]"));
        assert!(redacted.contains("--token=[REDACTED]"));
    }

    #[test]
    fn redacts_next_arg_for_secret_flags() {
        let (redacted, marker) = redact_secret_text("deploy --password hunter2 --region usw");
        assert_eq!(marker, Redaction::SecretStripped);
        assert_eq!(redacted, "deploy --password [REDACTED] --region usw");
    }

    #[test]
    fn leaves_non_secret_command_unchanged() {
        let (redacted, marker) = redact_secret_text("cargo test --workspace");
        assert_eq!(marker, Redaction::None);
        assert_eq!(redacted, "cargo test --workspace");
    }

    #[test]
    fn flag_branch_does_not_blank_after_url_with_token_substring() {
        // The URL contains "token" but is not a flag, so it must not blank the
        // following unrelated argument.
        let (redacted, _) = redact_secret_text("curl https://h/p?token=abc --retry 3");
        assert!(redacted.contains("--retry 3"));
    }

    #[test]
    fn blob_redacts_url_credentials_and_known_token_prefixes() {
        let input = "cloning https://x-access-token:ghp_abcdefghijklmnop@github.com/o/r.git\nexport TOKEN=ghp_zzzzzzzzzzzzzzzz";
        let out = redact_secret_blob(input);
        assert!(!out.contains("ghp_abcdefghijklmnop"));
        assert!(!out.contains("ghp_zzzzzzzzzzzzzzzz"));
        assert!(out.contains("https://[REDACTED]@github.com/o/r.git"));
        // Newline structure is preserved.
        assert!(out.contains('\n'));
    }

    #[test]
    fn blob_redacts_env_style_secret_assignment() {
        let out = redact_secret_blob("AWS_SECRET_ACCESS_KEY=supersecretvalue\nHOME=/Users/x");
        assert!(!out.contains("supersecretvalue"));
        assert!(out.contains("AWS_SECRET_ACCESS_KEY=[REDACTED]"));
        assert!(out.contains("HOME=/Users/x"));
    }
}
