use super::{Target, report_shelf_failure, summarize_batch, target_in};
use crate::OutputFormat;
use anyhow::{Context, Result, bail};
use cita_bibliography::{insert_field, rename_entry};
use cita_documents::arxiv_pdf_url;
use cita_store::{Library, ShelfEntry};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const URL_FIELD: &str = "url";

pub(crate) fn export(
    target: &Target<'_>,
    caller: &Path,
    format: OutputFormat,
    output: Option<&Path>,
) -> Result<()> {
    let path = output_path(
        caller,
        output,
        &format!("{}.{}", target.name(), extension(format)),
    );
    ensure_outside_store(&path, target.store_root())?;
    let rendered = match format {
        OutputFormat::Bib => render_bibtex(target.entries()?)?,
        OutputFormat::Json => target.library().export_shelf(target.name())?.to_json()?,
        OutputFormat::Toml => target.library().export_shelf(target.name())?.to_toml()?,
    };
    atomic_write(&path, rendered.as_bytes())?;
    println!("Exported {}", path.display());
    Ok(())
}

pub(crate) fn batch_export(
    library: &Library,
    caller: &Path,
    format: OutputFormat,
    output: Option<&Path>,
) -> Result<bool> {
    match format {
        OutputFormat::Json | OutputFormat::Toml => {
            let path = output_path(caller, output, &format!("cita.{}", extension(format)));
            ensure_outside_store(&path, library.root())?;
            let value = library.export_library()?;
            let rendered = match format {
                OutputFormat::Json => value.to_json()?,
                OutputFormat::Toml => value.to_toml()?,
                OutputFormat::Bib => unreachable!(),
            };
            atomic_write(&path, rendered.as_bytes())?;
            println!("Exported {}", path.display());
            Ok(false)
        }
        OutputFormat::Bib => batch_bibtex(library, caller, output),
    }
}

fn batch_bibtex(library: &Library, caller: &Path, output: Option<&Path>) -> Result<bool> {
    let directory = output_path(caller, output, ".");
    if !directory.is_dir() {
        bail!(
            "batch export destination {} is not an existing directory",
            directory.display()
        );
    }
    let shelves = library.shelves()?;
    let mut failed = 0usize;
    for name in &shelves {
        let outcome = target_in(library, name)
            .map_err(Into::into)
            .and_then(|target| {
                let path = directory.join(format!("{name}.bib"));
                ensure_outside_store(&path, library.root())?;
                atomic_write(&path, render_bibtex(target.entries()?)?.as_bytes())?;
                Ok(path)
            });
        match outcome {
            Ok(path) => println!("Shelf {name}: exported {}", path.display()),
            Err(error) => {
                failed += 1;
                report_shelf_failure(name, &error);
            }
        }
    }
    Ok(summarize_batch(failed, shelves.len()))
}

fn render_bibtex(entries: Vec<ShelfEntry>) -> Result<String> {
    let mut rendered = Vec::with_capacity(entries.len());
    for entry in entries {
        let rekeyed = rename_entry(entry.source.raw_bibtex(), &entry.key)?;
        rendered.push(derive_entry(&entry, rekeyed)?);
    }
    if rendered.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{}\n", rendered.join("\n\n")))
}

fn derive_entry(entry: &ShelfEntry, raw: String) -> Result<String> {
    let Some(arxiv) = entry.reference.identifiers.arxiv.first() else {
        return Ok(raw);
    };
    let url = arxiv_pdf_url(arxiv)
        .with_context(|| format!("could not build an arXiv URL for `{}`", entry.key))?;
    Ok(insert_field(&raw, URL_FIELD, url.as_str())?)
}

fn extension(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Bib => "bib",
        OutputFormat::Json => "json",
        OutputFormat::Toml => "toml",
    }
}

fn output_path(caller: &Path, output: Option<&Path>, default: &str) -> PathBuf {
    match output {
        Some(path) if path.is_absolute() => path.into(),
        Some(path) => caller.join(path),
        None => caller.join(default),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("output path has no parent: {}", path.display()))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    temporary
        .write_all(bytes)
        .with_context(|| format!("could not write {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("could not sync {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

fn ensure_outside_store(path: &Path, store: &Path) -> Result<()> {
    let target = resolved(path)?;
    let store = fs::canonicalize(store)
        .with_context(|| format!("could not resolve cita home {}", store.display()))?;
    if target.starts_with(&store) {
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
    fn extensions_match_formats() {
        assert_eq!(extension(OutputFormat::Bib), "bib");
        assert_eq!(extension(OutputFormat::Json), "json");
        assert_eq!(extension(OutputFormat::Toml), "toml");
    }
}
