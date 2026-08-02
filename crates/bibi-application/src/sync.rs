//! Unconditional strict synchronization.

use crate::{Error, Services};
use bibi_core::{Locator, ProviderName, RecordId, Replacement, Source};
use bibi_manifest::BibliographyStore;
use std::collections::BTreeMap;

/// Sync options.
#[derive(Clone, Debug, Default)]
pub struct SyncRequest {
    /// Restrict to one provider.
    pub provider: Option<ProviderName>,
    /// Fully plan without publishing.
    pub dry_run: bool,
}

/// Successful sync summary.
#[derive(Debug)]
pub struct SyncReport {
    /// Ordered replacement results.
    pub results: Vec<Replacement>,
    /// Local records ignored.
    pub local: usize,
    /// Whether the successor was published.
    pub committed: bool,
}

/// Resolve complete state for every selected managed record or change nothing.
pub async fn sync(
    services: &Services,
    store: &BibliographyStore,
    request: &SyncRequest,
) -> Result<SyncReport, Error> {
    let (bibliography, generation) = store.load()?.into_parts();
    let mut groups: BTreeMap<ProviderName, Vec<(RecordId, bibi_core::ProviderId)>> =
        BTreeMap::new();
    let mut local = 0;
    for record in bibliography.records() {
        match record.state().source() {
            Source::Local => local += 1,
            Source::Managed { provider, id }
                if request
                    .provider
                    .is_none_or(|selected| selected == *provider) =>
            {
                groups
                    .entry(*provider)
                    .or_default()
                    .push((record.id(), id.clone()));
            }
            Source::Managed { .. } => {}
        }
    }

    let mut replacements = Vec::new();
    for (provider, records) in groups {
        let locators = records
            .iter()
            .map(|(_, id)| Locator::ProviderIdentity(provider, id.clone()))
            .collect::<Vec<_>>();
        let states = services
            .providers
            .resolve(Some(provider), &locators)
            .await?;
        for ((record_id, requested_id), state) in records.into_iter().zip(states) {
            if state.source().managed_identity() != Some((provider, &requested_id)) {
                return Err(bibi_provider::Error::Provider(
                    bibi_provider::ProviderError::contract(
                        provider,
                        format!(
                            "returned source {:?} for requested identity {provider}:{requested_id}",
                            state.source()
                        ),
                    ),
                )
                .into());
            }
            replacements.push((record_id, state));
        }
    }
    let (successor, results) = bibliography.replace(replacements)?;
    if !request.dry_run {
        store.commit(&generation, &successor)?;
    }
    Ok(SyncReport {
        results,
        local,
        committed: !request.dry_run,
    })
}
