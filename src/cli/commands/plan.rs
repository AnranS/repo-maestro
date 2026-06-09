//! `maestro plan validate` plus the internal planner used by `maestro work`.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cli::util::slugify;
use crate::cli::PlanCmd;
use crate::config::{Plan, Project, ProjectsConfig};
use crate::paths;
use crate::schema::preview::{codes, Issue, PlanPreview};

pub fn run(c: PlanCmd) -> Result<()> {
    match c {
        PlanCmd::Validate { plan, json } => validate(plan, json),
        PlanCmd::Hash { plan } => hash(plan),
    }
}

fn validate(plan_path: std::path::PathBuf, json: bool) -> Result<()> {
    if json {
        return validate_json(&plan_path);
    }
    // Human path: full load (validates + bails on structural errors), then text.
    let p = Plan::load(&plan_path)?;
    let projects = load_projects_or_empty();
    let report = crate::config::analyze(&p, &projects);
    println!("→ {} task(s)", p.tasks.len());
    if report.findings.is_empty() {
        println!("ok · 0 errors · 0 warnings");
        return Ok(());
    }
    for f in &report.findings {
        match f {
            crate::config::Finding::Error { task, message, .. } => {
                println!("  ✗ {} {}", task.as_deref().unwrap_or("(plan)"), message);
            }
            crate::config::Finding::Warning { task, message, .. } => {
                println!("  ⚠ {} {}", task.as_deref().unwrap_or("(plan)"), message);
            }
        }
    }
    println!(
        "\n{} error(s), {} warning(s)",
        report.error_count(),
        report.warning_count()
    );
    if report.has_errors() {
        std::process::exit(2);
    }
    Ok(())
}

/// `plan validate --json`: stdout is ALWAYS a valid `PlanPreview`. A read/parse
/// failure ⇒ `plan.parse_failed`; a *structural* validate failure ⇒ its own code
/// (`plan.self_dependency` / `plan.cycle` / `plan.task_id_traversal` / …) — and we
/// do NOT run `analyze` on a structurally-invalid plan (it could choke on a
/// cyclic one). All human detail (incl. paths) goes to stderr, never the JSON.
fn validate_json(plan_path: &Path) -> Result<()> {
    let p = match Plan::read_only(plan_path) {
        Ok(p) => p,
        Err(e) => {
            let preview = PlanPreview::unparseable(Issue::error(
                codes::PARSE_FAILED,
                "could not parse PLAN.yaml",
            ));
            println!("{}", preview.to_json());
            eprintln!("plan validate: {e:#}");
            std::process::exit(2);
        }
    };
    if let Some(issue) = p.first_validation_issue() {
        let mut preview = crate::config::analyze::plan_preview_core(&p);
        preview.errors.push(issue);
        println!("{}", preview.to_json());
        std::process::exit(2);
    }
    // Structurally valid: enrich with analyze findings (unknown_project, …).
    let report = crate::config::analyze(&p, &load_projects_or_empty());
    let preview = crate::config::analyze::plan_preview(&p, &report);
    println!("{}", preview.to_json());
    if !preview.is_valid() {
        std::process::exit(2);
    }
    Ok(())
}

fn load_projects_or_empty() -> ProjectsConfig {
    paths::projects_file()
        .ok()
        .and_then(|f| if f.exists() { Some(f) } else { None })
        .and_then(|f| ProjectsConfig::load(&f).ok())
        .unwrap_or_else(|| ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        })
}

// `plan_preview` / `plan_preview_core` now live in `crate::config::analyze` so
// non-CLI callers (server, F-118 runtime health) reuse them without depending on
// a CLI command module. Call them via `crate::config::analyze::plan_preview`.

fn hash(plan_path: std::path::PathBuf) -> Result<()> {
    let hash = crate::file_guard::file_hash(&plan_path)?;
    println!("{hash}  {}", plan_path.display());
    println!();
    println!("maestro-action:");
    println!("  plan_hash: {hash}");
    Ok(())
}

pub(crate) fn synthesize_file(
    spec: &str,
    out: Option<PathBuf>,
    selected: Vec<String>,
    root_filter: Option<&Path>,
) -> Result<PathBuf> {
    synthesize_file_with_intent(
        spec,
        out,
        selected,
        SynthesisIntent::Change,
        root_filter,
        false,
    )
}

/// Like [`synthesize_file`] but routes the human "goal matched …" line to stderr,
/// so `work --dry --json` can keep stdout reserved for the `PlanPreview`.
pub(crate) fn synthesize_file_quiet(
    spec: &str,
    out: Option<PathBuf>,
    selected: Vec<String>,
    root_filter: Option<&Path>,
) -> Result<PathBuf> {
    synthesize_file_with_intent(
        spec,
        out,
        selected,
        SynthesisIntent::Change,
        root_filter,
        true,
    )
}

pub(crate) fn synthesize_audit_file(
    spec: &str,
    out: Option<PathBuf>,
    selected: Vec<String>,
) -> Result<PathBuf> {
    synthesize_file_with_intent(spec, out, selected, SynthesisIntent::Audit, None, false)
}

fn synthesize_file_with_intent(
    spec: &str,
    out: Option<PathBuf>,
    selected: Vec<String>,
    intent: SynthesisIntent,
    root_filter: Option<&Path>,
    quiet: bool,
) -> Result<PathBuf> {
    let pfile = paths::projects_file()?;
    if !pfile.exists() {
        anyhow::bail!("run `maestro init` and register projects first");
    }
    let projects = ProjectsConfig::load(&pfile)?;
    if projects.projects.is_empty() {
        anyhow::bail!("{}", empty_project_registry_message(&pfile));
    }
    let projects = if let Some(root) = root_filter {
        let scoped = projects_scoped_to_root(&projects, root)?;
        if scoped.projects.is_empty() {
            anyhow::bail!("no registered projects under --root {}", root.display());
        }
        scoped
    } else {
        projects
    };

    let path_tokens = goal_path_tokens(spec);
    let mut notices: Vec<String> = Vec::new();
    let projects = if !selected.is_empty() {
        // Explicit `--project` selection: filtered below by selected_projects.
        projects
    } else if matches!(intent, SynthesisIntent::Audit) {
        // F-AUDIT-001: an audit ("verify the discovered dependency DAG") must
        // cover EVERY project. It must NOT go through work's goal-relevance /
        // path-token narrowing — the root path and the boilerplate audit spec
        // text would otherwise silently shrink a 65-project audit to the one
        // project whose name happens to appear. Keep the whole registry, and
        // emit no granularity notice (an audit isn't "edit this file/module").
        projects
    } else {
        // Change intent: narrow to the projects the goal is actually about.
        if let Some(n) = planner_granularity_notice(&path_tokens) {
            notices.push(n);
        }
        let all: BTreeSet<String> = projects.projects.keys().cloned().collect();
        let mut relevant = projects_relevant_to_goal(&projects, spec, &path_tokens)?;

        // F-101: when the goal uses topology wording ("depends on",
        // "consumers of", "downstream of", "blast radius of", "impacted
        // by"), the lexical relevance pass alone under-scopes — it only
        // catches the producer the user named, never the downstream
        // consumers they actually want changed. Walk the reverse
        // dependency graph and add the transitive consumer closure.
        if let Some(phrase) = topology_wording_in_spec(spec) {
            // `relevant` may already be the full projects map when no
            // lexical match fired — in that case we don't have a clean
            // anchor set. Anchor set is "lexical matches that are also
            // a strict subset of `all`", which is the same as
            // `relevant.projects` IFF lexical narrowing happened.
            let lexical_narrowed = relevant.projects.len() < all.len();
            let anchors: BTreeSet<String> = if lexical_narrowed {
                relevant.projects.keys().cloned().collect()
            } else {
                BTreeSet::new()
            };
            if anchors.is_empty() {
                // Topology phrase detected but no project anchor matched
                // lexically — leave the fallback set as-is and note that
                // text relevance carried the load. Avoids silently
                // claiming a topology expansion happened when it didn't.
                notices.push(format!(
                    "goal uses topology wording (`{phrase}`) but no anchor project matched by name; fell back to text relevance.",
                ));
            } else {
                let downstream = expand_topology_downstream(&projects, &anchors);
                if downstream.is_empty() {
                    // Anchors are leaf projects (nothing depends on
                    // them). Common on a workspace where the user said
                    // "blast radius of <app>" and <app> is itself a
                    // consumer. Surface it instead of pretending we
                    // expanded.
                    notices.push(format!(
                        "goal uses topology wording (`{phrase}`); anchor project(s) have no consumers in the dependency graph, so the plan covers the anchor(s) only.",
                    ));
                } else {
                    // ADD the downstream projects to `relevant`. The
                    // lexical pass had already dropped them, so they
                    // aren't in `relevant.projects` — we have to pull
                    // them from the outer `projects` map (still the
                    // unfiltered registry inside this `if selected
                    // .is_empty()` arm).
                    for name in &downstream {
                        if let Some(p) = projects.projects.get(name) {
                            relevant.projects.insert(name.clone(), p.clone());
                        }
                    }
                    let anchor_list: Vec<&str> = anchors.iter().map(String::as_str).collect();
                    notices.push(format!(
                        "topology scope: included {} downstream project(s) from dependency graph for `{}` (matched on `{phrase}`).",
                        downstream.len(),
                        anchor_list.join(", "),
                    ));
                }
            }
        }

        // Make any narrowing visible — silently dropping projects the goal
        // didn't name is a nasty surprise (see self-test). Tell the user which
        // were kept/skipped and how to force-include.
        if relevant.projects.len() < all.len() {
            let kept: Vec<&str> = relevant.projects.keys().map(String::as_str).collect();
            let dropped: Vec<&str> = all
                .iter()
                .filter(|n| !relevant.projects.contains_key(*n))
                .map(String::as_str)
                .collect();
            // F-102: when there are many more projects than will reasonably fit
            // on one stdout line, summarize the skipped list. Threshold of 12
            // keeps small-workspace UX unchanged (still full list) and stops
            // the multi-hundred-character one-paragraph dump on 50+ project
            // monorepos.
            let dropped_str = format_skipped_list(&dropped, 12);
            let line = format!(
                "→ goal matched {}/{} project(s): {}. Skipped (didn't match the goal): {}. Add `--project <id>` to force-include.",
                kept.len(),
                all.len(),
                kept.join(", "),
                dropped_str,
            );
            // --json keeps stdout for the PlanPreview; the human line goes to stderr.
            if quiet {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        relevant
    };
    // Compose all notices into one yaml-safe block; empty when none fired.
    let notice = if notices.is_empty() {
        None
    } else {
        Some(notices.join(" "))
    };

    let selected = selected_projects(&projects, selected)?;
    let out_path = default_plan_path(spec, out);
    ensure_parent(&out_path)?;
    let plan = render_synthesized_plan(
        spec,
        &projects,
        &selected,
        Some(&out_path),
        intent,
        notice.as_deref(),
    );
    std::fs::write(&out_path, plan).with_context(|| format!("write {}", out_path.display()))?;
    Ok(out_path)
}

fn projects_scoped_to_root(projects: &ProjectsConfig, root: &Path) -> Result<ProjectsConfig> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize root filter {}", root.display()))?;
    let mut scoped = projects.clone();
    scoped
        .projects
        .retain(|_, project| project_path_starts_with(&project.path, &root).unwrap_or(false));
    Ok(scoped)
}

fn project_path_starts_with(project_path: &str, root: &Path) -> Result<bool> {
    let expanded = paths::expand(project_path)?;
    let path = if expanded.is_absolute() {
        expanded
    } else {
        paths::workspace_root()?.join(expanded)
    };
    let path = path.canonicalize().unwrap_or(path);
    Ok(path.starts_with(root))
}

fn empty_project_registry_message(pfile: &std::path::Path) -> String {
    let mut message = format!(
        "no projects registered in {}\n  next: maestro work \"<goal>\" --root <path>\n  or:   maestro init --analyze --root <path> --agent mock\n  cleanup: remove .maestro/ if this empty workspace was initialized by mistake",
        pfile.display()
    );
    let tmp_path = crate::config::projects::projects_tmp_path(pfile);
    if tmp_path.exists() {
        message.push_str(&format!(
            "\n  note: sibling {} exists — another maestro process may be writing the registry concurrently. Retry the command in a moment, or run `maestro doctor` to inspect the workspace.",
            tmp_path.display()
        ));
    }
    message
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SynthesisIntent {
    Change,
    Audit,
}

fn default_plan_path(spec: &str, out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let slug = bounded_plan_slug(spec);
        paths::workspace_root()
            .unwrap_or_default()
            .join("plans")
            .join(format!("{date}-{slug}.yaml"))
    })
}

fn bounded_plan_slug(spec: &str) -> String {
    const MAX_SLUG_CHARS: usize = 72;
    let slug = slugify(spec);
    if slug.len() <= MAX_SLUG_CHARS {
        return slug;
    }

    let hash = crate::file_guard::stable_hash_bytes(spec.as_bytes());
    let hash = hash
        .trim_start_matches("fnv1a64:")
        .chars()
        .take(8)
        .collect::<String>();
    let keep = MAX_SLUG_CHARS.saturating_sub(hash.len() + 1);
    let mut prefix = slug.chars().take(keep).collect::<String>();
    while prefix.ends_with('-') {
        prefix.pop();
    }
    if prefix.is_empty() {
        format!("plan-{hash}")
    } else {
        format!("{prefix}-{hash}")
    }
}

fn ensure_parent(path: &std::path::Path) -> Result<()> {
    if let Some(p) = path.parent() {
        paths::ensure_dir(p)?;
    }
    Ok(())
}

fn selected_projects(projects: &ProjectsConfig, selected: Vec<String>) -> Result<BTreeSet<String>> {
    if selected.is_empty() {
        return Ok(projects.projects.keys().cloned().collect());
    }
    let mut out = BTreeSet::new();
    for name in selected {
        if !projects.projects.contains_key(&name) {
            anyhow::bail!("project {name:?} is not registered");
        }
        out.insert(name);
    }
    Ok(out)
}

fn goal_path_tokens(spec: &str) -> Vec<String> {
    spec.split_whitespace()
        .filter_map(clean_goal_token)
        .filter(|token| {
            token.contains('/') || token.rsplit('/').next().is_some_and(|s| s.contains('.'))
        })
        .map(|token| token.to_ascii_lowercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// True when `word` is a standalone whitespace-delimited word in the goal
/// (edges trimmed of punctuation). Splitting on whitespace — not every
/// non-alphanumeric — is deliberate: "update api and web" matches `api`/`web`,
/// but a path token like "src/cli/commands" stays one word and must NOT match a
/// `cli` project (that path belongs to whichever project contains it).
fn goal_mentions_word(spec_lower: &str, word: &str) -> bool {
    spec_lower
        .split_whitespace()
        .any(|raw| raw.trim_matches(|c: char| !c.is_ascii_alphanumeric()) == word)
}

/// Like `goal_mentions_word` but tolerant of a trailing-`s` plural on either
/// side, so a goal "webhook senders" matches the name-word "sender".
fn goal_mentions_word_or_plural(spec_lower: &str, word: &str) -> bool {
    spec_lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .any(|g| {
            g == word || g.strip_suffix('s') == Some(word) || word.strip_suffix('s') == Some(g)
        })
}

/// The matchable words in a project name (snake/kebab/slash split, ≥2 chars),
/// lowercased. Shared by relevance matching and document-frequency scoring.
fn name_words_of(name: &str) -> Vec<String> {
    name.to_ascii_lowercase()
        .split(['-', '_', '/'])
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect()
}

/// Order-independent relevance from name-word document frequencies. A name-word
/// UNIQUE across the workspace (df == 1, e.g. "comment", "search") matches the
/// project on its own; otherwise ≥2 moderately-rare words (df <= `rare_df`,
/// e.g. "webhook" + "sender") must co-occur in the goal. Ubiquitous tokens
/// (df > `rare_df`, e.g. "listener", "platform"), generic basenames, and words
/// that are only goal path-segments never count — so this never over-widens.
fn name_matches_by_discriminative_words(
    name_words: &[&str],
    spec_lower: &str,
    word_df: &HashMap<String, usize>,
    rare_df: usize,
    path_words: &HashSet<String>,
) -> bool {
    let mut rare_hits = 0;
    for &w in name_words {
        if is_generic_basename(w)
            || path_words.contains(w)
            || !goal_mentions_word_or_plural(spec_lower, w)
        {
            continue;
        }
        match word_df.get(w).copied().unwrap_or(0) {
            1 => return true,
            df if df <= rare_df => rare_hits += 1,
            _ => {}
        }
    }
    rare_hits >= 2
}

/// Generic leaf-folder names too common to be a reliable relevance signal on
/// their own (a goal mentioning "test" or "lib" shouldn't pull every project
/// whose folder happens to be named that). Domain-ish names like api/web/ui are
/// intentionally NOT here — those are useful informal references.
fn is_generic_basename(base: &str) -> bool {
    matches!(
        base,
        "test"
            | "tests"
            | "lib"
            | "libs"
            | "src"
            | "app"
            | "apps"
            | "core"
            | "common"
            | "util"
            | "utils"
            | "deploy"
            | "build"
            | "dist"
            | "main"
            | "demo"
            | "example"
            | "examples"
            | "shared"
            | "internal"
            | "pkg"
            | "cmd"
            | "packages"
    )
}

/// True when `words` appear as a contiguous run in the goal's word stream
/// (the goal split on any non-alphanumeric). Lets a snake/kebab project name
/// match the same words written with spaces.
fn goal_contains_phrase(spec_lower: &str, words: &[&str]) -> bool {
    if words.is_empty() {
        return false;
    }
    let goal: Vec<&str> = spec_lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    goal.len() >= words.len() && goal.windows(words.len()).any(|w| w == words)
}

fn clean_goal_token(raw: &str) -> Option<String> {
    let token = raw.trim_matches(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.'))
    });
    (!token.is_empty() && token != ".").then(|| token.to_string())
}

fn planner_granularity_notice(path_tokens: &[String]) -> Option<String> {
    (!path_tokens.is_empty()).then(|| {
        format!(
            "goal references `{}`, but planner is project-level; tasks below cover matching projects, not individual files or modules.",
            path_tokens.join("`, `")
        )
    })
}

/// Render a "skipped projects" list for the `→ goal matched ...`
/// stdout line. When the list is short (≤ `max_inline`), the full names
/// are joined inline as before. Otherwise we keep the first `max_inline`
/// names and replace the tail with a count, so a 60-project monorepo
/// where 58 didn't match produces a short summary instead of a
/// 600-character paragraph.
fn format_skipped_list(names: &[&str], max_inline: usize) -> String {
    if names.len() <= max_inline {
        return names.join(", ");
    }
    let head: Vec<&str> = names.iter().take(max_inline).copied().collect();
    format!(
        "{} skipped (first {max_inline}: {})",
        names.len(),
        head.join(", ")
    )
}

/// Returns the first topology-scoped phrase the goal uses, if any.
/// Matching is case-insensitive whole-phrase. Returning the phrase
/// itself (rather than a bool) lets the caller mention it back in the
/// notice so the user sees what tripped the expansion.
fn topology_wording_in_spec(spec: &str) -> Option<&'static str> {
    // Order: longer / more specific first so "blast radius of" wins over
    // a "of" inside a longer match.
    const PHRASES: &[&str] = &[
        "blast radius of",
        "downstream of",
        "consumers of",
        "consumer of",
        "impacted by",
        "depends on",
        "depend on",
    ];
    let lower = spec.to_ascii_lowercase();
    PHRASES.iter().copied().find(|p| lower.contains(p))
}

/// Given a set of anchor project names, returns the transitive closure
/// of every project that (directly or transitively) **depends on** any
/// anchor. The closure does not include the anchors themselves — the
/// caller is responsible for unioning that back in if they want both
/// "the thing" and "everything downstream of the thing".
///
/// Edges come from `projects.yaml.dependencies`. After commit `4f94b94`
/// these are populated from both the discovery manifest pass AND the
/// codegraph fold-in, so the reverse graph is honest for both declared
/// and import-derived dependencies.
fn expand_topology_downstream(
    projects: &ProjectsConfig,
    anchors: &BTreeSet<String>,
) -> BTreeSet<String> {
    // Build reverse edges once: producer -> set of direct consumers.
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, project) in &projects.projects {
        for dep in &project.dependencies {
            reverse.entry(dep.as_str()).or_default().push(name.as_str());
        }
    }
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = anchors.iter().cloned().collect();
    while let Some(producer) = stack.pop() {
        let Some(consumers) = reverse.get(producer.as_str()) else {
            continue;
        };
        for &consumer in consumers {
            if anchors.contains(consumer) {
                continue; // never re-add an anchor as its own consumer
            }
            if out.insert(consumer.to_string()) {
                // Newly added — descend further so the closure is transitive.
                stack.push(consumer.to_string());
            }
        }
    }
    out
}

fn projects_relevant_to_goal(
    projects: &ProjectsConfig,
    spec: &str,
    path_tokens: &[String],
) -> Result<ProjectsConfig> {
    // Don't bail to "all projects" just because the goal has no path-like
    // tokens — a goal like "update the billing dashboard app" still names a
    // project by word/basename, and on a big monorepo scoping to that one (vs
    // all N) matters. `project_matches_goal` handles empty path_tokens, and the
    // matches-empty fallback below still widens to everything when nothing
    // matches.
    let spec_lower = spec.to_ascii_lowercase();

    // Document frequency of each project name-word across the workspace, so a
    // word UNIQUE to one project ("comment", "search") is a strong relevance
    // signal while a word shared by many ("listener", "platform") is weak. This
    // is what lets a goal naming a module's words out of order or as a subset
    // ("comment callback listener" → callback-comment-listener) scope correctly
    // instead of over-widening to every project.
    let mut word_df: HashMap<String, usize> = HashMap::new();
    for name in projects.projects.keys() {
        let mut seen = HashSet::new();
        for w in name_words_of(name) {
            if seen.insert(w.clone()) {
                *word_df.entry(w).or_default() += 1;
            }
        }
    }
    // A name-word shared by at most this many projects still counts as a
    // "moderately rare" signal (needs 2 of them to co-occur). Scales with the
    // workspace so it stays selective on a large monorepo.
    let rare_df = (projects.projects.len() / 12).max(2);

    let mut matches = BTreeSet::new();
    for (name, project) in &projects.projects {
        if project_matches_goal(name, project, &spec_lower, path_tokens, &word_df, rare_df)? {
            matches.insert(name.clone());
        }
    }

    if matches.is_empty() {
        return Ok(projects.clone());
    }

    let mut filtered = projects.clone();
    filtered.projects.retain(|name, _| matches.contains(name));
    Ok(filtered)
}

fn project_matches_goal(
    name: &str,
    project: &Project,
    spec_lower: &str,
    path_tokens: &[String],
    word_df: &HashMap<String, usize>,
    rare_df: usize,
) -> Result<bool> {
    let name_lower = name.to_ascii_lowercase();
    if spec_lower.contains(&name_lower) {
        return Ok(true);
    }

    // Multi-word project names (snake/kebab, e.g. `announcement-task-listener`)
    // usually show up in a goal as a natural phrase ("announcement task
    // listener"). Match when the name's words appear contiguously in the goal —
    // without this, snake_cased service names in a monorepo never match and the
    // planner over-widens to every project.
    let name_words: Vec<&str> = name_lower
        .split(['-', '_', '/'])
        .filter(|w| w.len() >= 2)
        .collect();
    if name_words.len() >= 2 && goal_contains_phrase(spec_lower, &name_words) {
        return Ok(true);
    }

    // Order-independent discriminative match. The in-order phrase check above
    // misses a goal that lists a module's words out of order or as a subset
    // ("comment callback listener" → callback-comment-listener, "webhook
    // senders" → webhook-*-sender). Score by document frequency: a name-word
    // UNIQUE to one project is decisive on its own; otherwise require ≥2
    // moderately-rare name-words to co-occur, so ubiquitous tokens shared by
    // many modules ("listener", "platform") never over-widen to everything.
    // Words that are merely path segments of the goal (e.g. "cli" inside
    // `src/cli/commands`) are about a file location, not a project reference —
    // don't let them trigger a name match.
    let path_words: HashSet<String> = path_tokens
        .iter()
        .flat_map(|t| t.split(['/', '_', '-', '.']))
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    if name_matches_by_discriminative_words(&name_words, spec_lower, word_df, rare_df, &path_words)
    {
        return Ok(true);
    }

    // Match the project's directory basename mentioned as a word in the goal,
    // e.g. goal "update api and web" → projects `demo-api` (path `api`) and
    // `demo-web` (path `web`). Without this, a goal that names dirs informally
    // (not by their registered id) silently drops those projects.
    if let Some(base) = Path::new(&project.path)
        .file_name()
        .and_then(|s| s.to_str())
    {
        let base_lower = base.to_ascii_lowercase();
        // Skip generic leaf-folder names (test/lib/src/app/...) — in deep
        // monorepo paths the leaf is often a common word that shows up in goals
        // incidentally ("add a unit test"), which would wrongly pull the project
        // in. The full project name (matched above/as a phrase) is the reliable
        // signal for those.
        if base_lower.len() >= 3
            && !is_generic_basename(&base_lower)
            && goal_mentions_word(spec_lower, &base_lower)
        {
            return Ok(true);
        }
    }

    let project_root = project_root_path(&project.path)?;
    let workspace_root = paths::workspace_root()?;
    let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);

    for token in path_tokens {
        let token_path = Path::new(token);
        if !token_path.is_absolute()
            && project_contains_relative_path_case_insensitive(&project_root, token_path)
        {
            return Ok(true);
        }

        let token_abs = if token_path.is_absolute() {
            token_path.to_path_buf()
        } else {
            workspace_root.join(token_path)
        };
        let token_abs = token_abs.canonicalize().unwrap_or(token_abs);
        if path_starts_with_case_insensitive(&token_abs, &project_root) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn project_contains_relative_path_case_insensitive(project_root: &Path, relative: &Path) -> bool {
    let mut current = project_root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                let Some(next) = std::fs::read_dir(&current)
                    .ok()
                    .into_iter()
                    .flat_map(|entries| entries.filter_map(Result::ok))
                    .find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&part.to_string_lossy())
                    })
                else {
                    return false;
                };
                current = next.path();
            }
            _ => return false,
        }
    }
    true
}

fn project_root_path(project_path: &str) -> Result<PathBuf> {
    let expanded = paths::expand(project_path)?;
    let path = if expanded.is_absolute() {
        expanded
    } else {
        paths::workspace_root()?.join(expanded)
    };
    Ok(path.canonicalize().unwrap_or(path))
}

fn path_starts_with_case_insensitive(path: &Path, prefix: &Path) -> bool {
    let path = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let prefix = prefix
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    prefix.len() <= path.len()
        && path
            .iter()
            .zip(prefix.iter())
            .all(|(left, right)| left == right)
}

fn render_synthesized_plan(
    spec: &str,
    projects: &ProjectsConfig,
    selected: &BTreeSet<String>,
    out_path: Option<&std::path::Path>,
    intent: SynthesisIntent,
    notice: Option<&str>,
) -> String {
    let derived = derived_topology(projects);
    let ordered = ordered_project_names(projects, selected, &derived);
    let mut s = String::new();
    s.push_str(&format!("spec: {}\n", yaml_string(spec)));
    s.push_str(match intent {
        SynthesisIntent::Change => "created_by: maestro-work\n",
        SynthesisIntent::Audit => "created_by: maestro-init-analyze\n",
    });
    if let Some(notice) = notice {
        s.push_str(&format!("notice: {}\n", yaml_string(notice)));
    }
    s.push('\n');
    s.push_str("goal:\n");
    s.push_str(&format!("  description: {}\n", yaml_string(spec)));
    s.push_str("  acceptance:\n");
    s.push_str("    - describe: synthesized project verification passes\n");
    s.push_str(&format!(
        "      check: {}\n\n",
        yaml_string(&acceptance_check(projects, &ordered))
    ));
    s.push_str("tasks:\n");
    let monorepo = is_monorepo(projects, selected);
    let change_ids = ordered
        .iter()
        .map(|name| (name.clone(), work_task_id(name, intent)))
        .collect::<BTreeMap<_, _>>();
    let verify_ids = ordered
        .iter()
        .filter_map(|name| {
            let project = projects.projects.get(name)?;
            verification_command(project).map(|_| (name.clone(), verify_task_id(name)))
        })
        .collect::<BTreeMap<_, _>>();

    for name in &ordered {
        let Some(project) = projects.projects.get(name) else {
            continue;
        };
        let task_id = change_ids.get(name).expect("change task id");
        s.push_str(&format!("  - id: {task_id}\n"));
        s.push_str(&format!("    project: {name}\n"));
        s.push_str("    skills: [");
        s.push_str(&skills_for(project).join(", "));
        s.push_str("]\n");
        let deps = effective_deps(name, projects, &derived)
            .into_iter()
            .filter(|dep| selected.contains(dep))
            .filter_map(|dep| change_ids.get(&dep).cloned())
            .collect::<Vec<_>>();
        if !deps.is_empty() {
            s.push_str(&format!("    depends_on: [{}]\n", deps.join(", ")));
        }
        s.push_str("    prompt: |\n");
        for line in task_prompt(spec, name, project, projects, selected, intent, monorepo).lines() {
            s.push_str("      ");
            s.push_str(line);
            s.push('\n');
        }
        s.push_str("    requires_approval_after: false\n");
        s.push_str("    timeout_minutes: 45\n\n");
    }

    for name in &ordered {
        let Some(project) = projects.projects.get(name) else {
            continue;
        };
        let Some(command) = verification_command(project) else {
            continue;
        };
        let task_id = verify_ids.get(name).expect("verify task id");
        let mut deps = vec![change_ids.get(name).expect("change task id").clone()];
        deps.extend(
            effective_deps(name, projects, &derived)
                .into_iter()
                .filter(|dep| selected.contains(dep))
                .filter_map(|dep| verify_ids.get(&dep).cloned()),
        );
        deps.sort();
        deps.dedup();
        s.push_str(&format!("  - id: {task_id}\n"));
        s.push_str(&format!("    project: {name}\n"));
        s.push_str("    kind: verify\n");
        s.push_str("    agent: shell\n");
        s.push_str(&format!("    depends_on: [{}]\n", deps.join(", ")));
        s.push_str("    command: |\n");
        for line in command.lines() {
            s.push_str("      ");
            s.push_str(line);
            s.push('\n');
        }
        s.push('\n');
    }

    if let Some(path) = out_path {
        s.push_str(match intent {
            SynthesisIntent::Change => "# Generated by `maestro work`.\n",
            SynthesisIntent::Audit => "# Generated by `maestro init --analyze`.\n",
        });
        s.push_str(&format!("# Review before running: {}\n", path.display()));
    }
    s
}

/// A project's effective dependency project names for ordering: its inferred
/// `dependencies`, the **contract producers** it depends on (any project that
/// `provides` what this project `consumes`), plus **code-graph-derived
/// producers** it imports (`derived`). The last is what lets a monorepo whose
/// modules declare no dependencies and cross no contracts still order a
/// consumer task after the modules it actually imports, instead of racing them.
fn effective_deps(
    name: &str,
    projects: &ProjectsConfig,
    derived: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let Some(p) = projects.projects.get(name) else {
        return Vec::new();
    };
    let mut deps = p.dependencies.clone();
    if let Some(consumed) = &p.contracts.consumes {
        for (other, op) in &projects.projects {
            if other != name && op.contracts.provides.as_deref() == Some(consumed.as_str()) {
                deps.push(other.clone());
            }
        }
    }
    if let Some(d) = derived.get(name) {
        deps.extend(d.iter().cloned());
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Code-graph-derived module dependencies — `consumer → set of producer
/// modules it imports` — for ordering a monorepo whose modules declare none.
/// Returns empty when there's no code-graph index (or the feature is off), so
/// planning degrades to declared/contract ordering. Acyclic + weight-thresholded.
fn derived_topology(projects: &ProjectsConfig) -> BTreeMap<String, BTreeSet<String>> {
    let Ok(root) = crate::paths::workspace_root() else {
        return BTreeMap::new();
    };
    let module_paths = module_paths_of(projects);
    crate::codegraph::load_derived_module_deps(&root, &module_paths, 1)
}

/// `(name, relative_path)` for every registered project — the input the
/// code-graph rollup needs to map files back to their owning module.
fn module_paths_of(projects: &ProjectsConfig) -> Vec<(String, String)> {
    projects
        .projects
        .iter()
        .map(|(name, p)| (name.clone(), p.path.clone()))
        .collect()
}

fn ordered_project_names(
    projects: &ProjectsConfig,
    selected: &BTreeSet<String>,
    derived: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let mut indegree = selected
        .iter()
        .map(|name| (name.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for name in selected {
        if !projects.projects.contains_key(name) {
            continue;
        }
        for dep in effective_deps(name, projects, derived) {
            if selected.contains(&dep) {
                *indegree.entry(name.clone()).or_default() += 1;
                outgoing.entry(dep.clone()).or_default().push(name.clone());
            }
        }
    }
    for values in outgoing.values_mut() {
        values.sort();
        values.dedup();
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(name.clone()))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::new();
    while let Some(name) = ready.pop_first() {
        ordered.push(name.clone());
        if let Some(children) = outgoing.get(&name) {
            for child in children {
                if let Some(count) = indegree.get_mut(child) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
    }
    if ordered.len() != selected.len() {
        return selected.iter().cloned().collect();
    }
    ordered
}

fn skills_for(project: &Project) -> Vec<&'static str> {
    let mut skills = vec!["workflow-task-guardrails", "verify-before-done"];
    if project.contracts.provides.is_some() || project.contracts.consumes.is_some() {
        skills.push("contract-first");
    }
    let project_type = project.r#type.as_deref().unwrap_or_default();
    let frontend_stack = project.stack.iter().any(|s| {
        matches!(
            s.to_lowercase().as_str(),
            "frontend" | "web" | "react" | "vue" | "svelte" | "next" | "vite"
        )
    });
    if matches!(project_type, "frontend" | "web") || frontend_stack {
        skills.push("qa-web-flow");
    }
    skills
}

/// Resolve a project's configured path to an absolute filesystem path.
fn resolve_project_path(path: &str) -> Option<PathBuf> {
    let expanded = paths::expand(path).ok()?;
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        paths::workspace_root().ok()?.join(expanded)
    };
    Some(abs.canonicalize().unwrap_or(abs))
}

/// True when 2+ selected projects live in the **same git repo** — i.e. a
/// monorepo, where one task's worktree exposes the sibling projects and an
/// agent could wander out of its lane. (Polyrepo → distinct roots → false, and
/// each worktree only contains that project anyway.)
fn is_monorepo(projects: &ProjectsConfig, selected: &BTreeSet<String>) -> bool {
    if selected.len() < 2 {
        return false;
    }
    let mut roots = std::collections::HashSet::new();
    for name in selected {
        let Some(p) = projects.projects.get(name) else {
            continue;
        };
        let Some(abs) = resolve_project_path(&p.path) else {
            return false;
        };
        roots.insert(crate::codegraph::find_repo_root(&abs));
    }
    roots.len() == 1
}

fn task_prompt(
    spec: &str,
    name: &str,
    project: &Project,
    projects: &ProjectsConfig,
    selected: &BTreeSet<String>,
    intent: SynthesisIntent,
    monorepo: bool,
) -> String {
    let upstream = project
        .dependencies
        .iter()
        .filter(|dep| selected.contains(*dep))
        .cloned()
        .collect::<Vec<_>>();
    let downstream = projects
        .projects
        .iter()
        .filter(|(other, p)| {
            *other != name
                && selected.contains(*other)
                && p.dependencies.contains(&name.to_string())
        })
        .map(|(other, _)| other.clone())
        .collect::<Vec<_>>();

    let mut p = String::new();
    p.push_str(&format!("Goal: {spec}\n"));
    p.push_str(&format!("Project: {name}\n"));
    if !upstream.is_empty() {
        p.push_str(&format!(
            "Registered upstream dependencies: {}\n",
            upstream.join(", ")
        ));
    }
    if !downstream.is_empty() {
        p.push_str(&format!(
            "Registered downstream consumers: {}\n",
            downstream.join(", ")
        ));
    }
    if let Some(provides) = &project.contracts.provides {
        p.push_str(&format!("Contract provided by this project: {provides}\n"));
    }
    if let Some(consumes) = &project.contracts.consumes {
        p.push_str(&format!("Contract consumed by this project: {consumes}\n"));
    }
    // Stack + the exact local check, so the agent knows the build system and
    // how to verify — and so a monorepo build command (`bazel …` / `rush …`)
    // in the prompt fires the matching built-in skill via trigger-matching.
    if !project.stack.is_empty() {
        p.push_str(&format!("Stack: {}\n", project.stack.join(", ")));
    }
    if let Some(check) = verification_command(project) {
        let one_line = check.lines().collect::<Vec<_>>().join(" ");
        p.push_str(&format!("Local check for this project: {one_line}\n"));
    }
    match intent {
        SynthesisIntent::Change => {
            p.push_str("Make the smallest project-local change needed for the goal. If this project does not need code changes, leave a concise handoff note in the task log and do not churn unrelated files.\n");
            if monorepo {
                p.push_str(&format!(
                    "Monorepo: this project lives at `{}`. Edit ONLY files under that directory — the sibling projects in this checkout are handled by their own tasks (which may run in parallel), so do not modify them. If a sibling needs a change, note it as a handoff instead of editing it.\n",
                    project.path
                ));
            }
            p.push_str("Before finishing, run the fastest relevant local check for this project and report the exact command/output summary.\n");
        }
        SynthesisIntent::Audit => {
            p.push_str("Audit this project as part of the discovered dependency DAG. Do not modify files. Confirm whether the registered upstream/downstream relationships, contracts, stack, and verification command look accurate; record any discrepancy in the task log.\n");
            p.push_str("Before finishing, run the fastest relevant local check for this project when one is registered and report the exact command/output summary.\n");
        }
    }
    p
}

fn verification_command(project: &Project) -> Option<String> {
    project
        .commands
        .get("test")
        .or_else(|| project.commands.get("check"))
        .or_else(|| project.commands.get("build"))
        .cloned()
}

fn acceptance_check(projects: &ProjectsConfig, ordered: &[String]) -> String {
    let checks = ordered
        .iter()
        .filter_map(|name| {
            let project = projects.projects.get(name)?;
            let command = verification_command(project)?;
            Some(format!(
                "(cd {} && {})",
                shell_quote(&project.path),
                command
            ))
        })
        .collect::<Vec<_>>();
    if checks.is_empty() {
        "echo 'no verification commands registered; replace this acceptance check' && false"
            .to_string()
    } else {
        checks.join(" && ")
    }
}

fn yaml_string(s: &str) -> String {
    format!("{s:?}")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn change_task_id(name: &str) -> String {
    format!("T_change_{}", id_fragment(name))
}

fn review_task_id(name: &str) -> String {
    format!("T_review_{}", id_fragment(name))
}

fn work_task_id(name: &str, intent: SynthesisIntent) -> String {
    match intent {
        SynthesisIntent::Change => change_task_id(name),
        SynthesisIntent::Audit => review_task_id(name),
    }
}

fn verify_task_id(name: &str) -> String {
    format!("T_verify_{}", id_fragment(name))
}

fn id_fragment(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Contracts;
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    fn plan_preview_has_counts_edges_blast_and_coded_issues() {
        let p: Plan = serde_yaml::from_str(
            r#"
spec: update the shared contract and consumers
tasks:
  - { id: T_change_shared_contracts, project: shared-contracts, kind: agent }
  - { id: T_change_billing_service, project: billing-service, kind: agent, depends_on: [T_change_shared_contracts] }
  - { id: T_change_web_frontend, project: web-frontend, kind: agent, depends_on: [T_change_shared_contracts] }
"#,
        )
        .unwrap();
        let cfg: ProjectsConfig = serde_yaml::from_str(
            r#"
version: 1
projects:
  shared-contracts:
    path: shared-contracts
  billing-service:
    path: billing-service
"#,
        )
        .unwrap();
        let report = crate::config::analyze(&p, &cfg);
        let preview = crate::config::analyze::plan_preview(&p, &report);

        assert_eq!(preview.task_count, 3);
        assert_eq!(preview.project_count, 3);
        assert_eq!(preview.dependency_edges.len(), 2);
        let blast = preview
            .blast_radius
            .iter()
            .find(|b| b.task == "T_change_shared_contracts")
            .expect("shared-contracts has downstream");
        assert_eq!(blast.downstream.len(), 2);
        // an unregistered project surfaces as a coded error issue, not prose
        let unknown = preview
            .errors
            .iter()
            .find(|i| i.code == codes::UNKNOWN_PROJECT)
            .expect("plan.unknown_project error");
        assert_eq!(unknown.path.as_deref(), Some("T_change_web_frontend"));
        assert!(!preview.is_valid());
        // round-trips through the shared serializer
        let back: PlanPreview = serde_json::from_str(&preview.to_json()).unwrap();
        assert_eq!(back, preview);
    }

    #[test]
    fn generic_basenames_are_not_relevance_signals() {
        // common leaf-folder words must not match (a goal saying "test"/"lib"
        // shouldn't pull every project whose folder is named that)...
        for g in ["test", "lib", "src", "app", "core", "shared", "packages"] {
            assert!(is_generic_basename(g), "{g} should be generic");
        }
        // ...but domain-ish names stay useful informal references.
        for s in ["api", "web", "gateway", "auth", "center"] {
            assert!(!is_generic_basename(s), "{s} should stay matchable");
        }
    }

    #[test]
    fn goal_phrase_match_handles_snake_kebab_names() {
        // snake/kebab project name written with spaces in the goal → matches
        assert!(goal_contains_phrase(
            "update the announcement task listener now",
            &["announcement", "task", "listener"]
        ));
        // order/contiguity required — scattered words don't match
        assert!(!goal_contains_phrase(
            "update the subscriber and the application",
            &["application", "subscriber"]
        ));
        // unrelated goal → no match (planner then widens via the empty fallback)
        assert!(!goal_contains_phrase(
            "run a security audit",
            &["announcement", "task", "listener"]
        ));
    }

    fn project(path: &str, deps: &[&str], test: &str) -> Project {
        let mut commands = BTreeMap::new();
        commands.insert("test".to_string(), test.to_string());
        project_with_commands(path, deps, commands)
    }

    #[test]
    fn discriminative_word_match_scopes_reordered_and_plural_names() {
        // Realistic backend frequencies: callback/listener are ubiquitous,
        // webhook/sender moderately rare, comment unique.
        let df: HashMap<String, usize> = [
            ("callback", 9),
            ("listener", 16),
            ("comment", 1),
            ("game", 1),
            ("webhook", 3),
            ("sender", 2),
            ("message", 2),
            ("test", 1),
            ("cache", 1),
            ("manage", 1),
        ]
        .into_iter()
        .map(|(w, n)| (w.to_string(), n))
        .collect();
        let rare_df = 4; // == 48 modules / 12
        let no_path = HashSet::new();
        let matches = |name: &str, goal: &str| {
            let words = name.split('-').collect::<Vec<_>>();
            name_matches_by_discriminative_words(&words, goal, &df, rare_df, &no_path)
        };

        // Out-of-order subset: the unique word "comment" pins the right listener,
        // and the ubiquitous callback/listener do NOT pull the others.
        assert!(matches(
            "callback-comment-listener",
            "fix the comment callback listener"
        ));
        assert!(!matches(
            "callback-game-listener",
            "fix the comment callback listener"
        ));

        // Plural + two moderately-rare words co-occur → both senders match…
        assert!(matches(
            "webhook-message-sender",
            "add metrics to the webhook senders"
        ));
        assert!(matches(
            "webhook-test-sender",
            "add metrics to the webhook senders"
        ));
        // …but a sibling sharing only the ubiquitous-ish "webhook" does not.
        assert!(!matches(
            "webhook-cache-manage",
            "add metrics to the webhook senders"
        ));

        // A path-segment word must not count as a name reference.
        let path_words: HashSet<String> = ["cli".to_string()].into_iter().collect();
        let df_cli: HashMap<String, usize> = [("demo", 2), ("cli", 1)]
            .into_iter()
            .map(|(w, n)| (w.to_string(), n))
            .collect();
        assert!(!name_matches_by_discriminative_words(
            &["demo", "cli"],
            "audit error handling in src/cli/commands",
            &df_cli,
            2,
            &path_words,
        ));
    }

    #[test]
    fn task_prompt_surfaces_stack_and_check_so_the_right_monorepo_skill_triggers() {
        let one = |name: &str, check: &str, stack: &[&str]| {
            let mut c = BTreeMap::new();
            c.insert("check".to_string(), check.to_string());
            let mut p = project_with_commands(name, &[], c);
            p.stack = stack.iter().map(|s| s.to_string()).collect();
            let mut cfg = ProjectsConfig {
                version: 1,
                defaults: Default::default(),
                projects: BTreeMap::new(),
            };
            cfg.projects.insert(name.to_string(), p);
            let sel: BTreeSet<String> = [name.to_string()].into_iter().collect();
            task_prompt(
                "goal",
                name,
                &cfg.projects[name],
                &cfg,
                &sel,
                SynthesisIntent::Change,
                false,
            )
        };

        // Bazel/Go module: prompt carries the bazel check (fires monorepo-bazel-go),
        // and nothing rush-flavored (so monorepo-rush stays quiet).
        let bazel = one(
            "app/openapi_platform",
            "bazel test //app/openapi_platform/...",
            &["rpc"],
        );
        assert!(bazel.contains("Stack: rpc"));
        assert!(
            bazel.contains("bazel test //app/openapi_platform"),
            "{bazel}"
        );
        assert!(!bazel.to_lowercase().contains("rush"), "{bazel}");

        // Rush module: prompt carries the rush bootstrap + subspace tag (fires
        // monorepo-rush), and never mentions bazel.
        let rush = one(
            "subspaces/minis/feed_sidebar",
            "node common/scripts/install-run-rush.js build --to feed_sidebar",
            &["node", "subspace:minis"],
        );
        assert!(rush.contains("subspace:minis"));
        assert!(rush.to_lowercase().contains("rush"), "{rush}");
        assert!(!rush.to_lowercase().contains("bazel"), "{rush}");
    }

    fn project_without_commands(path: &str, deps: &[&str]) -> Project {
        project_with_commands(path, deps, BTreeMap::new())
    }

    fn project_with_commands(
        path: &str,
        deps: &[&str],
        commands: BTreeMap<String, String>,
    ) -> Project {
        Project {
            path: path.to_string(),
            r#type: Some("library".to_string()),
            stack: vec![],
            commands,
            contracts: Contracts::default(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            memory_scope: vec![],
            agent: Some("codex".to_string()),
            agent_model: None,
            cursor_model: None,
            model_profile: None,
            role: None,
            agent_profile: None,
            review_profile: None,
            copy_files: Vec::new(),
        }
    }

    struct WorkspaceEnvGuard;

    impl Drop for WorkspaceEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("MAESTRO_WORKSPACE_ROOT");
        }
    }

    fn set_workspace_root(path: &std::path::Path) -> WorkspaceEnvGuard {
        std::env::set_var("MAESTRO_WORKSPACE_ROOT", path);
        WorkspaceEnvGuard
    }

    #[test]
    fn synthesized_plan_uses_project_dependency_order() {
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("core".to_string(), project("core", &[], "npm run test"));
        projects
            .projects
            .insert("cli".to_string(), project("cli", &["core"], "npm run test"));
        let selected = selected_projects(&projects, Vec::new()).unwrap();

        let plan = render_synthesized_plan(
            "Add JSON output",
            &projects,
            &selected,
            None,
            SynthesisIntent::Change,
            None,
        );
        let core_pos = plan.find("project: core").unwrap();
        let cli_pos = plan.find("project: cli").unwrap();
        assert!(core_pos < cli_pos);
        assert!(plan.contains("depends_on: [T_change_core]"));
        assert!(plan.contains("depends_on: [T_change_cli, T_verify_core]"));
        assert!(plan.contains("(cd 'core' && npm run test) && (cd 'cli' && npm run test)"));
    }

    #[test]
    fn synthesized_plan_orders_by_contract_producer_consumer() {
        // No explicit `dependencies` — only a provides/consumes contract link.
        let mut shared = project("shared", &[], "npm run test");
        shared.contracts.provides = Some("types/index.d.ts".to_string());
        let mut api = project("api", &[], "npm run test");
        api.contracts.consumes = Some("types/index.d.ts".to_string());

        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects.projects.insert("shared".to_string(), shared);
        projects.projects.insert("api".to_string(), api);
        let selected = selected_projects(&projects, Vec::new()).unwrap();

        let plan = render_synthesized_plan(
            "Propagate contract change",
            &projects,
            &selected,
            None,
            SynthesisIntent::Change,
            None,
        );
        // The consumer's change task waits for the producer's, and the producer
        // is laid out first — derived from the contract alone.
        assert!(
            plan.contains("depends_on: [T_change_shared]"),
            "plan was:\n{plan}"
        );
        assert!(plan.find("project: shared").unwrap() < plan.find("project: api").unwrap());
        // and the verify task chains through the producer's verify too
        assert!(plan.contains("depends_on: [T_change_api, T_verify_shared]"));
    }

    #[test]
    #[serial]
    fn monorepo_guardrail_added_when_projects_share_a_git_root() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        // a single git root shared by two project subdirs → monorepo
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("shared")).unwrap();
        std::fs::create_dir_all(dir.path().join("api")).unwrap();

        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("shared".to_string(), project("shared", &[], "npm run test"));
        projects
            .projects
            .insert("api".to_string(), project("api", &[], "npm run test"));
        let selected = selected_projects(&projects, Vec::new()).unwrap();

        let plan = render_synthesized_plan(
            "Change something",
            &projects,
            &selected,
            None,
            SynthesisIntent::Change,
            None,
        );
        assert!(
            plan.contains("Monorepo: this project lives at `shared`"),
            "expected monorepo guardrail, plan was:\n{plan}"
        );
    }

    #[test]
    #[serial]
    fn synthesize_file_root_filter_excludes_projects_outside_root() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        let root = dir.path().join("fixture");
        let core = root.join("packages/core");
        let cli = root.join("packages/cli");
        let stale = dir.path().join("stale/codex-lab-core");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::create_dir_all(&stale).unwrap();

        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects.projects.insert(
            "demo-core".to_string(),
            project(core.to_str().unwrap(), &[], "npm test"),
        );
        projects.projects.insert(
            "demo-cli".to_string(),
            project(cli.to_str().unwrap(), &["demo-core"], "npm test"),
        );
        projects.projects.insert(
            "codex-lab-core".to_string(),
            project(stale.to_str().unwrap(), &[], "npm test"),
        );

        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();

        let plan_path = synthesize_file(
            "Audit shared dependencies",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(&root),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("project: demo-core"));
        assert!(plan.contains("project: demo-cli"));
        assert!(!plan.contains("project: codex-lab-core"));
        assert!(!plan.contains("T_change_codex_lab_core"));
    }

    fn save_goal_relevance_fixture(dir: &TempDir) {
        let src_commands = dir.path().join("src/cli/commands");
        let demo_core = dir.path().join("examples/demo-monorepo/core");
        let demo_cli = dir.path().join("examples/demo-monorepo/cli");
        let maestro_web = dir.path().join("web");
        std::fs::create_dir_all(&src_commands).unwrap();
        std::fs::create_dir_all(&demo_core).unwrap();
        std::fs::create_dir_all(&demo_cli).unwrap();
        std::fs::create_dir_all(&maestro_web).unwrap();

        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("maestro".to_string(), project(".", &[], "cargo test"));
        projects.projects.insert(
            "demo-core".to_string(),
            project("examples/demo-monorepo/core", &[], "cargo test"),
        );
        projects.projects.insert(
            "demo-cli".to_string(),
            project("examples/demo-monorepo/cli", &[], "cargo test"),
        );
        projects
            .projects
            .insert("maestro-web".to_string(), project("web", &[], "pnpm test"));

        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();
    }

    #[test]
    #[serial]
    fn synthesize_file_filters_projects_by_goal_path_token() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        save_goal_relevance_fixture(&dir);

        let plan_path = synthesize_file(
            "audit error handling in src/cli/commands",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(dir.path()),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("project: maestro"));
        assert!(!plan.contains("project: demo-cli"));
        assert!(!plan.contains("project: demo-core"));
        assert!(!plan.contains("project: maestro-web"));
        assert!(!plan.contains("T_change_demo_cli"));
        assert!(!plan.contains("T_change_demo_core"));
        assert!(!plan.contains("T_change_maestro_web"));
    }

    #[test]
    #[serial]
    fn synthesize_file_includes_projects_named_by_dir_basename_word() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        save_goal_relevance_fixture(&dir);

        // A path token (src/cli/commands) triggers narrowing → maestro. The
        // words "cli" and "web" name the demo-cli (path .../cli) and maestro-web
        // (path web) projects, so they're included too — but demo-core (basename
        // `core`, never named) stays out.
        let plan_path = synthesize_file(
            "audit src/cli/commands and the cli and web projects",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(dir.path()),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();
        assert!(plan.contains("project: maestro"));
        assert!(plan.contains("project: demo-cli"));
        assert!(plan.contains("project: maestro-web"));
        assert!(!plan.contains("project: demo-core"));
    }

    #[test]
    #[serial]
    fn synthesize_file_falls_back_when_no_goal_token_matches() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        save_goal_relevance_fixture(&dir);

        let plan_path = synthesize_file(
            "clean up tests",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(dir.path()),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("project: maestro"));
        assert!(plan.contains("project: demo-cli"));
        assert!(plan.contains("project: demo-core"));
        assert!(plan.contains("project: maestro-web"));
    }

    #[test]
    #[serial]
    fn synthesize_file_emits_planner_granularity_notice_when_goal_has_path_token() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        save_goal_relevance_fixture(&dir);

        let plan_path = synthesize_file(
            "audit error handling in src/cli/commands",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(dir.path()),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("notice:"));
        assert!(plan.contains("planner is project-level"));
        assert!(plan.contains("src/cli/commands"));
    }

    #[test]
    #[serial]
    fn synthesize_file_path_match_is_case_insensitive_path_prefix() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        save_goal_relevance_fixture(&dir);

        let plan_path = synthesize_file(
            "audit error handling in src/CLI/commands",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            Some(dir.path()),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("project: maestro"));
        assert!(!plan.contains("project: demo-cli"));
        assert!(!plan.contains("project: demo-core"));
        assert!(!plan.contains("project: maestro-web"));
    }

    #[test]
    #[serial]
    fn synthesize_file_matches_relative_goal_path_under_project_root() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        let repo = dir.path().join("repo");
        let _guard = set_workspace_root(&workspace);
        std::fs::create_dir_all(workspace.join(".maestro")).unwrap();
        std::fs::create_dir_all(repo.join("src/cli/commands")).unwrap();
        std::fs::create_dir_all(repo.join("examples/demo-cli")).unwrap();
        std::fs::create_dir_all(repo.join("web")).unwrap();

        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects.projects.insert(
            "maestro".to_string(),
            project(repo.to_str().unwrap(), &[], "cargo test"),
        );
        projects.projects.insert(
            "demo-cli".to_string(),
            project(
                repo.join("examples/demo-cli").to_str().unwrap(),
                &[],
                "pnpm test",
            ),
        );
        projects.projects.insert(
            "maestro-web".to_string(),
            project(repo.join("web").to_str().unwrap(), &[], "pnpm build"),
        );
        projects
            .save(&workspace.join(".maestro/projects.yaml"))
            .unwrap();

        let plan_path = synthesize_file(
            "audit error handling in src/cli/commands",
            Some(workspace.join("PLAN.yaml")),
            Vec::new(),
            Some(&repo),
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("project: maestro"));
        assert!(!plan.contains("project: demo-cli"));
        assert!(!plan.contains("project: maestro-web"));
    }

    #[test]
    fn synthesized_plan_omits_verify_dependency_when_upstream_has_no_command() {
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("core".to_string(), project_without_commands("core", &[]));
        projects
            .projects
            .insert("cli".to_string(), project("cli", &["core"], "npm run test"));
        let selected = selected_projects(&projects, Vec::new()).unwrap();

        let plan = render_synthesized_plan(
            "Analyze workspace",
            &projects,
            &selected,
            None,
            SynthesisIntent::Change,
            None,
        );

        assert!(!plan.contains("id: T_verify_core"));
        assert!(plan.contains("depends_on: [T_change_cli]"));
        assert!(!plan.contains("T_verify_core"));
    }

    #[test]
    fn synthesized_audit_plan_uses_review_tasks_without_change_prompt() {
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("core".to_string(), project("core", &[], "npm run test"));
        let selected = selected_projects(&projects, Vec::new()).unwrap();

        let plan = render_synthesized_plan(
            "Analyze discovered DAG",
            &projects,
            &selected,
            None,
            SynthesisIntent::Audit,
            None,
        );

        assert!(plan.contains("created_by: maestro-init-analyze"));
        assert!(plan.contains("id: T_review_core"));
        assert!(plan.contains("Audit this project as part of the discovered dependency DAG"));
        assert!(plan.contains("Do not modify files"));
        assert!(!plan.contains("id: T_change_core"));
        assert!(!plan.contains("Make the smallest project-local change needed"));
        assert!(plan.contains("depends_on: [T_review_core]"));
    }

    #[test]
    fn empty_project_registry_error_includes_recovery_commands() {
        let message =
            empty_project_registry_message(std::path::Path::new("/tmp/ws/.maestro/projects.yaml"));

        assert!(message.contains("no projects registered"));
        assert!(message.contains("maestro work \"<goal>\" --root <path>"));
        assert!(message.contains("maestro init --analyze --root <path>"));
    }

    #[test]
    fn empty_project_registry_error_mentions_concurrent_tmp_when_present() {
        let dir = TempDir::new().unwrap();
        let pfile = dir.path().join(".maestro/projects.yaml");
        std::fs::create_dir_all(pfile.parent().unwrap()).unwrap();
        std::fs::write(pfile.with_file_name("projects.yaml.tmp"), "pending").unwrap();

        let message = empty_project_registry_message(&pfile);

        assert!(message.contains("projects.yaml.tmp"));
        assert!(message.contains("another maestro process may be writing"));
        assert!(message.contains("maestro doctor"));
    }

    // F-101 integration tests — synthesize_file level. These exist
    // because the original 4 helper-level tests in `mod topology_scope`
    // wouldn't have caught the caller-side bug (first draft `retain`-ed
    // on `relevant.projects` which never contained the downstream
    // projects in the first place). The helpers can be perfect and the
    // synthesized plan still wrong; these tests pin the wiring end-to-
    // end through the public synthesize_file path.

    #[test]
    #[serial]
    fn synthesize_file_expands_topology_consumers_from_dependency_graph() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        // A <- B <- C, plus an `unrelated` project that depends on
        // nothing in the chain. Goal mentions `A` via the topology
        // phrase, so the plan should cover A + B + C and NOT unrelated.
        for sub in ["A", "B", "C", "unrelated"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("A".to_string(), project("A", &[], "true"));
        projects
            .projects
            .insert("B".to_string(), project("B", &["A"], "true"));
        projects
            .projects
            .insert("C".to_string(), project("C", &["B"], "true"));
        projects
            .projects
            .insert("unrelated".to_string(), project("unrelated", &[], "true"));
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();

        let plan_path = synthesize_file(
            "Audit error handling in projects that depend on A",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            None,
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        // A is the anchor; B and C are direct + transitive consumers.
        assert!(
            plan.contains("project: A"),
            "anchor A must be in the plan:\n{plan}"
        );
        assert!(
            plan.contains("project: B"),
            "direct consumer B must be in the plan"
        );
        assert!(
            plan.contains("project: C"),
            "transitive consumer C must be in the plan",
        );
        assert!(
            !plan.contains("project: unrelated"),
            "non-consumer must not be in the plan",
        );
        // Notice must name back the count + matched phrase so the user
        // can see why the scope expanded.
        assert!(plan.contains("notice:"));
        assert!(plan.contains("topology scope"));
        assert!(
            plan.contains("downstream project"),
            "notice must mention downstream count:\n{plan}",
        );
    }

    #[test]
    #[serial]
    fn synthesize_file_emits_notice_when_topology_anchor_missing() {
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        for sub in ["alpha", "beta"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("alpha".to_string(), project("alpha", &[], "true"));
        projects
            .projects
            .insert("beta".to_string(), project("beta", &[], "true"));
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();

        // Topology phrase present, but the goal mentions a project name
        // that isn't in the registry, so the lexical pass doesn't
        // narrow → no anchor. Expectation: plan keeps the fallback
        // (text relevance over all projects) AND the notice tells the
        // user the wording didn't anchor anything.
        let plan_path = synthesize_file(
            "Audit error handling in projects that depend on nonexistent-svc",
            Some(dir.path().join("PLAN.yaml")),
            Vec::new(),
            None,
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(plan.contains("notice:"));
        // The "fell back to text relevance" wording is the contract —
        // it's what tells the user the expansion didn't fire silently.
        assert!(
            plan.contains("fell back to text relevance"),
            "no-anchor topology goal must notice the fallback:\n{plan}",
        );
        assert!(
            plan.contains("depend on"),
            "notice should name back the topology phrase that fired:\n{plan}",
        );
    }

    #[test]
    fn default_plan_path_bounds_long_generated_filenames() {
        let spec = "Audit discovered dependency graph for a deeply nested example workspace path with many segments. Review the project dependency evidence and propose a safe plan.";

        let path = default_plan_path(spec, None);
        let name = path.file_name().unwrap().to_string_lossy();

        assert!(
            name.len() <= 96,
            "generated plan filename should stay readable, got {name}"
        );
        assert!(name.contains("audit-discovered-dependency-graph"));
        assert!(name.ends_with(".yaml"));
    }

    mod topology_scope {
        //! F-101 + F-102: planner must expand topology-scoped goals
        //! ("depends on X" / "consumers of X" / "downstream of X" /
        //! "blast radius of X" / "impacted by X") through the
        //! dependency graph instead of relying purely on lexical name
        //! matching. The format_skipped_list helper covers F-102 so a
        //! 50+ project monorepo doesn't get a 600-char stdout dump.
        use super::super::{
            expand_topology_downstream, format_skipped_list, topology_wording_in_spec,
        };
        use crate::config::ProjectsConfig;
        use std::collections::BTreeSet;

        fn projects_with_deps(deps: &[(&str, &[&str])]) -> ProjectsConfig {
            let mut yaml = String::from("version: 1\ndefaults:\n  agent: cursor\nprojects:\n");
            for (name, ds) in deps {
                yaml.push_str(&format!("  {name}:\n    path: .\n"));
                if !ds.is_empty() {
                    yaml.push_str("    dependencies:\n");
                    for d in *ds {
                        yaml.push_str(&format!("      - {d}\n"));
                    }
                }
            }
            serde_yaml::from_str(&yaml).expect("test fixture must parse")
        }

        #[test]
        fn topology_wording_detection_is_case_insensitive_and_phrase_specific() {
            assert_eq!(
                topology_wording_in_spec("DEPENDS ON foo"),
                Some("depends on")
            );
            assert_eq!(
                topology_wording_in_spec("blast radius of bar"),
                Some("blast radius of")
            );
            assert_eq!(
                topology_wording_in_spec("downstream of baz"),
                Some("downstream of")
            );
            assert_eq!(
                topology_wording_in_spec("Consumers Of Quux"),
                Some("consumers of")
            );
            // No false positive on the word `of` alone or `dependency` without `on`.
            assert_eq!(topology_wording_in_spec("update of dependency-foo"), None);
        }

        #[test]
        fn expand_downstream_collects_direct_and_transitive_consumers() {
            // shared <- foo <- bar (transitive); shared <- baz (direct only)
            let projects = projects_with_deps(&[
                ("shared", &[]),
                ("foo", &["shared"]),
                ("bar", &["foo"]),
                ("baz", &["shared"]),
                ("unrelated", &[]),
            ]);
            let anchors: BTreeSet<String> = ["shared".to_string()].into_iter().collect();
            let downstream = expand_topology_downstream(&projects, &anchors);
            assert!(downstream.contains("foo"));
            assert!(
                downstream.contains("bar"),
                "transitive consumer must be included"
            );
            assert!(downstream.contains("baz"));
            assert!(!downstream.contains("unrelated"));
            assert!(
                !downstream.contains("shared"),
                "anchor must not appear in its own downstream set",
            );
        }

        #[test]
        fn expand_downstream_is_empty_when_anchor_has_no_consumers() {
            // leaf-style anchor: nothing depends on it.
            let projects = projects_with_deps(&[("leaf", &["upstream"]), ("upstream", &[])]);
            let anchors: BTreeSet<String> = ["leaf".to_string()].into_iter().collect();
            let downstream = expand_topology_downstream(&projects, &anchors);
            assert!(downstream.is_empty());
        }

        #[test]
        fn format_skipped_list_inlines_short_lists_and_summarises_long_ones() {
            // Short list: full inline.
            let short = ["a", "b", "c"];
            assert_eq!(format_skipped_list(&short, 12), "a, b, c");

            // Exactly at threshold: still inline.
            let at = vec!["a"; 12];
            let s = format_skipped_list(&at, 12);
            assert!(!s.contains("skipped ("));

            // Above threshold: truncated summary.
            let names: Vec<String> = (0..20).map(|i| format!("proj-{i:02}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let summary = format_skipped_list(&refs, 12);
            assert!(summary.starts_with("20 skipped (first 12: "));
            assert!(summary.contains("proj-00"));
            assert!(summary.contains("proj-11"));
            assert!(
                !summary.contains("proj-12"),
                "names past the cap must NOT appear in the inline list",
            );
        }
    }

    #[test]
    #[serial]
    fn synthesize_audit_file_includes_all_projects_even_when_spec_mentions_one() {
        // F-AUDIT-001: an `init --analyze` audit must cover EVERY project even
        // when the spec text names one project / carries a path token. Under
        // Change intent that would narrow to `alpha` only; Audit intent must
        // bypass goal-relevance narrowing entirely.
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        for sub in ["alpha", "beta", "gamma"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("alpha".to_string(), project("alpha", &[], "true"));
        projects
            .projects
            .insert("beta".to_string(), project("beta", &["alpha"], "true"));
        projects
            .projects
            .insert("gamma".to_string(), project("gamma", &[], "true"));
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();

        // Spec names `alpha` AND carries a path token (`.../alpha`) — exactly
        // what would shrink a Change plan to one project.
        let spec = format!(
            "Analyze and verify the discovered project dependency DAG under {}/alpha",
            dir.path().display()
        );
        let plan_path =
            synthesize_audit_file(&spec, Some(dir.path().join("PLAN.yaml")), Vec::new()).unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        for p in ["alpha", "beta", "gamma"] {
            assert!(
                plan.contains(&format!("id: T_review_{p}")),
                "audit must cover every project ({p} missing):\n{plan}"
            );
        }
    }

    #[test]
    #[serial]
    fn synthesize_audit_file_with_project_selection_scopes_to_those_projects() {
        // The other half of F-AUDIT-001 + the `--project` scoping the audit-size
        // warning advises: an audit WITH an explicit selection covers only the
        // selected project(s), not the whole registry.
        let dir = TempDir::new().unwrap();
        let _guard = set_workspace_root(dir.path());
        for sub in ["alpha", "beta", "gamma"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let mut projects = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        projects
            .projects
            .insert("alpha".to_string(), project("alpha", &[], "true"));
        projects
            .projects
            .insert("beta".to_string(), project("beta", &[], "true"));
        projects
            .projects
            .insert("gamma".to_string(), project("gamma", &[], "true"));
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        projects
            .save(&dir.path().join(".maestro/projects.yaml"))
            .unwrap();

        let plan_path = synthesize_audit_file(
            "Analyze and verify the discovered project dependency DAG",
            Some(dir.path().join("PLAN.yaml")),
            vec!["beta".to_string()],
        )
        .unwrap();
        let plan = std::fs::read_to_string(plan_path).unwrap();

        assert!(
            plan.contains("id: T_review_beta"),
            "selected beta present:\n{plan}"
        );
        assert!(
            !plan.contains("id: T_review_alpha"),
            "alpha must be excluded:\n{plan}"
        );
        assert!(
            !plan.contains("id: T_review_gamma"),
            "gamma must be excluded:\n{plan}"
        );
    }
}
