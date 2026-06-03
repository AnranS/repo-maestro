//! `mst` — a short alias for the `maestro` CLI. Identical behaviour; fewer
//! keystrokes. Both binaries share `maestro::cli::main_entry`.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    maestro::cli::main_entry().await
}
