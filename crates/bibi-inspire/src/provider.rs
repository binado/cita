//! INSPIRE retrieval and mapping behind the strict provider contract.

use crate::{
    batching, error,
    join::{self, DeclaredKeys},
    mapping::{self, MappedRecord},
    transport::{RECORD_FIELDS, Transport},
    wire::LiteratureRecord,
};
use bibi_core::{
    ArxivId, Bibtex, Doi, Locator, ProviderId, ProviderName, RecordState, Source,
    remote::{Provider, ProviderError, RetrievalError},
};
use std::collections::HashMap;

/// The INSPIRE provider.
#[derive(Debug)]
pub struct InspireProvider {
    transport: Transport,
}

impl InspireProvider {
    /// Build against the public INSPIRE API.
    pub fn new() -> Result<Self, RetrievalError> {
        Ok(Self::with_transport(Transport::builder().build()?))
    }

    /// Build over a configured transport.
    pub fn with_transport(transport: Transport) -> Self {
        Self { transport }
    }

    /// Retrieval seam for diagnostics and fixture capture.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    async fn search_hits(&self, terms: &[String]) -> Result<Vec<LiteratureRecord>, ProviderError> {
        let mut hits = Vec::new();
        for batch in batching::batch(terms) {
            let raw = self
                .transport
                .search_json(&batching::query(&batch), batch.len(), RECORD_FIELDS)
                .await?;
            hits.extend(mapping::parse_search(&raw)?.hits.hits);
        }
        Ok(hits)
    }

    async fn payloads(
        &self,
        records: &[MappedRecord],
    ) -> Result<HashMap<ProviderId, Bibtex>, ProviderError> {
        let mut paired = HashMap::new();
        for batch in batching::batch_ids(
            &records
                .iter()
                .map(|record| record.provider_id.clone())
                .collect::<Vec<_>>(),
        ) {
            let selected = records
                .iter()
                .filter(|record| batch.contains(&record.provider_id))
                .collect::<Vec<_>>();
            let declared = selected
                .iter()
                .map(|record| DeclaredKeys {
                    provider_id: record.provider_id.clone(),
                    texkeys: record.texkeys.clone(),
                })
                .collect::<Vec<_>>();
            let ids = selected
                .iter()
                .map(|record| record.provider_id.clone())
                .collect::<Vec<_>>();
            let raw = self
                .transport
                .search_bibtex(&batching::query(&control_number_terms(&ids)?), ids.len())
                .await?;
            let entries = mapping::split_entries(&raw)?;
            for joined in join::join(&ids, &declared, entries)? {
                paired.insert(joined.provider_id, joined.bibtex);
            }
        }
        Ok(paired)
    }
}

impl Provider for InspireProvider {
    fn name(&self) -> ProviderName {
        ProviderName::Inspire
    }

    async fn resolve(&self, locators: &[Locator]) -> Result<Vec<RecordState>, ProviderError> {
        let mut terms = Vec::with_capacity(locators.len());
        for locator in locators {
            terms.push(
                query_term(locator).ok_or_else(|| ProviderError::Unsupported {
                    provider: self.name(),
                    locator: locator.to_string(),
                })?,
            );
        }
        let hits = self.search_hits(&terms).await?;
        let mut mapped = Vec::with_capacity(locators.len());
        for locator in locators {
            let hit = hits
                .iter()
                .find(|hit| identifies_wire(hit, locator))
                .ok_or_else(|| ProviderError::NotFound {
                    provider: self.name(),
                    locator: locator.to_string(),
                })?;
            mapped.push(mapping::map_record(hit)?);
        }

        let mut distinct = mapped.clone();
        distinct.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
        distinct.dedup_by(|left, right| left.provider_id == right.provider_id);
        let payloads = self.payloads(&distinct).await?;
        let canonical = distinct
            .into_iter()
            .map(|record| (record.provider_id.clone(), record))
            .collect::<HashMap<_, _>>();
        mapped
            .into_iter()
            .map(|record| {
                let complete = &canonical[&record.provider_id];
                RecordState::new(
                    Source::managed(self.name(), complete.provider_id.clone()),
                    complete.identifiers.clone(),
                    complete.description.clone(),
                    payloads
                        .get(&complete.provider_id)
                        .cloned()
                        .ok_or_else(|| {
                            ProviderError::contract(
                                self.name(),
                                format!("record {} lost its paired BibTeX", complete.provider_id),
                            )
                        })?
                        .clone(),
                )
                .map_err(|error| ProviderError::contract(self.name(), error.to_string()))
            })
            .collect()
    }
}

fn query_term(locator: &Locator) -> Option<String> {
    match locator {
        Locator::Texkey(key) => Some(format!("texkeys:{key}")),
        Locator::Arxiv(id) => Some(format!("arxiv:{id}")),
        Locator::Doi(doi) => Some(format!("doi:{doi}")),
        Locator::ProviderIdentity(provider, id) if *provider == ProviderName::Inspire => {
            control_number(id.as_str()).map(|id| format!("control_number:{id}"))
        }
        Locator::ProviderIdentity(_, _) => None,
        Locator::Opaque(value) => control_number(value).map(|id| format!("control_number:{id}")),
    }
}

fn control_number_terms(ids: &[ProviderId]) -> Result<Vec<String>, ProviderError> {
    ids.iter()
        .map(|id| {
            control_number(id.as_str())
                .map(|id| format!("control_number:{id}"))
                .ok_or_else(|| error::invalid_value("INSPIRE record id", id.as_str()).into())
        })
        .collect()
}

fn control_number(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(id) = value.parse::<u64>() {
        return (id > 0).then_some(id);
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "inspirehep.net" {
        return None;
    }
    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    path.strip_prefix("/literature/")?
        .parse()
        .ok()
        .filter(|id| *id > 0)
}

fn identifies_wire(record: &LiteratureRecord, locator: &Locator) -> bool {
    match locator {
        Locator::Texkey(key) => record.metadata.texkeys.iter().any(|value| value == key),
        Locator::Arxiv(id) => record
            .metadata
            .arxiv_eprints
            .iter()
            .any(|value| ArxivId::new(&value.value).is_ok_and(|candidate| candidate == *id)),
        Locator::Doi(doi) => record
            .metadata
            .dois
            .iter()
            .any(|value| Doi::new(&value.value).is_ok_and(|candidate| candidate == *doi)),
        Locator::ProviderIdentity(provider, id) if *provider == ProviderName::Inspire => {
            control_number(id.as_str()).is_some_and(|id| record.record_id() == Some(id))
        }
        Locator::ProviderIdentity(_, _) => false,
        Locator::Opaque(value) => {
            control_number(value).is_some_and(|id| record.record_id() == Some(id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_control_numbers_and_locator_queries() {
        assert_eq!(control_number("1124337"), Some(1124337));
        assert_eq!(
            control_number("https://inspirehep.net/literature/1124337"),
            Some(1124337)
        );
        assert_eq!(
            query_term(&"1207.7214".parse().unwrap()),
            Some("arxiv:1207.7214".into())
        );
    }
}
