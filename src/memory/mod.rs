use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::adapter::MemorySlice;
use crate::config::ProjectsConfig;
use crate::paths;

pub mod graph;
pub mod retrieval;

#[cfg(feature = "embeddings")]
pub mod embeddings;

/// Semantically rank memory chunks against a query using local embeddings when
/// the `embeddings` feature is built and the model is available; returns `None`
/// otherwise so callers fall back to TF-IDF.
pub fn semantic_rank(
    chunks: &[retrieval::Chunk],
    query: &str,
    k: usize,
) -> Option<Vec<retrieval::Hit>> {
    #[cfg(feature = "embeddings")]
    {
        embeddings::semantic_rank(chunks, query, k)
    }
    #[cfg(not(feature = "embeddings"))]
    {
        let _ = (chunks, query, k);
        None
    }
}

pub const L1_DIR: &str = "l1_facts";
pub const L2_DIR: &str = "l2_decisions";

/// How many of the most-recent L2 decision files to pull per producer.
/// Two is a sane default: it usually means "the contract decision plus
/// one most-recent functional change". Capped to keep prompt size in
/// check — old decisions are noise for new tasks.
const L2_RECENT_PER_PROJECT: usize = 2;

/// Soft cap on how many L2 decision files to keep per project. Older ones are
/// pruned after each archive so the store doesn't grow without bound — fan-in
/// and `load_recent_l2` only ever read the newest few anyway.
pub const L2_MAX_PER_PROJECT: usize = 20;

pub struct MemoryStore {
    root: PathBuf,
}

impl MemoryStore {
    pub fn open() -> Result<Self> {
        Self::open_at(paths::maestro_dir()?.join("memory"))
    }

    pub fn open_at(root: PathBuf) -> Result<Self> {
        paths::ensure_dir(&root)?;
        paths::ensure_dir(&root.join(L1_DIR))?;
        paths::ensure_dir(&root.join(L2_DIR))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn l1_root(&self) -> PathBuf {
        self.root.join(L1_DIR)
    }

    pub fn l2_root(&self) -> PathBuf {
        self.root.join(L2_DIR)
    }

    /// Load the N most recent L2 decisions archived under a project.
    /// Most recent = lexicographically largest filename, because the
    /// archiver names files `<date>-<slug>-<run_id>.md` — date prefix
    /// gives a free chronological sort.
    pub fn load_recent_l2(&self, project: &str, n: usize) -> Result<Vec<MemorySlice>> {
        let dir = self.l2_root().join(project);
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("read {:?}", dir))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        files.reverse(); // newest first
        files.truncate(n);

        let mut out = Vec::with_capacity(files.len());
        for f in files {
            let rel = f
                .strip_prefix(self.l2_root())
                .unwrap_or(&f)
                .to_string_lossy()
                .to_string();
            let content =
                std::fs::read_to_string(&f).with_context(|| format!("read l2 decision {:?}", f))?;
            out.push(MemorySlice {
                // Prefix the topic so the agent prompt clearly attributes
                // the slice to "what some OTHER project decided" — not
                // to the consumer's own past.
                topic: format!("l2/{rel}"),
                content,
            });
        }
        Ok(out)
    }

    /// Contract-aware L2 fan-in. For a consumer task, find every project
    /// in `projects_cfg` whose `contracts.provides` matches this task's
    /// project's `contracts.consumes`, and pull each producer's two most
    /// recent L2 decisions. Returns an empty Vec when the consumer has no
    /// `consumes` declaration or no producer matches.
    pub fn load_l2_for_contract(
        &self,
        consumer: &str,
        projects_cfg: &ProjectsConfig,
    ) -> Result<Vec<MemorySlice>> {
        let consumer_project = match projects_cfg.projects.get(consumer) {
            Some(p) => p,
            None => return Ok(vec![]),
        };
        let consumes_path = match &consumer_project.contracts.consumes {
            Some(p) if !p.trim().is_empty() => p.trim(),
            _ => return Ok(vec![]),
        };

        let mut producers: Vec<String> = Vec::new();
        for (name, p) in &projects_cfg.projects {
            if name == consumer {
                continue;
            }
            if let Some(prov) = &p.contracts.provides {
                if paths_logically_match(prov, consumes_path) {
                    producers.push(name.clone());
                }
            }
        }

        let mut out = Vec::new();
        for prod in producers {
            out.extend(self.load_recent_l2(&prod, L2_RECENT_PER_PROJECT)?);
        }
        Ok(out)
    }

    /// Topology-aware L2 fan-in. Pull the most recent L2 decisions from each
    /// project this one `depends_on`, so a task building on top of another
    /// project sees what that project recently decided — even when the link is
    /// a plain dependency rather than a declared contract. Complements
    /// [`load_l2_for_contract`](Self::load_l2_for_contract); together they
    /// cover both the formal-contract and the inferred-import edges that
    /// discovery records in `dependencies`.
    pub fn load_l2_for_dependencies(
        &self,
        project: &str,
        projects_cfg: &ProjectsConfig,
    ) -> Result<Vec<MemorySlice>> {
        let p = match projects_cfg.projects.get(project) {
            Some(p) => p,
            None => return Ok(vec![]),
        };
        let mut out = Vec::new();
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for dep in &p.dependencies {
            if dep == project || !seen.insert(dep.as_str()) {
                continue;
            }
            if !projects_cfg.projects.contains_key(dep) {
                continue;
            }
            out.extend(self.load_recent_l2(dep, L2_RECENT_PER_PROJECT)?);
        }
        Ok(out)
    }

    /// Keep only the `keep` most-recent L2 decisions for a project, deleting
    /// older ones. L2 is append-only per run, so without a cap a long-lived
    /// project's archive grows without bound — pure noise, since fan-in and
    /// `load_recent_l2` only ever read the newest few. Returns how many were
    /// removed. Newest = lexicographically largest filename (date-prefixed).
    pub fn prune_l2(&self, project: &str, keep: usize) -> Result<usize> {
        let dir = self.l2_root().join(project);
        if !dir.is_dir() {
            return Ok(0);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("read {:?}", dir))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        if files.len() <= keep {
            return Ok(0);
        }
        files.sort(); // oldest first
        let remove = files.len() - keep;
        let mut removed = 0;
        for f in files.into_iter().take(remove) {
            if std::fs::remove_file(&f).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Load every file under `l1_facts/<topic>/` for each requested topic.
    /// Files inside `<topic>/` are read as text and emitted as MemorySlice entries.
    pub fn load_for_topics(&self, topics: &[String]) -> Result<Vec<MemorySlice>> {
        let mut out = vec![];
        let dedup: std::collections::BTreeSet<&str> = topics.iter().map(String::as_str).collect();
        for topic in dedup {
            let topic_dir = self.l1_root().join(topic);
            if !topic_dir.is_dir() {
                continue;
            }
            for entry in walk(&topic_dir)? {
                let rel = entry
                    .strip_prefix(self.l1_root())
                    .unwrap_or(&entry)
                    .to_string_lossy()
                    .to_string();
                let content = std::fs::read_to_string(&entry)
                    .with_context(|| format!("read memory file {:?}", entry))?;
                out.push(MemorySlice {
                    topic: rel,
                    content,
                });
            }
        }
        Ok(out)
    }

    /// `topic -> [relative path]` listing of L1 facts.
    pub fn list_l1(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let mut by_topic: BTreeMap<String, Vec<String>> = Default::default();
        if !self.l1_root().exists() {
            return Ok(by_topic);
        }
        for topic_entry in std::fs::read_dir(self.l1_root())? {
            let topic_entry = topic_entry?;
            if !topic_entry.path().is_dir() {
                continue;
            }
            let topic = topic_entry.file_name().to_string_lossy().to_string();
            for f in walk(&topic_entry.path())? {
                let rel = f
                    .strip_prefix(topic_entry.path())
                    .unwrap_or(&f)
                    .to_string_lossy()
                    .to_string();
                by_topic.entry(topic.clone()).or_default().push(rel);
            }
        }
        Ok(by_topic)
    }

    pub fn read(&self, topic: &str, name: &str) -> Result<String> {
        paths::validate_path_component("memory topic", topic)?;
        paths::validate_path_component("memory name", name)?;
        let p = self.l1_root().join(topic).join(name);
        std::fs::read_to_string(&p).with_context(|| format!("read {:?}", p))
    }

    pub fn add(&self, topic: &str, name: &str, content: &str) -> Result<PathBuf> {
        paths::validate_path_component("memory topic", topic)?;
        paths::validate_path_component("memory name", name)?;
        let dir = self.l1_root().join(topic);
        paths::ensure_dir(&dir)?;
        let target = dir.join(name);
        std::fs::write(&target, content).with_context(|| format!("write {:?}", target))?;
        Ok(target)
    }

    pub fn delete(&self, topic: &str, name: &str) -> Result<()> {
        paths::validate_path_component("memory topic", topic)?;
        paths::validate_path_component("memory name", name)?;
        let target = self.l1_root().join(topic).join(name);
        if target.exists() {
            std::fs::remove_file(&target).with_context(|| format!("remove {:?}", target))?;
        }
        // Remove the topic directory if it becomes empty.
        let topic_dir = self.l1_root().join(topic);
        if topic_dir.exists() {
            if let Ok(mut entries) = std::fs::read_dir(&topic_dir) {
                if entries.next().is_none() {
                    let _ = std::fs::remove_dir(&topic_dir);
                }
            }
        }
        Ok(())
    }
}

/// Loose path equivalence for contract files. Match if the basename
/// matches, or if either is a suffix of the other after normalization.
/// Reason: producers usually declare `schemas/openapi.yaml`, consumers
/// usually declare `../server/schemas/openapi.yaml`. We don't try to
/// fs::canonicalize() because contract paths might point at files that
/// don't exist yet during scaffolding — we want the match to work even
/// pre-creation.
fn paths_logically_match(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let basename = |s: &str| -> String {
        s.rsplit('/')
            .next()
            .unwrap_or(s)
            .trim_end_matches(|c: char| c.is_whitespace())
            .to_string()
    };
    let ba = basename(a);
    let bb = basename(b);
    if ba.is_empty() || bb.is_empty() {
        return false;
    }
    if ba != bb {
        return false;
    }
    // Same basename, now require a non-trivial suffix overlap to avoid
    // matching e.g. both files literally named "schema.yaml" in unrelated
    // projects. Take the last two components if available.
    let tail = |s: &str| -> String {
        let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
        let n = parts.len();
        if n >= 2 {
            format!("{}/{}", parts[n - 2], parts[n - 1])
        } else {
            parts.join("/")
        }
    };
    tail(a) == tail(b) || ba == bb // fall back to basename match if no parent
}

#[cfg(test)]
mod contract_l2_tests {
    use super::*;
    use crate::config::{Contracts, Project, ProjectsConfig};
    use std::collections::BTreeMap;

    fn make_store(root: &Path) -> MemoryStore {
        std::fs::create_dir_all(root.join(L1_DIR)).unwrap();
        std::fs::create_dir_all(root.join(L2_DIR)).unwrap();
        MemoryStore {
            root: root.to_path_buf(),
        }
    }

    #[test]
    fn rejects_path_traversal_in_topic_or_name() {
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        assert!(store.read("..", "x").is_err());
        assert!(store.read("ok", "../../escape").is_err());
        assert!(store.add("../evil", "x", "data").is_err());
        assert!(store.delete("ok", "..").is_err());
        // A normal topic/name still works.
        assert!(store.add("topicA", "factA", "hello").is_ok());
        assert_eq!(store.read("topicA", "factA").unwrap(), "hello");
    }

    fn project_with_contracts(
        path: &str,
        provides: Option<&str>,
        consumes: Option<&str>,
    ) -> Project {
        Project {
            path: path.into(),
            r#type: None,
            stack: vec![],
            commands: BTreeMap::new(),
            contracts: Contracts {
                provides: provides.map(|s| s.into()),
                consumes: consumes.map(|s| s.into()),
            },
            dependencies: Vec::new(),
            memory_scope: vec![],
            agent: None,
            agent_model: None,
            cursor_model: None,
            model_profile: None,
            role: None,
            agent_profile: None,
            review_profile: None,
            copy_files: Vec::new(),
        }
    }

    #[test]
    fn prune_l2_keeps_newest_and_deletes_older() {
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let dir = store.l2_root().join("api");
        std::fs::create_dir_all(&dir).unwrap();
        for d in 1..=5 {
            std::fs::write(dir.join(format!("2026-05-0{d}-x-run{d}.md")), "x").unwrap();
        }

        let removed = store.prune_l2("api", 2).unwrap();
        assert_eq!(removed, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        // The two newest (largest filenames) survive.
        assert_eq!(left, vec!["2026-05-04-x-run4.md", "2026-05-05-x-run5.md"]);

        // Idempotent: pruning again removes nothing.
        assert_eq!(store.prune_l2("api", 2).unwrap(), 0);
    }

    #[test]
    fn dependency_fan_in_pulls_l2_from_depended_on_projects() {
        // web depends on shared (e.g. an edge discovery inferred from a source
        // import) with no formal contract — topology fan-in still pulls
        // shared's recent decisions.
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let shared_l2 = store.l2_root().join("shared");
        std::fs::create_dir_all(&shared_l2).unwrap();
        std::fs::write(
            shared_l2.join("2026-05-20-add-email.md"),
            "added email to the User type",
        )
        .unwrap();

        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        cfg.projects.insert(
            "shared".into(),
            project_with_contracts("./shared", None, None),
        );
        let mut web = project_with_contracts("./web", None, None);
        web.dependencies = vec!["shared".into()];
        cfg.projects.insert("web".into(), web);

        let slices = store.load_l2_for_dependencies("web", &cfg).unwrap();
        assert_eq!(slices.len(), 1);
        assert!(slices[0].content.contains("email"));

        // A project with no dependencies pulls nothing.
        assert!(store
            .load_l2_for_dependencies("shared", &cfg)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn load_recent_l2_returns_files_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let api_l2 = store.l2_root().join("api");
        std::fs::create_dir_all(&api_l2).unwrap();
        std::fs::write(api_l2.join("2026-01-01-foo-run1.md"), "old").unwrap();
        std::fs::write(api_l2.join("2026-05-15-bar-run2.md"), "newer").unwrap();
        std::fs::write(api_l2.join("2026-06-01-baz-run3.md"), "newest").unwrap();

        let slices = store.load_recent_l2("api", 2).unwrap();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].content, "newest");
        assert_eq!(slices[1].content, "newer");
        // topic is prefixed with l2/ so the agent prompt distinguishes
        assert!(slices[0].topic.starts_with("l2/api/"));
    }

    #[test]
    fn contract_aware_pulls_producer_l2() {
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let server_l2 = store.l2_root().join("server");
        std::fs::create_dir_all(&server_l2).unwrap();
        std::fs::write(
            server_l2.join("2026-05-15-add-nickname.md"),
            "we added nickname column",
        )
        .unwrap();

        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        cfg.projects.insert(
            "server".into(),
            project_with_contracts("./server", Some("schemas/openapi.yaml"), None),
        );
        cfg.projects.insert(
            "web".into(),
            project_with_contracts("./web", None, Some("../server/schemas/openapi.yaml")),
        );

        let slices = store.load_l2_for_contract("web", &cfg).unwrap();
        assert_eq!(slices.len(), 1);
        assert!(slices[0].content.contains("nickname"));
    }

    #[test]
    fn contract_aware_skips_self_referential_match() {
        // If a project provides AND consumes the same file (degenerate
        // config), we don't want to inject its own L2 back into itself —
        // that's already covered by L1 topics + memory_scope.
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let l2 = store.l2_root().join("server");
        std::fs::create_dir_all(&l2).unwrap();
        std::fs::write(l2.join("2026-05-15-x.md"), "self-decision").unwrap();

        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        cfg.projects.insert(
            "server".into(),
            project_with_contracts(
                "./server",
                Some("schemas/openapi.yaml"),
                Some("schemas/openapi.yaml"),
            ),
        );

        let slices = store.load_l2_for_contract("server", &cfg).unwrap();
        assert!(slices.is_empty());
    }

    #[test]
    fn no_consumes_means_no_injection() {
        let tmp = tempfile::tempdir().unwrap();
        let store = make_store(tmp.path());
        let mut cfg = ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: BTreeMap::new(),
        };
        cfg.projects
            .insert("lib".into(), project_with_contracts("./lib", None, None));
        let slices = store.load_l2_for_contract("lib", &cfg).unwrap();
        assert!(slices.is_empty());
    }
}

#[cfg(test)]
mod path_match_tests {
    use super::paths_logically_match;

    #[test]
    fn identical_paths_match() {
        assert!(paths_logically_match(
            "schemas/openapi.yaml",
            "schemas/openapi.yaml"
        ));
    }

    #[test]
    fn relative_vs_absolute_match() {
        assert!(paths_logically_match(
            "schemas/openapi.yaml",
            "../server/schemas/openapi.yaml"
        ));
    }

    #[test]
    fn different_basenames_dont_match() {
        assert!(!paths_logically_match(
            "schemas/openapi.yaml",
            "schemas/users.proto"
        ));
    }

    #[test]
    fn empty_paths_dont_match() {
        assert!(!paths_logically_match("", ""));
    }
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p)?);
        } else if p.is_file() {
            // skip hidden files
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
