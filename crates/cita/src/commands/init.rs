use super::ensure_cache_layout;
use crate::git;
use anyhow::Result;
use cita_manifest::{BIBLIOGRAPHY_FILE, MANIFEST_FILE, Manifest};
use std::path::Path;

pub(crate) fn init(cwd: &Path, here: bool) -> Result<()> {
    let directory = if here {
        cwd.to_path_buf()
    } else {
        git::repository_root(cwd)?.unwrap_or_else(|| cwd.to_path_buf())
    };
    let manifest_path = directory.join(MANIFEST_FILE);
    let bibliography_path = directory.join(BIBLIOGRAPHY_FILE);
    let existed = manifest_path.exists();
    if existed {
        Manifest::load_verified(&manifest_path)?;
    } else if bibliography_path.exists() {
        Manifest::import_existing(&directory)?;
    } else {
        Manifest::create(&directory)?;
    }
    ensure_cache_layout(&directory)?;
    if existed {
        println!("Already initialized {}", manifest_path.display());
    } else {
        println!("Initialized {}", manifest_path.display());
    }
    Ok(())
}
