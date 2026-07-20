use super::find_manifest;
use anyhow::Result;
use cita_manifest::Manifest;
use std::path::Path;

pub(crate) fn remove(cwd: &Path, selectors: &[String]) -> Result<()> {
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    for item in manifest.remove_batch(selectors)? {
        println!("Removed {}", item.key);
    }
    Ok(())
}
