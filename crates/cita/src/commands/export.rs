use super::{Target, global_root, resolve_target};
use anyhow::{Context, Result, bail};
use cita_bibliography::insert_field;
use cita_core::ReferenceSource;
use cita_documents::arxiv_pdf_url;
use cita_manifest::{Library, SourceSnapshot, atomic_write};
use std::{
    fs,
    path::{Path, PathBuf},
};

const URL_FIELD: &str = "url";

/// Write one URL-enriched bibliography export.
pub(crate) fn export(target: &Target, caller: &Path, output: Option<&Path>) -> Result<()> {
    let path = export_outcome(target, caller, output)?;
    println!("Exported {}", path.display());
    Ok(())
}

fn export_outcome(target: &Target, caller: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let manifest = target.load()?;
    let path = match output {
        Some(output) if output.is_absolute() => output.to_path_buf(),
        Some(output) => caller.join(output),
        None => caller.join(format!("{}.bib", target.name)),
    };
    ensure_outside_store(&path, target.store_root())?;
    let rendered = manifest.render_derived(derive_entry)?;
    atomic_write(&path, rendered.as_bytes())?;
    Ok(path)
}

/// Export every shelf into an existing directory, continuing after failures.
pub(crate) fn batch_export(caller: &Path, output: Option<&Path>) -> Result<bool> {
    let library = Library::open_or_create(global_root()?)?;
    let directory = match output {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => caller.join(path),
        None => caller.to_path_buf(),
    };
    if !directory.is_dir() {
        bail!(
            "batch export destination {} is not an existing directory",
            directory.display()
        );
    }
    let mut failed = false;
    for name in library.shelves() {
        let outcome = resolve_target(Some(name)).and_then(|target| {
            let output = directory.join(format!("{name}.bib"));
            export_outcome(&target, caller, Some(&output))
        });
        match outcome {
            Ok(path) => println!("Shelf {name}: exported {}", path.display()),
            Err(error) => {
                failed = true;
                println!("Shelf {name}: failed:");
                for line in format!("{error:#}").lines() {
                    println!("  {line}");
                }
            }
        }
    }
    Ok(failed)
}

fn derive_entry(key: &str, source: &SourceSnapshot, entry: String) -> Result<String> {
    let reference = source
        .project()
        .with_context(|| format!("could not project `{key}`"))?;
    let Some(arxiv) = reference.identifiers.arxiv.first() else {
        return Ok(entry);
    };
    let url = arxiv_pdf_url(arxiv)
        .with_context(|| format!("could not build an arXiv URL for `{key}`"))?;
    Ok(insert_field(&entry, URL_FIELD, url.as_str())?)
}

fn ensure_outside_store(path: &Path, store: &Path) -> Result<()> {
    let target = resolved(path)?;
    let store = fs::canonicalize(store)
        .with_context(|| format!("could not resolve cita home {}", store.display()))?;
    if target == store || target.starts_with(&store) {
        bail!(
            "refusing to write an export inside the global cita store {}",
            store.display()
        );
    }
    Ok(())
}

fn resolved(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("could not resolve {}", path.display()))?;
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return Ok(canonical);
    }
    let (Some(parent), Some(file)) = (absolute.parent(), absolute.file_name()) else {
        bail!("could not resolve {}", absolute.display());
    };
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("could not resolve the directory {}", parent.display()))?;
    Ok(parent.join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_descendants_are_rejected() {
        let store = tempfile::tempdir().unwrap();
        assert!(ensure_outside_store(&store.path().join("out.bib"), store.path()).is_err());
    }
}
