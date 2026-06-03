use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    maestro::cli::main_entry().await
}
