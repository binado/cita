use super::{Target, add_message, highlight_style, inspire_client};
use anyhow::{Context, Result};
use cita_core::{Locator, MetadataProvider, Reference, ReferenceSource};
use cita_documents::{
    ArtifactKind, DocumentStore, Error as DocumentError, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
use cita_manifest::{AddOutcome, ConflictPolicy, KeyRequest, PendingReference, SourceSnapshot};
use std::{
    fmt,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
};

#[derive(Clone)]
struct Selected {
    key: String,
    reference: Reference,
    files_root: PathBuf,
    save_outcome: Option<AddOutcome>,
}

async fn select(target: &Target, selector: &str, save: bool) -> Result<Selected> {
    let _lock = save.then(|| target.lock()).transpose()?;
    let mut manifest = target.load()?;
    if let Some(item) = manifest.find(selector)? {
        let save_outcome = save.then(|| AddOutcome::Existing(item.key.clone()));
        return Ok(Selected {
            key: item.key,
            reference: item.reference,
            files_root: target.files_root(),
            save_outcome,
        });
    }
    let locator = selector.parse::<Locator>().map_err(|error| {
        anyhow::Error::from(error).context(format!("reference `{selector}` was not found"))
    })?;
    let client = inspire_client()?;
    if save {
        let record = client.resolve(&locator).await?;
        let key = record.texkey.clone();
        let reference = record.project()?;
        let outcome = manifest
            .add_batch(
                vec![PendingReference {
                    key: KeyRequest::Suggested(key),
                    source: SourceSnapshot::inspire(record),
                }],
                ConflictPolicy::Skip,
            )?
            .pop()
            .expect("one outcome");
        let key = match &outcome {
            AddOutcome::Added(key)
            | AddOutcome::Existing(key)
            | AddOutcome::Overwritten { key, .. } => key.clone(),
            AddOutcome::Skipped { conflicting, .. } => conflicting.clone(),
        };
        Ok(Selected {
            key,
            reference,
            files_root: target.files_root(),
            save_outcome: Some(outcome),
        })
    } else {
        let reference = client.resolve_reference(&locator).await?;
        Ok(Selected {
            key: selector.into(),
            reference,
            files_root: target.files_root(),
            save_outcome: None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FetchOptions {
    pub(crate) force: bool,
    pub(crate) cache_only: bool,
    pub(crate) return_url: bool,
    pub(crate) source: bool,
    pub(crate) open: bool,
    pub(crate) save: bool,
}

pub(crate) async fn fetch(target: &Target, selector: &str, options: FetchOptions) -> Result<()> {
    let policy = if options.force {
        FetchPolicy::Force
    } else if options.cache_only {
        FetchPolicy::CacheOnly
    } else {
        FetchPolicy::UseCache
    };
    let kind = if options.source {
        ArtifactKind::Source
    } else {
        ArtifactKind::Pdf
    };
    let selected = select(target, selector, options.save).await?;
    if let Some(outcome) = &selected.save_outcome {
        eprintln!(
            "{}",
            add_message(outcome, highlight_style(io::stderr().is_terminal()))
        );
    }
    let arxiv = selected
        .reference
        .identifiers
        .arxiv
        .first()
        .ok_or_else(|| anyhow::anyhow!("reference `{}` has no arXiv eprint", selected.key))?;
    let url = arxiv_pdf_url(arxiv)?.to_string();
    let target = if options.return_url {
        FetchTarget::Url(url)
    } else {
        let outcome = fetch_selected(&selected.files_root, arxiv, kind, policy).await?;
        eprintln!("{}", fetch_message(&selected.key, &url, kind, &outcome));
        FetchTarget::Path(outcome.path().to_owned())
    };
    if options.open {
        open_target(&target)?;
        eprintln!("Opened {target}");
    }
    println!("{target}");
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FetchTarget {
    Url(String),
    Path(PathBuf),
}

impl fmt::Display for FetchTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => formatter.write_str(url),
            Self::Path(path) => write!(formatter, "{}", path.display()),
        }
    }
}

fn open_target(target: &FetchTarget) -> Result<()> {
    match target {
        FetchTarget::Url(url) => opener::open(url).with_context(|| format!("could not open {url}")),
        FetchTarget::Path(path) => {
            opener::open(path).with_context(|| format!("could not open {}", path.display()))
        }
    }
}

async fn fetch_selected(
    files_root: &Path,
    arxiv: &str,
    kind: ArtifactKind,
    policy: FetchPolicy,
) -> Result<FetchOutcome> {
    DocumentStore::new(files_root)?
        .fetch_artifact(arxiv, kind, policy)
        .await
        .map_err(document_error_with_hint)
}

fn document_error_with_hint(error: DocumentError) -> anyhow::Error {
    match error {
        error @ (DocumentError::InvalidCachedPdf(_) | DocumentError::InvalidCachedSource(_)) => {
            anyhow::Error::from(error).context("retry with --force")
        }
        error @ (DocumentError::NotCached(_) | DocumentError::SourceNotCached(_)) => {
            anyhow::Error::from(error).context("rerun without --cache-only")
        }
        error => error.into(),
    }
}
fn fetch_message(key: &str, url: &str, kind: ArtifactKind, outcome: &FetchOutcome) -> String {
    match (kind, outcome) {
        (ArtifactKind::Pdf, FetchOutcome::Downloaded(_)) => format!("Fetched {key}: {url}"),
        (ArtifactKind::Pdf, FetchOutcome::Cached(_)) => {
            format!("Already fetched {key}: {url}")
        }
        (ArtifactKind::Source, FetchOutcome::Downloaded(_)) => {
            format!("Fetched source for {key}")
        }
        (ArtifactKind::Source, FetchOutcome::Cached(_)) => {
            format!("Already fetched source for {key}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fetch_targets_render_the_value_passed_to_the_opener() {
        assert_eq!(
            FetchTarget::Url("https://arxiv.org/pdf/1207.7214".into()).to_string(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            FetchTarget::Path(PathBuf::from("/tmp/1207.7214.pdf")).to_string(),
            "/tmp/1207.7214.pdf"
        );
    }

    #[test]
    fn fetch_messages_distinguish_pdfs_and_source_packages() {
        let downloaded = FetchOutcome::Downloaded(PathBuf::from("/tmp/source"));
        let cached = FetchOutcome::Cached(PathBuf::from("/tmp/source"));
        assert_eq!(
            fetch_message(
                "Paper",
                "https://arxiv.org/pdf/1207.7214",
                ArtifactKind::Source,
                &downloaded
            ),
            "Fetched source for Paper"
        );
        assert_eq!(
            fetch_message(
                "Paper",
                "https://arxiv.org/pdf/1207.7214",
                ArtifactKind::Source,
                &cached
            ),
            "Already fetched source for Paper"
        );
    }
}
