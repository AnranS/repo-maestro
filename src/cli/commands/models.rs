use anyhow::Result;

use crate::cli::ModelsArgs;

pub async fn run(a: ModelsArgs) -> Result<()> {
    if a.refresh {
        match crate::models::refresh().await {
            Ok(list) => {
                println!("→ refreshed ({} models)", list.len());
                for m in &list {
                    let label = m.label.as_deref().unwrap_or("");
                    println!("  {:<24} {label}", m.id);
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("→ refresh failed: {e:#}\n  falling back to cached list");
            }
        }
    }
    let list = crate::models::load_cached()?;
    println!("→ {} models", list.len());
    for m in &list {
        let label = m.label.as_deref().unwrap_or("");
        println!("  {:<24} {label}", m.id);
    }
    Ok(())
}
