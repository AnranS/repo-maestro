use anyhow::{Context, Result};

use crate::cli::{samples, SkillCmd};

pub fn run(c: SkillCmd) -> Result<()> {
    use crate::skills::{self, SkillScope};

    let to_scope = |s: &str| -> SkillScope { SkillScope::from_dir(s) };

    match c {
        SkillCmd::Sync => {
            let (cursor, claude) = crate::skills::sync_all()?;
            println!(
                "→ mirrored to .cursor/rules ({cursor} files) and .claude/skills ({claude} files)"
            );
            println!("  these are auto-updated whenever a skill is saved.");
            Ok(())
        }
        SkillCmd::Update { name, force } => {
            let results = samples::update_bundled_skills(name.as_deref(), force)?;
            print_update_results(&results);
            if !force {
                println!("hint: existing local edits are skipped; pass --force to overwrite.");
            }
            Ok(())
        }
        SkillCmd::Ls => {
            let all = skills::list_all()?;
            if all.values().all(|v| v.is_empty()) {
                println!("(no skills — `maestro skill new _global write-test` to scaffold one)");
                return Ok(());
            }
            for (scope, items) in &all {
                if items.is_empty() {
                    continue;
                }
                println!("\n[{scope}]");
                for s in items {
                    let desc = s.description.as_deref().unwrap_or("(no description)");
                    println!("  {:<24}  {desc}", s.name);
                }
            }
            Ok(())
        }
        SkillCmd::Show { scope, name } => {
            let s = skills::load(&to_scope(&scope), &name)?;
            println!("# {} [{}]", s.name, scope);
            if let Some(d) = &s.description {
                println!("description: {d}");
            }
            if let Some(t) = &s.trigger {
                println!("trigger: {t}");
            }
            println!("\n{}", s.content);
            Ok(())
        }
        SkillCmd::Add { scope, name, file } => {
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("read source file {:?}", file))?;
            let p = skills::save(&to_scope(&scope), &name, &content)?;
            println!("wrote {:?}", p);
            Ok(())
        }
        SkillCmd::New {
            scope,
            name,
            description,
        } => {
            let desc =
                description.unwrap_or_else(|| "describe what this skill does in one line".into());
            let template = format!(
                "---\ndescription: {desc}\ntrigger: (optional phrase that should call this skill)\n---\n\n# {name}\n\nWhen to use it / preconditions:\n\n- …\n\nSteps:\n\n1. …\n2. …\n\nOutputs:\n\n- …\n"
            );
            let p = skills::save(&to_scope(&scope), &name, &template)?;
            println!("scaffolded {:?}", p);
            Ok(())
        }
        SkillCmd::Rm { scope, name } => {
            skills::delete(&to_scope(&scope), &name)?;
            println!("removed {}/{}", scope, name);
            Ok(())
        }
    }
}

pub(crate) fn print_update_results(results: &[samples::SkillUpdateResult]) {
    for result in results {
        let action = skill_update_label(result.status);
        println!(
            "{action:<9} {}/{} -> {}",
            result.scope,
            result.name,
            result.path.display()
        );
    }
}

fn skill_update_label(status: samples::SkillUpdateStatus) -> &'static str {
    match status {
        samples::SkillUpdateStatus::Added => "added",
        samples::SkillUpdateStatus::Updated => "updated",
        samples::SkillUpdateStatus::Unchanged => "unchanged",
        samples::SkillUpdateStatus::Skipped => "skipped",
    }
}
