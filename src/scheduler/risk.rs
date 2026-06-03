//! Change-risk classification for approval gates.
//!
//! Turns "what did this task actually change?" into a high/low signal plus
//! human reasons, so an approval card can show *why* a step warrants review —
//! the antidote to approval fatigue (the 2026 UX failure mode where every gate
//! gets rubber-stamped because none of them say what's at stake).

/// Risk verdict for a task's pending change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChangeRisk {
    /// "high" | "low".
    pub level: String,
    /// Why — shown on the approval card.
    pub reasons: Vec<String>,
}

/// Files touched count above which a change is "large".
const LARGE_FILE_COUNT: usize = 20;

/// Last two path components, for logical contract matching (provider-relative
/// vs worktree-relative spellings of the same file).
fn tail2(s: &str) -> String {
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    let n = parts.len();
    if n >= 2 {
        format!("{}/{}", parts[n - 2], parts[n - 1])
    } else {
        parts.join("/")
    }
}

fn touches_contract(path: &str, contract_paths: &[String]) -> bool {
    let pt = tail2(path);
    contract_paths.iter().any(|c| {
        let c = c.trim();
        !c.is_empty() && (path == c || pt == tail2(c))
    })
}

/// A change to this file type ripples to downstream consumers — an RPC/data
/// IDL, an API spec, or a DB migration — so it warrants human judgment even
/// when the project declared no contract path. Deliberately excludes
/// gazelle-churned `BUILD.bazel` and lockfiles (frequent + low-judgment).
fn is_contract_or_schema_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let p = std::path::Path::new(&lower);
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        if matches!(ext, "thrift" | "proto" | "idl" | "graphql" | "fbs" | "avsc") {
            return true;
        }
    }
    if lower.contains("/migrations/") || lower.contains("/migrate/") {
        return true;
    }
    let base = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
    matches!(
        base,
        "openapi.yaml"
            | "openapi.yml"
            | "openapi.json"
            | "swagger.yaml"
            | "swagger.yml"
            | "swagger.json"
    )
}

/// Classify a worktree change. High-risk when it changes a declared contract,
/// deletes files, or is large; otherwise low. `files` is `(status, path)` from
/// `gitops::changed_files_with_status`; `contract_paths` is the project's
/// declared provides + consumes.
pub fn classify_change_risk(files: &[(char, String)], contract_paths: &[String]) -> ChangeRisk {
    let mut reasons = Vec::new();

    let contract_hits: Vec<&String> = files
        .iter()
        .filter(|(_, p)| touches_contract(p, contract_paths))
        .map(|(_, p)| p)
        .collect();
    if !contract_hits.is_empty() {
        reasons.push(format!(
            "changes a declared contract ({})",
            contract_hits
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // Contract/schema files are high blast-radius even when the project didn't
    // declare them as a contract path — e.g. a Bazel/Go monorepo discovered
    // from a registry has no declared `provides`, yet editing an RPC `.thrift`
    // or a DB migration ripples to every consumer. Flag by file type so the
    // approval gate puts these in front of a human regardless.
    let schema_hits: Vec<&String> = files
        .iter()
        .filter(|(_, p)| is_contract_or_schema_file(p))
        .filter(|(_, p)| !touches_contract(p, contract_paths)) // avoid double-listing
        .map(|(_, p)| p)
        .collect();
    if !schema_hits.is_empty() {
        reasons.push(format!(
            "changes a contract/schema file ({})",
            schema_hits
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let deletes = files.iter().filter(|(c, _)| *c == 'D').count();
    if deletes > 0 {
        reasons.push(format!("{deletes} file deletion(s)"));
    }

    if files.len() > LARGE_FILE_COUNT {
        reasons.push(format!("large change: {} files", files.len()));
    }

    if reasons.is_empty() {
        ChangeRisk {
            level: "low".to_string(),
            reasons: vec![format!(
                "routine change ({} file(s), no contract/deletes)",
                files.len()
            )],
        }
    } else {
        ChangeRisk {
            level: "high".to_string(),
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_change_is_high_risk() {
        let files = vec![('M', "src/index.js".to_string())];
        let r = classify_change_risk(&files, &["../../libs/shared/src/index.js".to_string()]);
        assert_eq!(r.level, "high");
        assert!(r.reasons[0].contains("contract"));
    }

    #[test]
    fn deletion_is_high_risk() {
        let files = vec![
            ('D', "old/legacy.rs".to_string()),
            ('M', "a.rs".to_string()),
        ];
        let r = classify_change_risk(&files, &[]);
        assert_eq!(r.level, "high");
        assert!(r.reasons.iter().any(|x| x.contains("deletion")));
    }

    #[test]
    fn routine_change_is_low_risk() {
        let files = vec![
            ('M', "src/util.rs".to_string()),
            ('A', "src/new.rs".to_string()),
        ];
        let r = classify_change_risk(&files, &["schemas/api.yaml".to_string()]);
        assert_eq!(r.level, "low");
    }

    #[test]
    fn idl_and_migration_changes_are_high_risk_without_declared_contract() {
        // A registry-discovered monorepo module has no declared contract path,
        // yet an RPC IDL or DB migration edit must still gate for human review.
        for path in [
            "idl/rpc/orders_service/service.thrift",
            "app/svc/api/order.proto",
            "app/svc/db/migrations/0007_add_index.sql",
        ] {
            let r = classify_change_risk(&[('M', path.to_string())], &[]);
            assert_eq!(r.level, "high", "{path} should be high-risk");
            assert!(
                r.reasons.iter().any(|x| x.contains("contract/schema")),
                "{path}: {:?}",
                r.reasons
            );
        }
    }

    #[test]
    fn routine_build_and_lockfiles_stay_low_risk() {
        // gazelle-churned BUILD.bazel and lockfiles change constantly — flagging
        // them would make the gate noise. They must stay low.
        for path in ["app/svc/BUILD.bazel", "go.sum", "pnpm-lock.yaml"] {
            let r = classify_change_risk(&[('M', path.to_string())], &[]);
            assert_eq!(r.level, "low", "{path} should stay low-risk");
        }
    }
}
