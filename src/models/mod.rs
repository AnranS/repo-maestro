//! Cursor model list. We keep a hardcoded fallback so the UI always has
//! something to show even before the user runs `maestro models --refresh`, and
//! cache a fresh list pulled from `cursor-agent models` whenever requested.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCache {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub models: Vec<ModelInfo>,
}

/// Last-resort list shown when `cursor-agent --list-models` hasn't been
/// cached yet AND the live refresh failed (offline, binary missing, etc).
/// Kept short on purpose — once refresh succeeds, the cache wins.
pub const FALLBACK_MODELS_BY_PROVIDER: &[(&str, &[(&str, &str)])] = &[
    (
        "cursor",
        &[
            ("auto", "Auto"),
            ("composer-2-fast", "Composer 2 Fast"),
            ("composer-2", "Composer 2"),
            ("claude-4.5-sonnet", "Sonnet 4.5"),
            ("claude-4.5-sonnet-thinking", "Sonnet 4.5 Thinking"),
            ("claude-4-sonnet", "Sonnet 4"),
            ("gpt-5.2", "GPT-5.2"),
            ("gpt-5.1", "GPT-5.1"),
        ],
    ),
    (
        "codex",
        &[
            ("gpt-5.2", "GPT-5.2"),
            ("gpt-5.1", "GPT-5.1"),
            ("gpt-5-codex", "GPT-5 Codex"),
        ],
    ),
    (
        "claude",
        &[
            ("sonnet", "Claude Sonnet"),
            ("opus", "Claude Opus"),
            ("haiku", "Claude Haiku"),
        ],
    ),
];

pub const MODEL_PROVIDERS: &[&str] = &["cursor", "codex", "claude"];

#[deprecated(note = "use FALLBACK_MODELS_BY_PROVIDER or for_provider(\"cursor\")")]
pub const FALLBACK_MODELS: &[(&str, &str)] = &[
    ("auto", "Auto"),
    ("composer-2-fast", "Composer 2 Fast"),
    ("composer-2", "Composer 2"),
    ("claude-4.5-sonnet", "Sonnet 4.5"),
    ("claude-4.5-sonnet-thinking", "Sonnet 4.5 Thinking"),
    ("claude-4-sonnet", "Sonnet 4"),
    ("gpt-5.2", "GPT-5.2"),
    ("gpt-5.1", "GPT-5.1"),
];

pub fn cache_path() -> Result<PathBuf> {
    cache_path_for("cursor")
}

pub fn cache_path_for(provider: &str) -> Result<PathBuf> {
    let filename = if provider == "cursor" {
        "cursor_models.json".to_string()
    } else {
        format!("{provider}_models.json")
    };
    Ok(paths::maestro_dir()?.join(filename))
}

fn default_provider() -> String {
    "cursor".to_string()
}

pub fn fallback_models_for(provider: &str) -> Vec<ModelInfo> {
    FALLBACK_MODELS_BY_PROVIDER
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, models)| *models)
        .unwrap_or(FALLBACK_MODELS_BY_PROVIDER[0].1)
        .iter()
        .map(|(id, label)| ModelInfo {
            id: (*id).to_string(),
            label: Some((*label).to_string()),
            aliases: vec![],
            provider: Some(provider.to_string()),
        })
        .collect()
}

pub fn fallback_models() -> Vec<ModelInfo> {
    fallback_models_for("cursor")
}

pub fn load_cached() -> Result<Vec<ModelInfo>> {
    for_provider("cursor")
}

pub fn for_provider(provider: &str) -> Result<Vec<ModelInfo>> {
    let p = cache_path_for(provider)?;
    if !p.exists() {
        return Ok(fallback_models_for(provider));
    }
    let text = std::fs::read_to_string(&p)?;
    let cache: ModelCache = serde_json::from_str(&text).unwrap_or(ModelCache {
        provider: provider.to_string(),
        updated_at: chrono::Utc::now(),
        models: vec![],
    });
    if cache.models.is_empty() {
        Ok(fallback_models_for(provider))
    } else {
        Ok(with_provider(cache.models, &cache.provider))
    }
}

/// True iff `cursor_models.json` doesn't exist yet or is empty. Callers use
/// this to decide whether to trigger an automatic refresh on the first
/// `/api/models` hit so the UI lights up without manual intervention.
pub fn cache_missing_or_empty() -> bool {
    cache_missing_or_empty_for("cursor")
}

pub fn cache_missing_or_empty_for(provider: &str) -> bool {
    let p = match cache_path_for(provider) {
        Ok(p) => p,
        Err(_) => return true,
    };
    if !p.exists() {
        return true;
    }
    match std::fs::read_to_string(&p) {
        Ok(text) => {
            let cache: ModelCache = match serde_json::from_str(&text) {
                Ok(c) => c,
                Err(_) => return true,
            };
            cache.models.is_empty()
        }
        Err(_) => true,
    }
}

/// Return cached models if present; otherwise run a live refresh and cache
/// the result. Falls back to the hardcoded list if refresh fails (no network,
/// cursor-agent not installed, etc.) so the UI never renders zero options.
pub async fn load_or_refresh() -> Vec<ModelInfo> {
    load_or_refresh_for("cursor").await
}

pub async fn load_or_refresh_for(provider: &str) -> Vec<ModelInfo> {
    if !cache_missing_or_empty_for(provider) {
        if let Ok(list) = for_provider(provider) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    match refresh_provider(provider).await {
        Ok(list) if !list.is_empty() => list,
        _ => fallback_models_for(provider),
    }
}

pub async fn load_or_refresh_all() -> Vec<ModelInfo> {
    let mut out = Vec::new();
    for provider in MODEL_PROVIDERS {
        out.extend(load_or_refresh_for(provider).await);
    }
    out
}

/// Run `cursor-agent models` and parse its output. The output format is
/// loose — sometimes "name: id" lines, sometimes JSON, sometimes plain ids.
/// We try a few parsers and fall back to a hardcoded list.
pub async fn refresh() -> Result<Vec<ModelInfo>> {
    refresh_provider("cursor").await
}

pub async fn refresh_provider(provider: &str) -> Result<Vec<ModelInfo>> {
    if provider != "cursor" {
        return Ok(fallback_models_for(provider));
    }
    let binary = std::env::var("MAESTRO_CURSOR_AGENT").unwrap_or_else(|_| "cursor-agent".into());
    let output = Command::new(&binary)
        .arg("--list-models")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawn {binary} --list-models"))?;

    if !output.status.success() {
        anyhow::bail!(
            "cursor-agent --list-models exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let models = with_provider(parse_models_output(&stdout), provider);

    let cache = ModelCache {
        provider: provider.to_string(),
        updated_at: chrono::Utc::now(),
        models: models.clone(),
    };
    let _ = paths::maestro_dir().map(|d| paths::ensure_dir(&d));
    let _ = std::fs::write(
        cache_path_for(provider)?,
        serde_json::to_string_pretty(&cache)?,
    );

    Ok(models)
}

pub async fn refresh_all() -> Result<Vec<ModelInfo>> {
    let mut out = Vec::new();
    for provider in MODEL_PROVIDERS {
        match refresh_provider(provider).await {
            Ok(list) if !list.is_empty() => out.extend(list),
            _ => out.extend(fallback_models_for(provider)),
        }
    }
    Ok(out)
}

fn with_provider(mut models: Vec<ModelInfo>, provider: &str) -> Vec<ModelInfo> {
    for model in &mut models {
        model.provider.get_or_insert_with(|| provider.to_string());
    }
    models
}

/// Parse `cursor-agent --list-models` output. Accept JSON arrays/objects, and
/// the plain-text `<id> - <label>` format we observe today, e.g.
///
/// ```text
/// Available models
///
/// auto - Auto
/// composer-2-fast - Composer 2 Fast (current, default)
/// gpt-5.2 - GPT-5.2
/// ...
///
/// Tip: use --model <id> ...
/// ```
fn parse_models_output(stdout: &str) -> Vec<ModelInfo> {
    if let Some(out) = try_parse_json(stdout) {
        if !out.is_empty() {
            return out;
        }
    }
    parse_text_models(stdout)
}

fn try_parse_json(stdout: &str) -> Option<Vec<ModelInfo>> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let arr = value.as_array()?;
    let mut out = vec![];
    for item in arr {
        if let Some(s) = item.as_str() {
            out.push(ModelInfo {
                id: s.to_string(),
                label: None,
                aliases: vec![],
                provider: None,
            });
        } else if let Some(obj) = item.as_object() {
            let id = obj
                .get("id")
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("model"))
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(id) = id {
                out.push(ModelInfo {
                    id,
                    label: obj
                        .get("label")
                        .or_else(|| obj.get("display_name"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    aliases: obj
                        .get("aliases")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    provider: obj
                        .get("provider")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                });
            }
        }
    }
    Some(out)
}

fn parse_text_models(stdout: &str) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = vec![];
    for line in stdout.lines() {
        let cleaned = strip_ansi(line);
        let cleaned = cleaned.trim();
        if cleaned.is_empty() {
            continue;
        }
        // Only accept lines that match the documented "<id> - <label>" shape.
        // This naturally filters out the "Available models" header and the
        // "Tip: use --model …" footer.
        let Some(parsed) = parse_id_label_line(cleaned) else {
            continue;
        };
        if !out.iter().any(|m| m.id == parsed.id) {
            out.push(parsed);
        }
    }
    out
}

/// Parse a single `<id> - <label>` line. `<id>` must be a valid model token
/// (alnum / `.` / `-` / `_`, no spaces). The separator is ` - ` (a hyphen
/// surrounded by spaces). Returns `None` for anything that doesn't fit.
fn parse_id_label_line(line: &str) -> Option<ModelInfo> {
    let sep_idx = line.find(" - ")?;
    let id_part = line[..sep_idx].trim();
    let label_part = line[sep_idx + 3..].trim();
    if id_part.is_empty() || id_part.len() > 80 || label_part.is_empty() {
        return None;
    }
    if !is_model_id(id_part) {
        return None;
    }
    let mut label = label_part.to_string();
    // Strip trailing markers like "(current, default)" that cursor-agent
    // annotates the active model with — we keep the human label clean.
    if let Some(p) = label.find(" (current") {
        label.truncate(p);
    }
    Some(ModelInfo {
        id: id_part.to_string(),
        label: Some(label),
        aliases: vec![],
        provider: None,
    })
}

fn is_model_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must contain a letter and may only use safe id characters.
    let mut has_alpha = false;
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            has_alpha = true;
        }
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
            return false;
        }
    }
    has_alpha
}

fn strip_ansi(s: &str) -> String {
    // Cheap escape-sequence stripper. Matches CSI sequences like \x1b[31m.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until letter
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_OUTPUT: &str = "Available models\n\
        \n\
        auto - Auto\n\
        composer-2-fast - Composer 2 Fast (current, default)\n\
        composer-2 - Composer 2\n\
        gpt-5.3-codex-low - Codex 5.3 Low\n\
        claude-4.5-sonnet - Sonnet 4.5\n\
        claude-4.5-sonnet-thinking - Sonnet 4.5 Thinking\n\
        kimi-k2.5 - Kimi K2.5\n\
        \n\
        Tip: use --model <id> (or /model <id> in interactive mode) to switch.\n";

    #[test]
    fn parses_real_cursor_agent_output() {
        let models = parse_models_output(REAL_OUTPUT);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "auto",
                "composer-2-fast",
                "composer-2",
                "gpt-5.3-codex-low",
                "claude-4.5-sonnet",
                "claude-4.5-sonnet-thinking",
                "kimi-k2.5",
            ]
        );
        let composer = models.iter().find(|m| m.id == "composer-2-fast").unwrap();
        assert_eq!(composer.label.as_deref(), Some("Composer 2 Fast"));
    }

    #[test]
    fn rejects_header_and_footer_lines() {
        assert!(parse_id_label_line("Available models").is_none());
        assert!(parse_id_label_line(
            "Tip: use --model <id> (or /model <id> in interactive mode) to switch."
        )
        .is_none());
        assert!(parse_id_label_line("not-an-id-line just text").is_none());
    }

    #[test]
    fn accepts_dotted_and_versioned_ids() {
        let m =
            parse_id_label_line("gpt-5.3-codex-xhigh-fast - Codex 5.3 Extra High Fast").unwrap();
        assert_eq!(m.id, "gpt-5.3-codex-xhigh-fast");
        assert_eq!(m.label.as_deref(), Some("Codex 5.3 Extra High Fast"));
    }

    #[test]
    fn json_array_path_still_works() {
        let json =
            r#"[{"id":"sonnet-4","label":"Sonnet 4"}, {"id":"opus-4","display_name":"Opus 4"}]"#;
        let models = parse_models_output(json);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "sonnet-4");
        assert_eq!(models[0].label.as_deref(), Some("Sonnet 4"));
        assert_eq!(models[1].label.as_deref(), Some("Opus 4"));
    }

    #[test]
    fn cache_with_provider_field() {
        let legacy = r#"{"updated_at":"2026-05-23T00:00:00Z","models":[{"id":"auto"}]}"#;
        let cache: ModelCache = serde_json::from_str(legacy).unwrap();
        assert_eq!(cache.provider, "cursor");

        let models = with_provider(cache.models, "cursor");
        assert_eq!(models[0].provider.as_deref(), Some("cursor"));
    }

    #[test]
    fn all_provider_fallbacks_are_groupable() {
        let models = MODEL_PROVIDERS
            .iter()
            .flat_map(|provider| fallback_models_for(provider))
            .collect::<Vec<_>>();
        for provider in MODEL_PROVIDERS {
            assert!(
                models
                    .iter()
                    .any(|model| model.provider.as_deref() == Some(*provider)),
                "{provider} should have fallback models"
            );
        }
    }
}
