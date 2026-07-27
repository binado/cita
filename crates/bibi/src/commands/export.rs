use super::find_manifest;
use anyhow::{Context, Result, anyhow, bail};
use bibi_bibliography::insert_field;
use bibi_core::ReferenceSource;
use bibi_documents::arxiv_pdf_url;
use bibi_manifest::{
    BIBLIOGRAPHY_FILE, LIBRARY_FILE, MANIFEST_FILE, Manifest, SourceSnapshot, atomic_write,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const URL_FIELD: &str = "url";

/// Write a derived bibliography and report where it went.
pub(crate) fn export(project: &Path, caller: &Path, output: Option<&Path>) -> Result<()> {
    let path = export_outcome(project, caller, output)?;
    println!("Exported {}", path.display());
    Ok(())
}

/// Write a derived bibliography for the project containing `project`.
///
/// A relative `output` resolves against `caller`, matching how import paths use
/// the caller's directory rather than the resolved project.
pub(crate) fn export_outcome(
    project: &Path,
    caller: &Path,
    output: Option<&Path>,
) -> Result<PathBuf> {
    // The export claims to hold the same entries as references.bib, so a
    // drifted bibliography must fail rather than silently disagree with it.
    let manifest = Manifest::load_verified(find_manifest(project)?)?;
    let directory = manifest
        .path()
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let path = match output {
        Some(output) if output.is_absolute() => output.to_path_buf(),
        Some(output) => caller.join(output),
        None => default_export_path(&directory)?,
    };
    ensure_not_managed(&path, &manifest)?;
    let rendered = manifest.render_derived(derive_entry)?;
    atomic_write(&path, rendered.as_bytes())?;
    Ok(path)
}

/// Add the derived arXiv PDF URL to one re-keyed entry that has an arXiv ID.
fn derive_entry(key: &str, source: &SourceSnapshot, entry: String) -> Result<String> {
    let reference = source
        .project()
        .with_context(|| format!("could not project `{key}`"))?;
    // The projected identity already carries INSPIRE's curated arXiv ID, so one
    // rule covers imported and INSPIRE snapshots alike.
    let Some(arxiv) = reference.identifiers.arxiv.first() else {
        return Ok(entry);
    };
    let url = arxiv_pdf_url(arxiv)
        .with_context(|| format!("could not build an arXiv URL for `{key}`"))?;
    Ok(insert_field(&entry, URL_FIELD, url.as_str())?)
}

fn default_export_path(directory: &Path) -> Result<PathBuf> {
    let name = directory.file_name().ok_or_else(|| {
        anyhow!(
            "could not derive an export file name from {}; pass --output",
            directory.display()
        )
    })?;
    // `OsString` rather than `format!` so a non-UTF-8 directory name works.
    let mut file = name.to_os_string();
    file.push(".bib");
    Ok(directory.join(file))
}

/// Refuse to write over a managed file.
///
/// The identity check covers this project's own files. `--output` is the only
/// path in the CLI that can leave the discovered project, so it also has to
/// answer for every other project's files, which the ownership check below does.
fn ensure_not_managed(path: &Path, manifest: &Manifest) -> Result<()> {
    let target = resolved(path)?;
    for managed in [manifest.bibliography_path(), manifest.path()] {
        if target == resolved(managed)? {
            bail!(
                "refusing to write the export over the managed file {}",
                managed.display()
            );
        }
    }
    ensure_not_owned_elsewhere(&target)
}

/// Refuse a target that some other project or library manages.
///
/// A managed name is only managed inside the directory that owns it, so a
/// `references.bib` in a plain LaTeX directory remains a legal export target;
/// the same name beside a `cita.toml` does not.
fn ensure_not_owned_elsewhere(target: &Path) -> Result<()> {
    let (Some(name), Some(parent)) = (target.file_name(), target.parent()) else {
        return Ok(());
    };
    let (owner, kind) = match name.to_str() {
        Some(MANIFEST_FILE | BIBLIOGRAPHY_FILE) => (MANIFEST_FILE, "project"),
        Some(LIBRARY_FILE) => (LIBRARY_FILE, "library"),
        _ => return Ok(()),
    };
    if parent.join(owner).is_file() {
        bail!(
            "refusing to write the export over the managed file {}, which belongs to the {kind} at {}",
            target.display(),
            parent.display()
        );
    }
    Ok(())
}

/// An absolute, canonical path for comparing against managed files.
///
/// An existing target is canonicalized outright, so a case-insensitive
/// filesystem reports the on-disk name and `References.bib` cannot alias
/// `references.bib`. A target that does not exist yet cannot alias an existing
/// managed file, so only its parent is canonicalized, which still collapses
/// `..` segments and symlinked directories.
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
