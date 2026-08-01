//! Offline record commands.

use crate::{Error, render_records};
use bibi_core::{Locator, RecordId};
use bibi_manifest::BibliographyStore;

/// One strict removal result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedRecord {
    /// Stable UUID removed.
    pub id: RecordId,
    /// Derived texkey removed.
    pub texkey: String,
}

/// Create an empty bibliography without overwriting.
pub fn init(store: &BibliographyStore) -> Result<(), Error> {
    store.create_empty()?;
    Ok(())
}

/// Render exactly one resident record.
pub fn show(store: &BibliographyStore, selector: &str) -> Result<String, Error> {
    let bibliography = store.load()?.bibliography;
    let record = bibliography.resolve(&Locator::parse(selector)?)?;
    Ok(render_records([record]))
}

/// Remove a complete locator batch or change nothing.
pub fn remove(
    store: &BibliographyStore,
    selectors: &[String],
    dry_run: bool,
) -> Result<crate::MutationReport<RemovedRecord>, Error> {
    let locators = selectors
        .iter()
        .map(|selector| Locator::parse(selector))
        .collect::<Result<Vec<_>, _>>()?;
    let (bibliography, generation) = store.load()?.into_parts();
    let (successor, removed) = bibliography.remove(&locators)?;
    let results = removed
        .into_iter()
        .map(|record| RemovedRecord {
            id: record.id(),
            texkey: record.texkey().to_owned(),
        })
        .collect();
    if !dry_run {
        store.commit(&generation, &successor)?;
    }
    Ok(crate::MutationReport {
        results,
        committed: !dry_run,
    })
}
