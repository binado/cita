use super::open;
use anyhow::{Context, Result, bail};
use bibi_bibfile::{Bibfile, FIELD_PREFIX, atomic_write};
use bibi_bibliography::{insert_field, strip_fields_with_prefix};
use bibi_documents::arxiv_pdf_url;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const URL_FIELD: &str = "url";

/// Write a bibliography for somebody else to read.
///
/// This inverts what export used to do. The working file is now the source of
/// truth and carries bibi's own fields, which LaTeX ignores but a human or a
/// reference manager should not have to see — so the default is to take them
/// out, not to add anything. The derived `url` is still added for tools like
/// Zotero that want a resolvable link.
pub(crate) fn export(
    path: &Path,
    output: Option<&Path>,
    caller: &Path,
    keep_metadata: bool,
) -> Result<()> {
    let file = open(path)?;
    let rendered = render(&file, keep_metadata)?;
    let Some(output) = output else {
        return write_to(&default_path(&file)?, &rendered);
    };
    if output == Path::new("-") {
        io::stdout()
            .write_all(rendered.as_bytes())
            .context("could not write the export to stdout")?;
        return Ok(());
    }
    let target = if output.is_absolute() {
        output.to_path_buf()
    } else {
        caller.join(output)
    };
    // The only file an export must never become is the source of truth itself.
    if canonical(&target) == canonical(file.path()) {
        bail!(
            "refusing to write the export over {}, which is the bibliography itself",
            file.path().display()
        );
    }
    write_to(&target, &rendered)
}

fn write_to(target: &Path, rendered: &str) -> Result<()> {
    atomic_write(target, rendered.as_bytes())?;
    println!("Exported {}", target.display());
    Ok(())
}

/// Render each entry with export policy applied, joined deterministically.
///
/// Unlike a write back to the bibliography, layout here is normalized: the
/// export is a derived artifact nobody edits, so there are no authored bytes to
/// preserve.
fn render(file: &Bibfile, keep_metadata: bool) -> Result<String> {
    if file.entries().is_empty() {
        return Ok(String::new());
    }
    let mut entries = Vec::with_capacity(file.entries().len());
    for entry in file.entries() {
        let mut text = if keep_metadata {
            entry.bibtex.clone()
        } else {
            strip_fields_with_prefix(&entry.bibtex, FIELD_PREFIX)?
        };
        // Project from the stored entry, not the stripped copy, so a curated
        // arXiv id still decides the link after its field has been removed.
        let reference = entry.project()?;
        if let Some(arxiv) = reference.identifiers.arxiv.first() {
            let url = arxiv_pdf_url(arxiv)
                .with_context(|| format!("could not build an arXiv URL for `{}`", entry.key))?;
            text = insert_field(&text, URL_FIELD, url.as_str())?;
        }
        entries.push(text.trim().to_owned());
    }
    Ok(format!("{}\n", entries.join("\n\n")))
}

/// `<bibliography-directory-name>.bib`, beside the bibliography.
fn default_path(file: &Bibfile) -> Result<PathBuf> {
    let directory = file.path().parent().unwrap_or(Path::new("."));
    let absolute = std::path::absolute(directory)
        .with_context(|| format!("could not resolve {}", directory.display()))?;
    let name = absolute.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "could not derive an export file name from {}; pass --output",
            directory.display()
        )
    })?;
    // `OsString` rather than `format!` so a non-UTF-8 directory name works.
    let mut export = name.to_os_string();
    export.push(".bib");
    Ok(directory.join(export))
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}
