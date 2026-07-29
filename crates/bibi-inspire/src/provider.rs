//! Composition: retrieval, then mapping, behind the generic contract.

use crate::{
    batching, error,
    join::{self, DeclaredKeys},
    mapping::{self, MappedRecord},
    transport::{JOIN_FIELDS, RECORD_FIELDS, Transport},
};
use bibi_bibtex::BibtexEntry;
use bibi_core::{Locator, Provenance, ProviderId, ProviderName, ProviderOwned};
use bibi_provider::{
    MappingError, PayloadItem, PayloadRequest, Provider, ProviderCapabilities, ProviderError,
    ProviderFuture, ProviderMetadata, RefreshItem, RefreshRequest, RefreshState, Resolution,
    RetrievalError,
};
use std::collections::HashMap;

/// The INSPIRE provider.
#[derive(Debug)]
pub struct InspireProvider {
    name: ProviderName,
    transport: Transport,
}

impl InspireProvider {
    /// Build a provider against the public INSPIRE API.
    pub fn new() -> Result<Self, RetrievalError> {
        Ok(Self::with_transport(Transport::builder().build()?))
    }

    /// Build a provider over a configured transport.
    pub fn with_transport(transport: Transport) -> Self {
        Self {
            name: error::provider(),
            transport,
        }
    }

    /// The retrieval half, for diagnostics and fixture capture.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Fetch and map records for a set of query terms.
    async fn search(&self, terms: &[String]) -> Result<Vec<MappedRecord>, ProviderError> {
        let mut records = Vec::new();
        for batch in batching::batch(terms) {
            let raw = self
                .transport
                .search_json(&batching::query(&batch), batch.len(), RECORD_FIELDS)
                .await
                .map_err(ProviderError::Retrieval)?;
            records.extend(mapping::map_search(&raw).map_err(ProviderError::Mapping)?);
        }
        Ok(records)
    }

    /// The texkeys for a batch, from request tokens or from one narrowed request.
    async fn declared_keys(
        &self,
        requests: &[PayloadRequest],
    ) -> Result<Vec<DeclaredKeys>, ProviderError> {
        let mut known = Vec::new();
        let mut unknown = Vec::new();
        for request in requests {
            if request.join_tokens.is_empty() {
                unknown.push(request.provider_id.clone());
            } else {
                known.push(DeclaredKeys {
                    provider_id: request.provider_id.clone(),
                    texkeys: request.join_tokens.clone(),
                });
            }
        }
        if !unknown.is_empty() {
            let mut looked_up = Vec::new();
            for batch in batching::batch_ids(&unknown) {
                let terms = control_number_terms(&batch)?;
                let raw = self
                    .transport
                    .search_json(&batching::query(&terms), terms.len(), JOIN_FIELDS)
                    .await
                    .map_err(ProviderError::Retrieval)?;
                looked_up.extend(mapping::map_declared_keys(&raw).map_err(ProviderError::Mapping)?);
            }
            known.extend(looked_up);
        }
        Ok(known)
    }

    /// Fetch BibTeX for a batch and pair it back to the records requested.
    async fn payloads_for(
        &self,
        requests: &[PayloadRequest],
    ) -> Result<Vec<PayloadItem>, ProviderError> {
        let provider_ids = requests
            .iter()
            .map(|request| request.provider_id.clone())
            .collect::<Vec<_>>();
        let declared = self.declared_keys(requests).await?;
        let terms = control_number_terms(&provider_ids)?;
        let raw = self
            .transport
            .search_bibtex(&batching::query(&terms), terms.len())
            .await
            .map_err(ProviderError::Retrieval)?;
        let entries = mapping::split_entries(&raw).map_err(ProviderError::Mapping)?;
        join::join(&provider_ids, &declared, entries).map_err(ProviderError::Mapping)
    }
}

impl Provider for InspireProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::NETWORK
    }

    fn recognizes_unqualified_id(&self, value: &str) -> bool {
        control_number(value).is_some()
    }

    /// Resolve locators in batches, matching results back by identifier.
    ///
    /// A JSON record carries its own arXiv id, DOI, and control number, so the
    /// association between a result and the locator that asked for it is in the
    /// payload rather than inferred — no join protocol is needed here. Two
    /// locators may legitimately resolve to one record; both return it, and the
    /// application's duplicate policy collapses them.
    fn resolve<'a>(
        &'a self,
        locators: &'a [Locator],
    ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>> {
        Box::pin(async move {
            let terms = locators.iter().map(query_term).collect::<Vec<_>>();
            let mut wanted = terms.iter().flatten().cloned().collect::<Vec<_>>();
            wanted.sort();
            wanted.dedup();
            if wanted.is_empty() {
                return Ok(locators
                    .iter()
                    .map(|_| Resolution::UnsupportedLocator)
                    .collect());
            }

            let records = self.search(&wanted).await?;
            let matched = locators
                .iter()
                .zip(&terms)
                .map(|(locator, term)| {
                    term.as_ref()
                        .and_then(|_| records.iter().find(|record| identifies(record, locator)))
                })
                .collect::<Vec<_>>();

            // One payload request covers every record any locator resolved to.
            let mut resolved = matched.iter().flatten().copied().collect::<Vec<_>>();
            resolved.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
            resolved.dedup_by(|left, right| left.provider_id == right.provider_id);
            let payloads = self.payloads(&resolved).await?;

            locators
                .iter()
                .zip(terms)
                .zip(matched)
                .map(|((_, term), record)| match (term, record) {
                    (None, _) => Ok(Resolution::UnsupportedLocator),
                    (Some(_), None) => Ok(Resolution::NotFound),
                    (Some(_), Some(record)) => {
                        let payload =
                            payloads.get(&record.provider_id).cloned().ok_or_else(|| {
                                // INSPIRE identified the record and then declined to
                                // render it. bibi cannot store a record without a
                                // payload, and inventing one is exactly what it
                                // refuses to do.
                                ProviderError::Mapping(error::ambiguous_join(format!(
                                    "record {} resolved but returned no BibTeX entry",
                                    record.provider_id
                                )))
                            })?;
                        Ok(Resolution::Found(Box::new(ProviderOwned {
                            provenance: Provenance::managed(
                                self.name.clone(),
                                record.provider_id.clone(),
                                record.revision.clone(),
                            ),
                            identifiers: record.identifiers.clone(),
                            description: record.description.clone(),
                            payload,
                        })))
                    }
                })
                .collect()
        })
    }

    /// Examine records by stable id, requesting only the narrowed field set.
    fn refresh_metadata<'a>(
        &'a self,
        requests: &'a [RefreshRequest],
    ) -> ProviderFuture<'a, Vec<RefreshItem>> {
        Box::pin(async move {
            let provider_ids = requests
                .iter()
                .map(|request| request.provider_id.clone())
                .collect::<Vec<_>>();
            let terms = match control_number_terms(&provider_ids) {
                Ok(terms) => terms,
                Err(error) => return failed(requests, &error),
            };
            let records = match self.search(&terms).await {
                Ok(records) => records,
                Err(error) => return failed(requests, &error),
            };
            let by_id = records
                .iter()
                .map(|record| (record.provider_id.clone(), record))
                .collect::<HashMap<_, _>>();
            requests
                .iter()
                .map(|request| RefreshItem {
                    bibi_id: request.bibi_id,
                    result: Ok(match by_id.get(&request.provider_id) {
                        // Absence is a per-record no-op with a warning, not a
                        // failure: identifiers change and records get merged.
                        None => RefreshState::Missing,
                        Some(record) => RefreshState::Metadata(Box::new(ProviderMetadata {
                            provider_id: record.provider_id.clone(),
                            revision: record.revision.clone(),
                            identifiers: record.identifiers.clone(),
                            description: record.description.clone(),
                            join_tokens: record.texkeys.clone(),
                        })),
                    }),
                })
                .collect()
        })
    }

    fn fetch_payloads<'a>(
        &'a self,
        requests: &'a [PayloadRequest],
    ) -> ProviderFuture<'a, Result<Vec<PayloadItem>, ProviderError>> {
        Box::pin(async move {
            let mut items = Vec::with_capacity(requests.len());
            let ids = requests
                .iter()
                .map(|request| request.provider_id.clone())
                .collect::<Vec<_>>();
            for batch_ids in batching::batch_ids(&ids) {
                let batch = requests
                    .iter()
                    .filter(|request| batch_ids.contains(&request.provider_id))
                    .cloned()
                    .collect::<Vec<_>>();
                items.extend(self.payloads_for(&batch).await?);
            }
            Ok(items)
        })
    }

    /// INSPIRE has nothing to say about an entry the user wrote.
    fn ingest(&self, _entry: BibtexEntry) -> Result<Resolution, ProviderError> {
        Ok(Resolution::UnsupportedLocator)
    }
}

impl InspireProvider {
    /// Payloads for already-mapped records, keyed by provider id.
    async fn payloads(
        &self,
        records: &[&MappedRecord],
    ) -> Result<HashMap<ProviderId, BibtexEntry>, ProviderError> {
        if records.is_empty() {
            return Ok(HashMap::new());
        }
        let requests = records
            .iter()
            .map(|record| PayloadRequest {
                provider_id: record.provider_id.clone(),
                join_tokens: record.texkeys.clone(),
            })
            .collect::<Vec<_>>();
        let mut paired = HashMap::new();
        let ids = requests
            .iter()
            .map(|request| request.provider_id.clone())
            .collect::<Vec<_>>();
        for batch_ids in batching::batch_ids(&ids) {
            let batch = requests
                .iter()
                .filter(|request| batch_ids.contains(&request.provider_id))
                .cloned()
                .collect::<Vec<_>>();
            for item in self.payloads_for(&batch).await? {
                if let Some(payload) = item.payload {
                    paired.insert(item.provider_id, payload);
                }
            }
        }
        Ok(paired)
    }
}

/// Fail every request in one call with the same error.
fn failed(requests: &[RefreshRequest], error: &ProviderError) -> Vec<RefreshItem> {
    let message = error.to_string();
    let retrieval = matches!(error, ProviderError::Retrieval(_));
    requests
        .iter()
        .map(|request| RefreshItem {
            bibi_id: request.bibi_id,
            result: Err(if retrieval {
                ProviderError::Retrieval(error::transport(&message))
            } else {
                ProviderError::Mapping(MappingError::ContractViolation {
                    provider: error::provider(),
                    message: message.clone(),
                })
            }),
        })
        .collect()
}

/// The query term that asks for one locator, if INSPIRE handles its kind.
fn query_term(locator: &Locator) -> Option<String> {
    match locator {
        Locator::Arxiv(id) => Some(format!("arxiv:{id}")),
        Locator::Doi(doi) => Some(format!("doi:{doi}")),
        Locator::ProviderId(value) => {
            control_number(value).map(|id| format!("control_number:{id}"))
        }
    }
}

fn control_number_terms(provider_ids: &[ProviderId]) -> Result<Vec<String>, ProviderError> {
    provider_ids
        .iter()
        .map(|provider_id| {
            control_number(provider_id.as_str())
                .map(|id| format!("control_number:{id}"))
                .ok_or_else(|| {
                    ProviderError::Mapping(error::invalid_value(
                        "INSPIRE record id",
                        provider_id.as_str(),
                    ))
                })
        })
        .collect()
}

/// Read an INSPIRE control number out of a bare id or a literature URL.
///
/// This is where INSPIRE's own syntax is recognized, including its web address:
/// a provider's URL is that provider's business, not the core locator parser's.
fn control_number(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(id) = value.parse::<u64>() {
        return (id > 0).then_some(id);
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "inspirehep.net" {
        return None;
    }
    let path = url.path();
    let path = path.strip_suffix('/').unwrap_or(path);
    path.strip_prefix("/literature/")?
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
}

/// Does this record answer that locator?
fn identifies(record: &MappedRecord, locator: &Locator) -> bool {
    match locator {
        Locator::Arxiv(id) => record.identifiers.arxiv.as_ref() == Some(id),
        Locator::Doi(doi) => record.identifiers.doi.as_ref() == Some(doi),
        Locator::ProviderId(value) => {
            control_number(value).is_some_and(|id| record.provider_id.as_str() == id.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_its_own_id_syntax_and_nothing_else() {
        let provider = InspireProvider::with_transport(Transport::builder().build().unwrap());
        assert!(provider.recognizes_unqualified_id("1124337"));
        assert!(provider.recognizes_unqualified_id("https://inspirehep.net/literature/1124337"));
        assert!(provider.recognizes_unqualified_id("https://inspirehep.net/literature/42/"));
        for value in [
            "0",
            "",
            "12a",
            "2401.00001",
            "10.1/x",
            "http://inspirehep.net/literature/42",
            "https://inspirehep.net/authors/42",
            "https://example.com/literature/42",
        ] {
            assert!(!provider.recognizes_unqualified_id(value), "{value}");
        }
    }

    #[test]
    fn builds_the_query_term_each_locator_kind_deserves() {
        assert_eq!(
            query_term(&Locator::Arxiv(
                bibi_core::ArxivId::new("1207.7214v2").unwrap()
            )),
            Some("arxiv:1207.7214".to_owned())
        );
        assert_eq!(
            query_term(&Locator::Doi(bibi_core::Doi::new("10.1/ABC").unwrap())),
            Some("doi:10.1/abc".to_owned())
        );
        assert_eq!(
            query_term(&Locator::ProviderId("1124337".into())),
            Some("control_number:1124337".to_owned())
        );
        // An id INSPIRE cannot read is unsupported, not not-found.
        assert_eq!(
            query_term(&Locator::ProviderId("2024ApJ...1X".into())),
            None
        );
    }
}
