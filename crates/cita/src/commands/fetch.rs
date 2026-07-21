use super::{add_message, ensure_cache_layout, find_manifest, inspire_client};
use anyhow::{Context, Result};
use cita_core::{Locator, MetadataProvider, Reference, ReferenceSource};
use cita_documents::{
    DocumentStore, Error as DocumentError, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
use cita_manifest::{AddOutcome, KeyRequest, Manifest, PendingReference, SourceSnapshot};
use std::{
    fmt,
    path::{Path, PathBuf},
};

#[derive(Clone)]
struct Selected {
    key: String,
    reference: Reference,
    manifest_path: PathBuf,
    save_outcome: Option<AddOutcome>,
}

async fn select(cwd: &Path, selector: &str, save: bool) -> Result<Selected> {
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load_verified(&path)?;
    if let Some(item) = manifest.find(selector)? {
        let save_outcome = save.then(|| AddOutcome::Existing(item.key.clone()));
        return Ok(Selected {
            key: item.key,
            reference: item.reference,
            manifest_path: path,
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
            .add_batch(vec![PendingReference {
                key: KeyRequest::Suggested(key),
                source: SourceSnapshot::inspire(record),
            }])
            .map_err(|error| match error {
                error @ cita_manifest::Error::KeyConflict { .. } => anyhow::Error::from(error)
                    .context(
                        "save with a different local key using `cita add --key <key> <locator>`",
                    ),
                error => error.into(),
            })?
            .pop()
            .expect("one outcome");
        let key = match &outcome {
            AddOutcome::Added(key) | AddOutcome::Existing(key) => key.clone(),
        };
        Ok(Selected {
            key,
            reference,
            manifest_path: path,
            save_outcome: Some(outcome),
        })
    } else {
        let reference = client.resolve_reference(&locator).await?;
        Ok(Selected {
            key: selector.into(),
            reference,
            manifest_path: path,
            save_outcome: None,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch(
    cwd: &Path,
    selector: &str,
    force: bool,
    cache_only: bool,
    return_url: bool,
    open: bool,
    save: bool,
) -> Result<()> {
    let policy = if force {
        FetchPolicy::Force
    } else if cache_only {
        FetchPolicy::CacheOnly
    } else {
        FetchPolicy::UseCache
    };
    let selected = select(cwd, selector, save).await?;
    if let Some(outcome) = &selected.save_outcome {
        eprintln!("{}", add_message(outcome));
    }
    let arxiv = selected
        .reference
        .identifiers
        .arxiv
        .first()
        .ok_or_else(|| anyhow::anyhow!("reference `{}` has no arXiv eprint", selected.key))?;
    let url = arxiv_pdf_url(arxiv)?.to_string();
    let target = if return_url {
        FetchTarget::Url(url)
    } else {
        let outcome = fetch_selected(&selected.manifest_path, arxiv, policy).await?;
        eprintln!("{}", fetch_message(&selected.key, &url, &outcome));
        FetchTarget::Path(outcome.path().to_owned())
    };
    if open {
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

async fn fetch_selected(path: &Path, arxiv: &str, policy: FetchPolicy) -> Result<FetchOutcome> {
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    if policy != FetchPolicy::CacheOnly {
        ensure_cache_layout(root)?;
    }
    DocumentStore::new(root.join(".cita/files"))?
        .fetch(arxiv, policy)
        .await
        .map_err(document_error_with_hint)
}

fn document_error_with_hint(error: DocumentError) -> anyhow::Error {
    match error {
        error @ DocumentError::InvalidCachedPdf(_) => {
            anyhow::Error::from(error).context("retry with --force")
        }
        error @ DocumentError::NotCached(_) => {
            anyhow::Error::from(error).context("rerun without --cache-only")
        }
        error => error.into(),
    }
}
fn fetch_message(key: &str, url: &str, outcome: &FetchOutcome) -> String {
    match outcome {
        FetchOutcome::Downloaded(_) => format!("Fetched {key}: {url}"),
        FetchOutcome::Cached(_) => format!("Already fetched {key}: {url}"),
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
}
