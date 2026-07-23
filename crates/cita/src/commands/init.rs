use super::ensure_cache_layout;
use anyhow::{Result, bail};
use cita_manifest::{BIBLIOGRAPHY_FILE, LIBRARY_FILE, MANIFEST_FILE, Manifest};
use std::path::Path;

pub(crate) fn init(cwd: &Path, path: Option<&Path>) -> Result<()> {
    let directory = match path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => cwd.join(path),
        None => cwd.to_path_buf(),
    };
    if !directory.is_dir() {
        bail!(
            "initialization path {} is not an existing directory",
            directory.display()
        );
    }
    if directory.join(LIBRARY_FILE).is_file() {
        bail!(
            "cannot initialize a shelf at library root {}; use `cita library shelf <name> init`",
            directory.display()
        );
    }
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
