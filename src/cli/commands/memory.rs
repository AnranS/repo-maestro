use anyhow::{Context, Result};

use crate::cli::MemoryCmd;
use crate::memory::MemoryStore;

pub fn run(c: MemoryCmd) -> Result<()> {
    let store = MemoryStore::open()?;
    match c {
        MemoryCmd::List => {
            let by_topic = store.list_l1()?;
            if by_topic.is_empty() {
                println!("(no L1 facts yet — drop files into {:?})", store.l1_root());
                return Ok(());
            }
            for (topic, files) in &by_topic {
                println!(
                    "{topic} ({} file{})",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
                );
                for f in files {
                    println!("  - {f}");
                }
            }
            Ok(())
        }
        MemoryCmd::Show { topic, name } => {
            print!("{}", store.read(&topic, &name)?);
            Ok(())
        }
        MemoryCmd::Add { topic, name, file } => {
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("read source file {:?}", file))?;
            let target = store.add(&topic, &name, &content)?;
            println!("wrote {:?}", target);
            Ok(())
        }
        MemoryCmd::Path => {
            println!("{}", store.root().display());
            Ok(())
        }
    }
}
