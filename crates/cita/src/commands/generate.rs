use super::find_manifest;
use anyhow::Result;
use cita_manifest::Manifest;
use std::path::Path;

pub(crate) fn generate(cwd: &Path) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    manifest.generate()?;
    println!("Generated {}", manifest.bibliography_path().display());
    Ok(())
}
