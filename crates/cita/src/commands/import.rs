use super::{Target, inspire_client, print_add_outcomes};
use anyhow::{Context, Result};
use cita_bibliography::{BibtexSnapshot, split_entries};
use cita_core::{Locator, ReferenceSource};
use cita_inspire_client::Error as InspireError;
use cita_store::{ConflictPolicy, KeyRequest, PendingReference, SourceSnapshot};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

pub(crate) async fn import(
    target: &Target<'_>,
    caller: &Path,
    input: &str,
    overwrite: bool,
    skip_errors: bool,
) -> Result<()> {
    let mut source = String::new();
    if input == "-" {
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read BibTeX from stdin")?;
    } else {
        let path = caller.join(input);
        source = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
    }
    // A file-level parse error cannot be recovered because entry boundaries are
    // not trustworthy. Per-entry INSPIRE/projection errors below are isolatable.
    let parsed = split_entries(&source)?;
    let client = inspire_client()?;
    let mut pending = Vec::with_capacity(parsed.len());
    let mut skipped = 0usize;
    for (key, raw) in parsed {
        let snapshot = match BibtexSnapshot::new(raw) {
            Ok(snapshot) => snapshot,
            Err(error) if skip_errors => {
                skipped += 1;
                eprintln!("Skipped {key}: {error}");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let imported = SourceSnapshot::Import(snapshot);
        match canonicalize(&client, &key, &imported).await {
            Ok(source) => pending.push(PendingReference {
                key: KeyRequest::Exact(key),
                source,
            }),
            Err(error) if skip_errors => {
                skipped += 1;
                eprintln!("Skipped {key}: {error:#}");
            }
            Err(error) => return Err(error),
        }
    }
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let outcomes = if skip_errors {
        let (outcomes, storage_skips) =
            target
                .library()
                .add_batch_skipping_errors(target.name(), pending, policy)?;
        for item in storage_skips {
            skipped += 1;
            eprintln!("Skipped {}: {}", item.key, item.message);
        }
        outcomes
    } else {
        target.library().add_batch(target.name(), pending, policy)?
    };
    print_add_outcomes(&outcomes);
    if skipped > 0 {
        eprintln!("Skipped {skipped} invalid import entries");
    }
    Ok(())
}

async fn canonicalize(
    client: &cita_inspire_client::Client,
    key: &str,
    imported: &SourceSnapshot,
) -> Result<SourceSnapshot> {
    let reference = imported
        .project()
        .with_context(|| format!("could not project `{key}`"))?;
    let locators = reference
        .identifiers
        .arxiv
        .into_iter()
        .map(Locator::Arxiv)
        .chain(reference.identifiers.dois.into_iter().map(Locator::Doi));
    for locator in locators {
        match client.resolve_snapshot(&locator).await {
            Ok(record) => return Ok(SourceSnapshot::inspire(record)),
            Err(InspireError::NotFound(_)) => continue,
            Err(error @ (InspireError::Transport(_) | InspireError::HttpStatus { .. })) => {
                eprintln!(
                    "Could not canonicalize {key} through INSPIRE ({error}); kept imported BibTeX"
                );
                return Ok(imported.clone());
            }
            Err(error @ (InspireError::Malformed(_) | InspireError::InvalidBaseUrl(_))) => {
                return Err(
                    anyhow::Error::from(error).context(format!("could not canonicalize `{key}`"))
                );
            }
        }
    }
    Ok(imported.clone())
}
