//! Minimal starter files per stack. We deliberately keep them tiny — the goal
//! is "a runnable, gitignored, IDE-friendly skeleton", not a production
//! template. The chat agent / user can flesh them out via normal maestro runs.

use std::path::Path;

pub struct File {
    pub rel_path: &'static str,
    pub content: String,
}

pub fn scaffold_files(stack: &[String], module_name: &str) -> Vec<File> {
    let stack_lower: Vec<String> = stack.iter().map(|s| s.to_lowercase()).collect();
    let has = |needle: &str| stack_lower.iter().any(|s| s == needle);

    if has("python") || has("fastapi") || has("flask") {
        return python_files(module_name, has("fastapi"));
    }
    if has("rust") {
        return rust_files(module_name);
    }
    if has("go") {
        return go_files(module_name);
    }
    if has("typescript") || has("react") || has("vite") || has("node") {
        return node_files(module_name, has("react"));
    }
    if has("bash") || has("shell") {
        return shell_files(module_name);
    }

    generic_files(module_name)
}

fn python_files(name: &str, fastapi: bool) -> Vec<File> {
    let mut out = vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, "Python"),
        },
        File {
            rel_path: ".gitignore",
            content: PY_GITIGNORE.into(),
        },
        File {
            rel_path: "pyproject.toml",
            content: format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\ndependencies = []\n"
            ),
        },
    ];
    if fastapi {
        out.push(File {
            rel_path: "main.py",
            content: r#"from fastapi import FastAPI

app = FastAPI()


@app.get("/health")
def health() -> dict[str, str]:
    return {"status": "ok"}
"#
            .into(),
        });
    } else {
        out.push(File {
            rel_path: "main.py",
            content: format!(
                "\"\"\"{name} entry point.\"\"\"\n\n\ndef main() -> None:\n    print(\"hello from {name}\")\n\n\nif __name__ == \"__main__\":\n    main()\n"
            ),
        });
    }
    out
}

fn rust_files(name: &str) -> Vec<File> {
    vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, "Rust"),
        },
        File {
            rel_path: ".gitignore",
            content: RUST_GITIGNORE.into(),
        },
        File {
            rel_path: "Cargo.toml",
            content: format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"
            ),
        },
        File {
            rel_path: "src/main.rs",
            content: format!("fn main() {{\n    println!(\"hello from {name}\");\n}}\n"),
        },
    ]
}

fn go_files(name: &str) -> Vec<File> {
    vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, "Go"),
        },
        File {
            rel_path: ".gitignore",
            content: GO_GITIGNORE.into(),
        },
        File {
            rel_path: "go.mod",
            content: format!("module {name}\n\ngo 1.22\n"),
        },
        File {
            rel_path: "main.go",
            content: format!(
                "package main\n\nimport \"fmt\"\n\nfunc main() {{\n    fmt.Println(\"hello from {name}\")\n}}\n"
            ),
        },
    ]
}

fn node_files(name: &str, react: bool) -> Vec<File> {
    let mut out = vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, if react { "React + TypeScript" } else { "TypeScript" }),
        },
        File {
            rel_path: ".gitignore",
            content: NODE_GITIGNORE.into(),
        },
        File {
            rel_path: "package.json",
            content: if react {
                format!(
                    "{{\n  \"name\": \"{name}\",\n  \"private\": true,\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"scripts\": {{\n    \"dev\": \"vite\",\n    \"build\": \"vite build\",\n    \"lint\": \"echo TODO\"\n  }}\n}}\n"
                )
            } else {
                format!(
                    "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"main\": \"src/index.ts\",\n  \"scripts\": {{\n    \"build\": \"tsc -b\",\n    \"lint\": \"echo TODO\"\n  }}\n}}\n"
                )
            },
        },
        File {
            rel_path: "tsconfig.json",
            content: "{\n  \"compilerOptions\": {\n    \"target\": \"ES2022\",\n    \"module\": \"ESNext\",\n    \"moduleResolution\": \"bundler\",\n    \"strict\": true,\n    \"esModuleInterop\": true,\n    \"skipLibCheck\": true\n  },\n  \"include\": [\"src\"]\n}\n"
                .into(),
        },
    ];
    if react {
        out.push(File {
            rel_path: "index.html",
            content: format!(
                "<!doctype html>\n<html><head><meta charset=utf-8><title>{name}</title></head><body><div id=root></div><script type=module src=/src/main.tsx></script></body></html>\n"
            ),
        });
        out.push(File {
            rel_path: "src/main.tsx",
            content: format!(
                "import {{ createRoot }} from \"react-dom/client\"\n\ncreateRoot(document.getElementById(\"root\")!).render(<h1>{name}</h1>)\n"
            ),
        });
    } else {
        out.push(File {
            rel_path: "src/index.ts",
            content: format!("console.log(\"hello from {name}\")\n"),
        });
    }
    out
}

fn shell_files(name: &str) -> Vec<File> {
    vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, "Bash CLI"),
        },
        File {
            rel_path: ".gitignore",
            content: "*.log\n".into(),
        },
        File {
            rel_path: format!("{name}.sh").leak(),
            content: format!(
                "#!/usr/bin/env bash\nset -euo pipefail\n\necho \"hello from {name}\"\n"
            ),
        },
    ]
}

fn generic_files(name: &str) -> Vec<File> {
    vec![
        File {
            rel_path: "README.md",
            content: readme_stub(name, "generic"),
        },
        File {
            rel_path: ".gitignore",
            content: ".DS_Store\n".into(),
        },
    ]
}

fn readme_stub(name: &str, stack: &str) -> String {
    format!(
        "# {name}\n\nScaffolded by `maestro scaffold` ({stack} starter).\n\n## What this is\n\n_(Describe the module's purpose. The maestro Planner can fill this in via memory L2 after the first successful PR.)_\n"
    )
}

// ─── shared gitignores ───
const PY_GITIGNORE: &str = r#"__pycache__/
*.py[cod]
.venv/
venv/
.env
*.egg-info/
dist/
build/
.pytest_cache/
.mypy_cache/
.ruff_cache/
"#;

const RUST_GITIGNORE: &str = r#"target/
Cargo.lock.bak
*.bk
"#;

const GO_GITIGNORE: &str = r#"bin/
*.exe
*.test
*.out
vendor/
"#;

const NODE_GITIGNORE: &str = r#"node_modules/
dist/
.env
.env.*
*.log
.vite/
"#;

/// Drop a *placeholder* contract file (OpenAPI yaml, proto, etc.) at the
/// indicated path so consumers can start typing imports even before the
/// producer is implemented.
pub fn contract_placeholder(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("contract");
    Some(match ext {
        "yaml" | "yml" => format!(
            "openapi: 3.0.0\ninfo:\n  title: {stem}\n  version: 0.1.0\npaths: {{}}\ncomponents:\n  schemas: {{}}\n"
        ),
        "proto" => format!(
            "syntax = \"proto3\";\n\npackage {stem};\n\n// TODO: declare your contract here.\nmessage Placeholder {{}}\n"
        ),
        "graphql" | "gql" | "graphqls" => "schema { query: Query }\ntype Query { _placeholder: String }\n".into(),
        "ts" | "tsx" | "d.ts" => format!("// {stem} contract\n\nexport type Placeholder = unknown\n"),
        "rs" => format!("// {stem} contract\n\npub struct Placeholder;\n"),
        "py" => format!("\"\"\"{stem} contract.\"\"\"\n"),
        "json" => format!(
            "{{\n  \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n  \"title\": \"{stem}\",\n  \"type\": \"object\",\n  \"properties\": {{}}\n}}\n"
        ),
        _ => format!("# {stem}\n\nPlaceholder contract — flesh this out.\n"),
    })
}
