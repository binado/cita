//! Acquiring records' documents.

use crate::{Progress, error::Error, services::Services};
use bibi_core::{ArxivId, Locator};
use bibi_documents::{ArtifactKind, default_filename};
use bibi_manifest::ManifestStore;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Options for `fetch`.
#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
    /// The records to fetch for, in input order.
    pub selectors: Vec<String>,
    /// Fetch source archives instead of PDFs.
    pub source: bool,
    /// Report public URLs instead of downloading anything.
    pub url: bool,
    /// Download to this exact file for one selector, or into this existing
    /// directory for one or more selectors.
    pub output: Option<PathBuf>,
    /// Replace occupied destinations instead of failing those items.
    pub force: bool,
    /// The directory default names and relative output paths resolve against.
    pub working_directory: PathBuf,
}

/// What one successful fetch produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchTarget {
    /// An artifact downloaded now.
    Downloaded(PathBuf),
    /// A public URL, with nothing downloaded.
    Url(String),
}

impl FetchTarget {
    /// The single line the command writes for this success.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Downloaded(path) => path.to_string_lossy(),
            Self::Url(url) => std::borrow::Cow::Borrowed(url),
        }
    }
}

/// One selector's ordered fetch outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchOutcome {
    /// The selector produced a unique file or URL.
    Success {
        /// The selector as supplied.
        selector: String,
        /// What it produced.
        target: FetchTarget,
    },
    /// An earlier selector already named the same artifact.
    Skipped {
        /// The selector as supplied.
        selector: String,
        /// The first selector that named the artifact.
        first: String,
    },
    /// This selector could not be completed.
    Failure {
        /// The selector as supplied.
        selector: String,
        /// The human-readable reason.
        message: String,
    },
}

impl FetchOutcome {
    /// The selector this outcome belongs to.
    pub fn selector(&self) -> &str {
        match self {
            Self::Success { selector, .. }
            | Self::Skipped { selector, .. }
            | Self::Failure { selector, .. } => selector,
        }
    }
}

/// Ordered outcomes for a fetch batch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FetchReport {
    /// One outcome per input selector, in input order.
    pub outcomes: Vec<FetchOutcome>,
}

impl FetchReport {
    /// True when at least one selector failed.
    pub fn has_failures(&self) -> bool {
        self.outcomes
            .iter()
            .any(|outcome| matches!(outcome, FetchOutcome::Failure { .. }))
    }
}

enum OutputMode {
    Default(PathBuf),
    Directory(PathBuf),
    Exact(PathBuf),
}

enum Planned {
    Ready {
        selector: String,
        arxiv: ArxivId,
        destination: Option<PathBuf>,
    },
    Skipped {
        selector: String,
        first: String,
    },
    Failure {
        selector: String,
        message: String,
    },
}

impl Planned {
    fn selector(&self) -> &str {
        match self {
            Self::Ready { selector, .. }
            | Self::Skipped { selector, .. }
            | Self::Failure { selector, .. } => selector,
        }
    }
}

/// Resolve and acquire a batch of arXiv artifacts.
///
/// The manifest is loaded once and every selector, artifact, destination,
/// duplicate, and existing-file collision is planned before a document client
/// is requested. Downloads then run sequentially. Each file remains an atomic
/// operation in `bibi-documents`; the batch is intentionally partial, so one
/// failed item does not roll successful files back.
pub async fn fetch<F>(
    services: &Services,
    store: &ManifestStore,
    request: &FetchRequest,
    mut progress_for: F,
) -> Result<FetchReport, Error>
where
    F: FnMut(&str) -> Progress,
{
    if request.selectors.is_empty() {
        return Ok(FetchReport::default());
    }
    if request.url && request.output.is_some() {
        return Err(Error::usage("`--url` cannot be combined with `--output`"));
    }

    let output_mode = output_mode(request)?;
    let manifest = store.load()?.manifest;
    let kind = if request.source {
        ArtifactKind::Source
    } else {
        ArtifactKind::Pdf
    };
    let mut seen_artifacts = HashMap::<String, String>::new();
    let mut seen_destinations = HashMap::<PathBuf, String>::new();
    let mut planned = Vec::with_capacity(request.selectors.len());

    for selector in &request.selectors {
        let record = match Locator::parse(selector)
            .map_err(Error::from)
            .and_then(|parsed| Ok(manifest.resolve(&parsed)?))
        {
            Ok(record) => record,
            Err(error) => {
                planned.push(Planned::Failure {
                    selector: selector.clone(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        let Some(arxiv) = record.identifiers.arxiv.clone() else {
            planned.push(Planned::Failure {
                selector: selector.clone(),
                message: format!(
                    "`{}` has no arXiv identifier, and arXiv is the only document source",
                    record.key
                ),
            });
            continue;
        };
        let artifact = format!("{}:{}", kind.label(), arxiv.as_str());
        if let Some(first) = seen_artifacts.get(&artifact) {
            planned.push(Planned::Skipped {
                selector: selector.clone(),
                first: first.clone(),
            });
            continue;
        }
        seen_artifacts.insert(artifact.clone(), selector.clone());

        let destination = if request.url {
            None
        } else {
            Some(match &output_mode {
                OutputMode::Default(directory) | OutputMode::Directory(directory) => {
                    directory.join(default_filename(&arxiv, kind))
                }
                OutputMode::Exact(path) => path.clone(),
            })
        };
        if let Some(path) = &destination {
            if let Some(other_artifact) = seen_destinations.get(path) {
                planned.push(Planned::Failure {
                    selector: selector.clone(),
                    message: format!(
                        "destination `{}` is also planned for a different artifact ({other_artifact})",
                        path.display()
                    ),
                });
                continue;
            }
            seen_destinations.insert(path.clone(), artifact);
            if !request.force && path.exists() {
                planned.push(Planned::Failure {
                    selector: selector.clone(),
                    message: format!("destination `{}` already exists", path.display()),
                });
                continue;
            }
        }
        planned.push(Planned::Ready {
            selector: selector.clone(),
            arxiv,
            destination,
        });
    }

    // The set is only used to make the preflight invariant explicit: every
    // ready destination is unique before the first await that can touch HTTP.
    debug_assert_eq!(
        planned
            .iter()
            .filter_map(|item| match item {
                Planned::Ready {
                    destination: Some(path),
                    ..
                } => Some(path),
                _ => None,
            })
            .collect::<HashSet<_>>()
            .len(),
        planned
            .iter()
            .filter(|item| matches!(
                item,
                Planned::Ready {
                    destination: Some(_),
                    ..
                }
            ))
            .count()
    );

    let mut report = FetchReport {
        outcomes: Vec::with_capacity(planned.len()),
    };
    for item in planned {
        let mut progress = progress_for(item.selector());
        let outcome = match item {
            Planned::Skipped { selector, first } => FetchOutcome::Skipped { selector, first },
            Planned::Failure { selector, message } => FetchOutcome::Failure { selector, message },
            Planned::Ready {
                selector,
                arxiv,
                destination: None,
            } => match bibi_documents::public_url(&arxiv, kind) {
                Ok(url) => FetchOutcome::Success {
                    selector,
                    target: FetchTarget::Url(url.to_string()),
                },
                Err(error) => FetchOutcome::Failure {
                    selector,
                    message: error.to_string(),
                },
            },
            Planned::Ready {
                selector,
                arxiv,
                destination: Some(destination),
            } => {
                let result = match services.documents() {
                    Ok(documents) => documents
                        .download_with_progress(
                            &arxiv,
                            kind,
                            &destination,
                            request.force,
                            |bytes, total| progress.on_chunk(bytes, total),
                        )
                        .await
                        .map_err(Error::from),
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => FetchOutcome::Success {
                        selector,
                        target: FetchTarget::Downloaded(destination),
                    },
                    Err(error) => FetchOutcome::Failure {
                        selector,
                        message: error.to_string(),
                    },
                }
            }
        };
        progress.finish();
        report.outcomes.push(outcome);
    }
    Ok(report)
}

fn output_mode(request: &FetchRequest) -> Result<OutputMode, Error> {
    let Some(output) = &request.output else {
        return Ok(OutputMode::Default(request.working_directory.clone()));
    };
    let path = absolute(&request.working_directory, output);
    if path.is_dir() {
        Ok(OutputMode::Directory(path))
    } else if request.selectors.len() == 1 {
        Ok(OutputMode::Exact(path))
    } else {
        Err(Error::usage(format!(
            "`--output {}` must be an existing directory when fetching multiple selectors",
            output.display()
        )))
    }
}

fn absolute(working_directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    }
}
