use super::{open, persist};
use anyhow::Result;
use std::path::Path;

pub(crate) fn rekey(path: &Path, selector: &str, new_key: &str) -> Result<()> {
    let mut file = open(path)?;
    let old = file.rekey(selector, new_key)?;
    if old == new_key {
        println!("{new_key} already holds this reference");
        return Ok(());
    }
    println!("Renamed {old} -> {new_key}");
    persist(&file)?;
    Ok(())
}
