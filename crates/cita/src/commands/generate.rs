use super::find_manifest;
use anyhow::Result;
use cita_manifest::Manifest;
use std::path::{Path, PathBuf};

pub(crate) fn generate(cwd: &Path) -> Result<()> {
    let path = generate_outcome(cwd)?;
    println!("Generated {}", path.display());
    Ok(())
}

pub(crate) fn generate_outcome(cwd: &Path) -> Result<PathBuf> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    manifest.generate()?;
    Ok(manifest.bibliography_path().to_path_buf())
}
