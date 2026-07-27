use super::{open, persist};
use anyhow::Result;
use std::path::Path;

pub(crate) fn remove(path: &Path, selectors: &[String]) -> Result<()> {
    let mut file = open(path)?;
    let removed = file.remove_batch(selectors)?;
    for item in &removed {
        println!("Removed {}", item.key);
    }
    if !removed.is_empty() {
        persist(&file)?;
    }
    Ok(())
}
