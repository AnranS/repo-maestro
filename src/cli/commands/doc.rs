//! `maestro doc` — open the embedded documentation site or print pages.

use anyhow::{Context, Result};

use crate::cli::util::open_in_browser;
use crate::cli::{DocArgs, DocSubcmd};

pub async fn run(a: DocArgs) -> Result<()> {
    match a.subcmd {
        Some(DocSubcmd::Ls { lang }) => ls(lang.as_deref()),
        Some(DocSubcmd::Show { page, lang }) => show(&page, lang.as_deref()),
        None => open(a).await,
    }
}

async fn open(a: DocArgs) -> Result<()> {
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let addr = format!("{}:{}", a.host, a.port);
    let mut target_fragment = match a.page.as_deref() {
        Some(p) if !p.is_empty() => format!("#docs/{p}"),
        _ => "#docs".to_string(),
    };
    if let Some(lang) = a.lang.as_deref() {
        target_fragment.push_str(&format!("?lang={lang}"));
    }
    let url = format!("http://{addr}/{target_fragment}");

    // If something is already listening on the port, assume it's a friendly
    // maestro ui (or compatible) and just open the browser. This means `maestro doc`
    // is safe to invoke repeatedly while a dashboard is already up.
    let already_up = TcpStream::connect_timeout(
        &addr.parse().context("parse host:port")?,
        Duration::from_millis(120),
    )
    .is_ok();

    if !already_up {
        let exe = std::env::current_exe().context("locate maestro executable")?;
        let log_path = std::env::temp_dir().join("maestro-ui.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open log {:?}", log_path))?;
        Command::new(&exe)
            .args(["ui", "--host", &a.host, "--port"])
            .arg(a.port.to_string())
            .stdin(Stdio::null())
            .stdout(log_file.try_clone()?)
            .stderr(log_file)
            .spawn()
            .context("spawn `maestro ui`")?;

        tokio::time::sleep(Duration::from_millis(700)).await;
        println!("→ dashboard:  http://{addr}");
        println!("→ logs:       {}", log_path.display());
    } else {
        println!("→ dashboard already running at http://{addr}");
    }
    println!("→ docs:       {url}");

    if !a.no_browser {
        let _ = open_in_browser(&url);
    }
    Ok(())
}

fn ls(lang: Option<&str>) -> Result<()> {
    let idx = crate::docs::load_index(lang)?;
    println!(
        "\n{} — {}  (lang={})\n",
        idx.title, idx.description, idx.lang
    );
    for g in &idx.groups {
        println!("[{}] {}", g.id, g.title);
        for p in &g.pages {
            println!("  {:<22}  {}", p.id, p.title);
        }
        println!();
    }
    if idx.languages.len() > 1 {
        println!("languages available: {}", idx.languages.join(", "));
    }
    Ok(())
}

fn show(page_id: &str, lang: Option<&str>) -> Result<()> {
    let idx = crate::docs::load_index(lang)?;
    let page = idx
        .groups
        .iter()
        .flat_map(|g| g.pages.iter())
        .find(|p| p.id == page_id)
        .ok_or_else(|| anyhow::anyhow!("unknown page '{page_id}'. Try `maestro doc ls`."))?;
    let body = crate::docs::load_page(&page.file, lang)?;
    print!("{body}");
    Ok(())
}
