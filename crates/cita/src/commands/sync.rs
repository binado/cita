use super::{Target, inspire_client};
use anyhow::{Context, Result};
use cita_core::{Locator, ReferenceSource};
use cita_inspire_client::Error as InspireError;
use cita_store::{Library, SourceSnapshot, SyncCandidate, SyncUpdate};
use std::collections::BTreeMap;

pub(crate) async fn sync(target: &Target<'_>) -> Result<()> {
    let outcome = sync_candidates(target.library(), target.sync_candidates()?).await?;
    println!("{outcome}");
    Ok(())
}

pub(crate) async fn sync_all(library: &Library) -> Result<()> {
    let candidates = library.sync_candidates(None)?;
    let outcome = sync_candidates(library, candidates).await?;
    println!("{outcome}");
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SyncOutcome {
    changed: usize,
    managed: usize,
    imported: usize,
    promoted: usize,
}

impl std::fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.changed == 0 {
            write!(
                formatter,
                "Already in sync: {} managed, {} imported",
                self.managed, self.imported
            )
        } else {
            write!(
                formatter,
                "Synced {} unique references ({} managed, {} imported, {} promoted)",
                self.changed, self.managed, self.imported, self.promoted
            )
        }
    }
}

async fn sync_candidates(library: &Library, candidates: Vec<SyncCandidate>) -> Result<SyncOutcome> {
    let client = inspire_client()?;
    let managed = candidates
        .iter()
        .filter(|candidate| candidate.source.inspire_entry().is_some())
        .count();
    let imported = candidates.len() - managed;
    let record_ids = candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .source
                .inspire_entry()
                .map(|entry| entry.record_id)
        })
        .collect::<Vec<_>>();
    let refreshed = client
        .refresh_records(&record_ids)
        .await?
        .into_iter()
        .map(|record| (record.record_id, record))
        .collect::<BTreeMap<_, _>>();
    let mut updates = Vec::new();
    let mut promoted = 0usize;
    for candidate in candidates {
        let replacement = if let Some(entry) = candidate.source.inspire_entry() {
            let record = refreshed.get(&entry.record_id).ok_or_else(|| {
                anyhow::anyhow!("INSPIRE did not return managed record {}", entry.record_id)
            })?;
            Some(SourceSnapshot::inspire(record.clone()))
        } else {
            canonicalize_import(&client, &candidate).await?
        };
        if let Some(replacement) = replacement
            && replacement != candidate.source
        {
            if candidate.source.inspire_entry().is_none() && replacement.inspire_entry().is_some() {
                promoted += 1;
            }
            updates.push(SyncUpdate {
                id: candidate.id,
                expected: candidate.source,
                replacement,
            });
        }
    }
    let changed = library.apply_sync(updates)?;
    Ok(SyncOutcome {
        changed,
        managed,
        imported,
        promoted,
    })
}

async fn canonicalize_import(
    client: &cita_inspire_client::Client,
    candidate: &SyncCandidate,
) -> Result<Option<SourceSnapshot>> {
    let reference = candidate
        .source
        .project()
        .context("could not project imported reference during sync")?;
    let locators = reference
        .identifiers
        .arxiv
        .into_iter()
        .map(Locator::Arxiv)
        .chain(reference.identifiers.dois.into_iter().map(Locator::Doi));
    for locator in locators {
        match client.resolve_snapshot(&locator).await {
            Ok(record) => return Ok(Some(SourceSnapshot::inspire(record))),
            Err(InspireError::NotFound(_)) => continue,
            Err(error @ (InspireError::Transport(_) | InspireError::HttpStatus { .. })) => {
                eprintln!(
                    "Could not canonicalize imported reference through INSPIRE ({error}); left unchanged"
                );
                return Ok(None);
            }
            Err(error @ (InspireError::Malformed(_) | InspireError::InvalidBaseUrl(_))) => {
                return Err(anyhow::Error::from(error)
                    .context("could not canonicalize imported reference during sync"));
            }
        }
    }
    Ok(None)
}
