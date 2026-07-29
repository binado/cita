//! Init, show, remove, and rename: the mutations that need no provider.

use crate::{
    error::Error,
    reports::{BatchReport, ItemFailure, SkipReason, SkippedItem},
};
use bibi_bibtex::CitationKey;
use bibi_core::{Record, Selector};
use bibi_manifest::ManifestStore;

/// Create an empty manifest at the selected target.
///
/// `init` exists for creating a project without adding anything; nothing
/// depends on it, because the commands that write records create a manifest on
/// demand. It refuses to overwrite any existing file, including one this build
/// cannot read: `init` must not be a way to discard a newer manifest.
pub fn init(store: &ManifestStore) -> Result<(), Error> {
    Ok(store.create_empty()?)
}

/// Emit one stored record as locally keyed BibTeX.
///
/// Offline by construction. A live provider response is a diagnostic for
/// provider-specific tooling, not an alternate meaning of `show`.
pub fn show(store: &ManifestStore, selector: &str) -> Result<String, Error> {
    let manifest = store.load()?.manifest;
    let record = manifest.resolve(&Selector::parse(selector)?)?;
    crate::render::render_records([record])
}

/// What a removal deleted.
#[derive(Clone, Debug)]
pub struct RemovedRecord {
    /// The key it was stored under.
    pub key: CitationKey,
    /// Its BibTeX, under that key, so the removal is recoverable input.
    pub bibtex: String,
}

/// What a removal did.
#[derive(Debug)]
pub struct RemoveReport {
    /// Per-selector outcomes, in input order.
    pub items: BatchReport<RemovedRecord>,
    /// Whether the manifest was written.
    pub committed: bool,
}

/// Delete records, emitting what was deleted.
///
/// Every selector resolves against the manifest as loaded, so removing one
/// record cannot change what a later selector in the same command means. The
/// document cache is untouched: it is derived, shared between projects, and
/// evicted explicitly.
pub fn remove(
    store: &ManifestStore,
    selectors: &[String],
    dry_run: bool,
) -> Result<RemoveReport, Error> {
    let (manifest, generation) = store.load()?.into_parts();
    let mut candidate = manifest.to_candidate();
    let mut items = BatchReport::new();
    let mut removed = Vec::new();

    for selector in selectors {
        let resolved = Selector::parse(selector)
            .map_err(Error::from)
            .and_then(|parsed| Ok(manifest.resolve(&parsed)?));
        match resolved {
            Err(error) => items
                .failures
                .push(ItemFailure::new(selector, error.to_string())),
            Ok(record) if removed.contains(&record.id) => {
                // Two selectors naming one record: the second asks for a state
                // that already holds, so it is reported rather than failed.
                items.skipped.push(SkippedItem {
                    item: selector.clone(),
                    reason: SkipReason::AlreadyRemoved {
                        existing: record.key.clone(),
                    },
                    bibtex: None,
                });
            }
            Ok(record) => {
                let deleted = candidate.remove(&record.id)?;
                removed.push(deleted.id);
                items.successes.push(RemovedRecord {
                    key: deleted.key.clone(),
                    bibtex: deleted.rendered()?,
                });
            }
        }
    }

    let committed = !dry_run && !items.successes.is_empty();
    if committed {
        store.commit(&generation, candidate)?;
    }
    Ok(RemoveReport { items, committed })
}

/// Change one record's local citation key.
///
/// Renaming requires no internal bookkeeping — no other bibi state refers to a
/// record by key — so the reason it is always explicit is the `\cite{}` calls
/// in the user's document, which only they can update.
pub fn rename(store: &ManifestStore, selector: &str, key: &CitationKey) -> Result<Record, Error> {
    let (manifest, generation) = store.load()?.into_parts();
    let id = manifest.resolve(&Selector::parse(selector)?)?.id;
    let mut candidate = manifest.to_candidate();
    candidate.rename(&id, key.clone())?;
    let renamed = candidate
        .get(&id)
        .expect("the renamed record is still in the candidate")
        .clone();
    store.commit(&generation, candidate)?;
    Ok(renamed)
}
