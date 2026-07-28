use super::open;
use anyhow::{Result, bail};
use std::path::Path;

/// Validate the bibliography and report every problem at once.
///
/// This replaces the old `generate`: with no derived artifact there is nothing
/// to repair, but the file is now hand-edited, so it is worth asking whether it
/// still holds together.
pub(crate) fn check(path: &Path) -> Result<()> {
    let file = open(path)?;
    let diagnostics = file.check();
    if diagnostics.is_empty() {
        println!(
            "{} is valid: {} references",
            file.path().display(),
            file.entries().len()
        );
        return Ok(());
    }
    for problem in &diagnostics {
        match &problem.key {
            Some(key) => eprintln!("{key}: {}", problem.message),
            None => eprintln!("{}", problem.message),
        }
    }
    bail!(
        "{} has {} problem{}",
        file.path().display(),
        diagnostics.len(),
        if diagnostics.len() == 1 { "" } else { "s" }
    )
}
