//! Built-in user documentation. The full markdown corpus lives under
//! `docs/site/` and is embedded into the binary at build time via
//! `rust-embed`. The web dashboard renders it through `react-markdown`,
//! the CLI can dump pages straight to stdout.
//!
//! ## i18n
//!
//! `docs/site/index.yaml` declares `languages: [en, zh, ...]`. English
//! pages live at the root (`docs/site/<file>.md`); other languages mirror
//! the tree under `docs/site/<lang>/<file>.md`. Group/page titles can
//! carry per-language overrides via `title_zh:`-style keys. The frontend
//! requests `/api/docs/index?lang=zh` and `/api/docs/page?file=...&lang=zh`
//! to flip the corpus.

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Embeds every file under `docs/site/`. The index lives at `index.yaml`,
/// every page is referenced from there by a relative `file:` field.
#[derive(RustEmbed)]
#[folder = "docs/site/"]
struct DocsAsset;

/// Default language code when none is provided on the API call.
pub const DEFAULT_LANG: &str = "en";

/// Raw index struct deserialised from `index.yaml`. Carries per-language
/// title overrides we'll fold into the response before sending it out.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawIndex {
    pub version: u32,
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    #[serde(default = "default_lang_string")]
    pub default_language: String,
    pub title: String,
    #[serde(default, flatten)]
    pub title_translations: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub description: String,
    pub groups: Vec<RawGroup>,
}

fn default_languages() -> Vec<String> {
    vec![DEFAULT_LANG.to_string()]
}
fn default_lang_string() -> String {
    DEFAULT_LANG.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawGroup {
    pub id: String,
    pub title: String,
    #[serde(default, flatten)]
    pub title_translations: BTreeMap<String, serde_yaml::Value>,
    pub pages: Vec<RawPage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawPage {
    pub id: String,
    pub title: String,
    #[serde(default, flatten)]
    pub title_translations: BTreeMap<String, serde_yaml::Value>,
    pub file: String,
}

/// Public, localised view of the index returned to the API.
#[derive(Debug, Clone, Serialize)]
pub struct DocsIndex {
    pub version: u32,
    pub lang: String,
    pub languages: Vec<String>,
    pub title: String,
    pub description: String,
    pub groups: Vec<DocsGroup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocsGroup {
    pub id: String,
    pub title: String,
    pub pages: Vec<DocsPage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocsPage {
    pub id: String,
    pub title: String,
    pub file: String,
}

/// Resolve `title_<lang>` from an extras map. Falls back to the default
/// English title when no localized variant exists, so partially-translated
/// corpora stay readable instead of going blank.
fn pick_title(
    default_title: &str,
    extras: &BTreeMap<String, serde_yaml::Value>,
    lang: &str,
) -> String {
    if lang == DEFAULT_LANG {
        return default_title.to_string();
    }
    let key = format!("title_{lang}");
    extras
        .get(&key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_title.to_string())
}

fn pick_description(
    default_desc: &str,
    extras: &BTreeMap<String, serde_yaml::Value>,
    lang: &str,
) -> String {
    if lang == DEFAULT_LANG {
        return default_desc.to_string();
    }
    let key = format!("description_{lang}");
    extras
        .get(&key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_desc.to_string())
}

fn load_raw_index() -> Result<RawIndex> {
    let raw = DocsAsset::get("index.yaml")
        .ok_or_else(|| anyhow!("docs/site/index.yaml is missing from the binary"))?;
    let text = std::str::from_utf8(raw.data.as_ref())?;
    let idx: RawIndex = serde_yaml::from_str(text)?;
    Ok(idx)
}

/// Localised index. `lang` is validated against the declared `languages`
/// list and falls back to the default if unknown — never errors on bad
/// input so the UI's language switcher can never break the page.
pub fn load_index(lang: Option<&str>) -> Result<DocsIndex> {
    let raw = load_raw_index()?;
    let resolved = lang
        .filter(|l| raw.languages.iter().any(|x| x == l))
        .unwrap_or(&raw.default_language)
        .to_string();

    let groups = raw
        .groups
        .iter()
        .map(|g| DocsGroup {
            id: g.id.clone(),
            title: pick_title(&g.title, &g.title_translations, &resolved),
            pages: g
                .pages
                .iter()
                .map(|p| DocsPage {
                    id: p.id.clone(),
                    title: pick_title(&p.title, &p.title_translations, &resolved),
                    file: p.file.clone(),
                })
                .collect(),
        })
        .collect();

    Ok(DocsIndex {
        version: raw.version,
        lang: resolved.clone(),
        languages: raw.languages,
        title: pick_title(&raw.title, &raw.title_translations, &resolved),
        description: pick_description(&raw.description, &raw.title_translations, &resolved),
        groups,
    })
}

/// Look up a page's markdown body for the requested language. We
/// intentionally disallow `..` and absolute paths so the embed boundary
/// can't be escaped from the API. When `lang != en` we try the localised
/// file first (`<lang>/<file>`) and silently fall back to English if it
/// hasn't been translated yet — partial translations stay usable.
pub fn load_page(file: &str, lang: Option<&str>) -> Result<String> {
    if file.is_empty() || file.contains("..") || file.starts_with('/') {
        return Err(anyhow!("invalid docs path: {file}"));
    }

    let lang = lang.unwrap_or(DEFAULT_LANG);
    if lang != DEFAULT_LANG {
        let localised = format!("{lang}/{file}");
        if let Some(asset) = DocsAsset::get(&localised) {
            return Ok(std::str::from_utf8(asset.data.as_ref())?.to_string());
        }
    }
    let asset = DocsAsset::get(file).ok_or_else(|| anyhow!("docs page not found: {file}"))?;
    Ok(std::str::from_utf8(asset.data.as_ref())?.to_string())
}

/// Convenience iterator used by `maestro doc list` to print a flat catalog.
pub fn iter_pages(idx: &DocsIndex) -> impl Iterator<Item = (&str, &DocsPage)> {
    idx.groups
        .iter()
        .flat_map(|g| g.pages.iter().map(move |p| (g.title.as_str(), p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_index_loads_with_every_page() {
        let idx = load_index(None).expect("index.yaml parses");
        assert_eq!(idx.lang, "en");
        assert!(!idx.groups.is_empty());
        let mut total = 0usize;
        for (_, page) in iter_pages(&idx) {
            load_page(&page.file, None)
                .unwrap_or_else(|e| panic!("page {} should load: {e}", page.file));
            total += 1;
        }
        assert!(
            total >= 10,
            "expected a comprehensive docs corpus, got {total}"
        );
    }

    #[test]
    fn unknown_lang_falls_back_to_default() {
        let idx = load_index(Some("klingon")).expect("falls back");
        assert_eq!(idx.lang, "en");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(load_page("", None).is_err());
        assert!(load_page("../Cargo.toml", None).is_err());
        assert!(load_page("/etc/passwd", None).is_err());
    }

    #[test]
    fn zh_falls_back_to_en_when_page_missing() {
        // Use a real english page that may not yet have a zh translation.
        let body = load_page("51-faq.md", Some("zh")).expect("falls back");
        assert!(!body.is_empty());
    }
}
