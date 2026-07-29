//! The provider for works no other provider holds.

use crate::{
    contract::{Provider, ProviderCapabilities, ProviderFuture},
    error::{MappingError, ProviderError},
    outcome::{PayloadItem, ProviderRecord, RefreshItem, RefreshRequest, RefreshState, Resolution},
};
use bibi_bibtex::BibtexEntry;
use bibi_core::{
    ArxivId, Description, Doi, Identifiers, Locator, Provenance, ProviderId, ProviderName,
};

/// The name the local provider is stored under.
pub const LOCAL_PROVIDER: &str = "local";

/// Records the user supplied, which nothing ever refreshes.
///
/// Software releases, datasets, references outside a provider's subject area,
/// and genuinely unpublished material are admitted this way. The design point
/// is that this is not a second *kind* of record: it is an ordinary provider
/// whose ownership happens to be that nothing changes, so refresh does not test
/// for it and skip — it asks every provider to refresh, and this one reports
/// that it refreshes nothing.
///
/// Deriving metadata from BibTeX is permitted here and nowhere else, because
/// here the BibTeX is the original rather than a lossy rendering of a
/// structured record.
#[derive(Debug)]
pub struct LocalProvider {
    name: ProviderName,
}

impl LocalProvider {
    /// Construct the local provider.
    pub fn new() -> Self {
        Self {
            name: ProviderName::new(LOCAL_PROVIDER).expect("the local provider name is valid"),
        }
    }
}

impl Default for LocalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for LocalProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::INGEST_ONLY
    }

    fn recognizes_unqualified_id(&self, _value: &str) -> bool {
        false
    }

    fn resolve<'a>(
        &'a self,
        locators: &'a [Locator],
    ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>> {
        Box::pin(async move { Ok(vec![Resolution::UnsupportedLocator; locators.len()]) })
    }

    fn refresh_metadata<'a>(
        &'a self,
        requests: &'a [RefreshRequest],
    ) -> ProviderFuture<'a, Vec<RefreshItem>> {
        Box::pin(async move {
            requests
                .iter()
                .map(|request| RefreshItem {
                    bibi_id: request.bibi_id,
                    result: Ok(RefreshState::Unrefreshable),
                })
                .collect()
        })
    }

    fn fetch_payloads<'a>(
        &'a self,
        _provider_ids: &'a [ProviderId],
    ) -> ProviderFuture<'a, Result<Vec<PayloadItem>, ProviderError>> {
        // Unreachable through the ordinary flow: a local record reports
        // `Unrefreshable`, so nothing ever advances to fetching its payload,
        // which it already holds.
        Box::pin(async move {
            Err(ProviderError::contract(
                &self.name,
                "local records carry their own payload and are never fetched",
            ))
        })
    }

    fn ingest(&self, entry: BibtexEntry) -> Result<Resolution, ProviderError> {
        let metadata = entry.local_metadata().map_err(|error| {
            ProviderError::Mapping(MappingError::InvalidPayload {
                provider: self.name.clone(),
                message: error.to_string(),
            })
        })?;
        // Identifiers the user wrote are still facts about the work, so they
        // are normalized and kept even though the description beside them is
        // only as good as what was pasted. An unparseable one is dropped rather
        // than failing the entry: it is advisory here, and the entry is being
        // kept precisely because no provider could supply a better one.
        let identifiers = Identifiers {
            doi: metadata.doi.as_deref().and_then(|doi| Doi::new(doi).ok()),
            arxiv: metadata
                .arxiv
                .as_deref()
                .and_then(|arxiv| ArxivId::new(arxiv).ok()),
        };
        Ok(Resolution::Found(Box::new(ProviderRecord {
            provenance: Provenance::unmanaged(self.name.clone()),
            identifiers,
            description: Description {
                title: metadata.title,
                authors: metadata.authors,
                collaborations: metadata.collaborations,
                year: metadata.year,
            },
            payload: entry,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    fn ingest(source: &str) -> Resolution {
        LocalProvider::new().ingest(entry(source)).unwrap()
    }

    #[test]
    fn ingest_projects_the_users_own_bibtex() {
        let Resolution::Found(record) = ingest(
            "@software{tool,\n  title = {A tool},\n  author = {Roe, Richard},\n  year = 2024,\n  doi = {10.5281/ZENODO.1}\n}",
        ) else {
            panic!("expected a record");
        };
        assert_eq!(record.description.title, "A tool");
        assert_eq!(record.description.authors, ["Roe, Richard"]);
        assert_eq!(record.description.year, Some(2024));
        assert_eq!(
            record.identifiers.doi.as_ref().unwrap().as_str(),
            "10.5281/zenodo.1"
        );
        // Provenance names the provider and holds no handle to refresh with.
        assert_eq!(record.provenance.provider.as_str(), "local");
        assert!(record.provenance.provider_id.is_none());
        assert!(record.provenance.revision.is_none());
    }

    #[test]
    fn ingest_keeps_the_payload_byte_identical() {
        let source = "@misc{k,  title = {T}  ,  note = {  spaced  }  }";
        let Resolution::Found(record) = ingest(source) else {
            panic!("expected a record");
        };
        assert_eq!(record.payload.source(), source);
    }

    #[test]
    fn ingest_rejects_an_entry_it_cannot_describe() {
        let untitled = LocalProvider::new().ingest(entry("@misc{k,doi={10.1/x}}"));
        assert!(matches!(
            untitled,
            Err(ProviderError::Mapping(MappingError::InvalidPayload { .. }))
        ));
    }

    #[test]
    fn an_unusable_identifier_is_dropped_rather_than_failing_the_entry() {
        let Resolution::Found(record) = ingest("@misc{k,title={T},doi={not a doi}}") else {
            panic!("expected a record");
        };
        assert!(record.identifiers.doi.is_none());
    }

    #[tokio::test]
    async fn the_local_provider_resolves_nothing_and_refreshes_nothing() {
        let provider = LocalProvider::new();
        let locators = [Locator::Doi(Doi::new("10.1/x").unwrap())];
        assert_eq!(
            provider.resolve(&locators).await.unwrap(),
            [Resolution::UnsupportedLocator]
        );
        let requests = [RefreshRequest {
            bibi_id: bibi_core::BibiId::new(),
            provider_id: ProviderId::new("x").unwrap(),
            stored_revision: None,
        }];
        let refreshed = provider.refresh_metadata(&requests).await;
        assert!(matches!(
            refreshed[0].result,
            Ok(RefreshState::Unrefreshable)
        ));
        assert!(!provider.capabilities().refresh);
        assert!(provider.capabilities().ingest_without_refresh());
    }
}
