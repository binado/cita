//! Remote add and exact local import.

use crate::{Error, Services};
use bibi_bibtex::parse_file;
use bibi_core::{Admission, Bibliography, CollisionPolicy, Locator, ProviderName, RecordState};
use bibi_manifest::BibliographyStore;

/// Remote add options.
#[derive(Clone, Debug, Default)]
pub struct AddRequest {
    /// Explicit provider, otherwise inferred/defaulted.
    pub provider: Option<ProviderName>,
    /// Converge stable identity collisions.
    pub overwrite: bool,
    /// Fully plan without publishing.
    pub dry_run: bool,
}

/// Local import options.
#[derive(Clone, Debug, Default)]
pub struct ImportRequest {
    /// Converge stable identity collisions.
    pub overwrite: bool,
    /// Fully plan without publishing.
    pub dry_run: bool,
}

/// Successful ordered mutation result.
#[derive(Debug)]
pub struct MutationReport<T> {
    /// Results in input order.
    pub results: Vec<T>,
    /// Whether the complete successor was published.
    pub committed: bool,
}

/// Resolve and admit all locators or change nothing.
pub async fn add(
    services: &Services,
    store: &BibliographyStore,
    raw_locators: &[String],
    request: &AddRequest,
) -> Result<MutationReport<Admission>, Error> {
    let locators = raw_locators
        .iter()
        .map(|raw| Locator::parse(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let states = services
        .providers
        .resolve(request.provider, &locators)
        .await?;
    admit(store, states, request.overwrite, request.dry_run)
}

/// Parse and admit one complete local BibTeX stream or change nothing.
pub fn import(
    store: &BibliographyStore,
    source: &str,
    request: &ImportRequest,
) -> Result<MutationReport<Admission>, Error> {
    let states = parse_file(source)
        .map_err(Error::from)?
        .into_iter()
        .map(RecordState::local)
        .collect::<Result<Vec<_>, _>>()?;
    admit(store, states, request.overwrite, request.dry_run)
}

fn admit(
    store: &BibliographyStore,
    states: Vec<RecordState>,
    overwrite: bool,
    dry_run: bool,
) -> Result<MutationReport<Admission>, Error> {
    let (bibliography, generation) = store.load_or_empty()?.into_parts();
    let policy = if overwrite {
        CollisionPolicy::Overwrite
    } else {
        CollisionPolicy::Reject
    };
    let (successor, results) = bibliography.add(states, policy)?;
    publish(store, &generation, &successor, dry_run)?;
    Ok(MutationReport {
        committed: !dry_run,
        results,
    })
}

fn publish(
    store: &BibliographyStore,
    generation: &bibi_manifest::Generation,
    bibliography: &Bibliography,
    dry_run: bool,
) -> Result<(), Error> {
    if !dry_run {
        store.commit(generation, bibliography)?;
    }
    Ok(())
}
