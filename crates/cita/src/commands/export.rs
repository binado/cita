use super::{Target, report_shelf_failure, summarize_batch, target_in};
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
pub(crate) fn export(target: &Target<'_>, caller: &Path, output: Option<&Path>) -> Result<()> {
    let path = export_outcome(target, caller, output)?;
    println!("Exported {}", path.display());
    Ok(())
}

fn export_outcome(target: &Target<'_>, caller: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let manifest = target.load()?;
    let path = match output {
        Some(output) if output.is_absolute() => output.to_path_buf(),
        Some(output) => caller.join(output),
        None => caller.join(format!("{}.bib", target.name())),
    };
    ensure_outside_store(&path, target.store_root())?;
    let rendered = manifest.render_derived(derive_entry)?;
    atomic_write(&path, rendered.as_bytes())?;
    Ok(path)
}

/// Export every shelf into an existing directory, continuing after failures.
///
/// Like every batch form this is a sequence of independent exports, not one
/// transaction; successes go to stdout and failures to stderr so redirecting the
/// data stream still surfaces the errors.
pub(crate) fn batch_export(
    library: &Library,
    caller: &Path,
    output: Option<&Path>,
) -> Result<bool> {
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
    let mut failed = 0usize;
    for name in library.shelves() {
        let outcome = target_in(library, name)
            .map_err(Into::into)
            .and_then(|target| {
                let output = directory.join(format!("{name}.bib"));
                export_outcome(&target, caller, Some(&output))
            });
        match outcome {
            Ok(path) => println!("Shelf {name}: exported {}", path.display()),
            Err(error) => {
                failed += 1;
                report_shelf_failure(name, &error);
            }
        }
    }
    Ok(summarize_batch(failed, library.shelves().len()))
}

/// Add the derived arXiv PDF URL to one re-keyed entry that has an arXiv ID.
///
/// Only entries without an authored `url` are touched; `insert_field` enforces that.
fn derive_entry(key: &str, source: &SourceSnapshot, entry: String) -> Result<String> {
    let reference = source
        .project()
        .with_context(|| format!("could not project `{key}`"))?;
    // The projected identity already carries INSPIRE's curated arXiv ID, so one
    // rule covers imported and INSPIRE snapshots alike and no per-source match is
    // needed here.
    let Some(arxiv) = reference.identifiers.arxiv.first() else {
        return Ok(entry);
    };
    let url = arxiv_pdf_url(arxiv)
        .with_context(|| format!("could not build an arXiv URL for `{key}`"))?;
    Ok(insert_field(&entry, URL_FIELD, url.as_str())?)
}

/// Refuse a destination inside the global store, including through a symlink.
fn ensure_outside_store(path: &Path, store: &Path) -> Result<()> {
    let target = resolved(path)?;
    let store = fs::canonicalize(store)
        .with_context(|| format!("could not resolve cita home {}", store.display()))?;
    // `starts_with` is component-wise and already true for equal paths, so a
    // sibling like `~/.cita-backup.bib` is correctly left alone.
    if target.starts_with(&store) {
        bail!(
            "refusing to write an export inside the global cita store {}",
            store.display()
        );
    }
    Ok(())
}

/// An absolute, canonical path for comparing against the store.
///
/// An existing target is canonicalized outright, so a case-insensitive filesystem
/// reports the on-disk name and `References.bib` cannot alias `references.bib`. A
/// target that does not exist yet cannot alias an existing file, so only its parent
/// is canonicalized, which still collapses `..` segments and symlinked directories.
/// Only `NotFound` justifies that fallback: every other error means the path could
/// not be resolved at all, and treating it as "absent" would compare a path whose
/// final component was never resolved.
fn resolved(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("could not resolve {}", path.display()))?;
    match fs::canonicalize(&absolute) {
        Ok(canonical) => return Ok(canonical),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow::Error::from(error))
                .with_context(|| format!("could not resolve {}", absolute.display()));
        }
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
        assert!(ensure_outside_store(store.path(), store.path()).is_err());
    }

    #[test]
    fn a_sibling_sharing_the_store_prefix_is_allowed() {
        // Guards the component-wise comparison: a textual prefix check would
        // wrongly reject `<store>-backup.bib`.
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("cita");
        fs::create_dir(&store).unwrap();
        let sibling = root.path().join("cita-backup.bib");
        ensure_outside_store(&sibling, &store).unwrap();
    }
}
