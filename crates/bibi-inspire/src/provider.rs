//! Composition: retrieval, then mapping, behind the generic contract.

use crate::{
    batching, error,
    join::{self, DeclaredKeys},
    mapping::{self, MappedRecord},
    transport::{JOIN_FIELDS, RECORD_FIELDS, Transport},
    wire::LiteratureRecord,
};
use bibi_core::{
    ArxivId, Doi, Locator, Provenance, Provider, ProviderId, ProviderOwned,
    remote::{
        BibtexEntry, MappingError, PayloadItem, PayloadRequest, ProviderError, ProviderMetadata,
        RefreshItem, RefreshRequest, RefreshState, RemoteProvider, Resolution, RetrievalError,
    },
};
use std::collections::HashMap;

/// The INSPIRE provider.
#[derive(Debug)]
pub struct InspireProvider {
    name: Provider,
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

    /// Fetch the raw hits for a set of query terms, one batch request per
    /// [`batching::batch`] chunk.
    ///
    /// Hits come back unmapped: matching and mapping are per-hit decisions, and
    /// one malformed record must not stop the others from answering.
    async fn search_hits(&self, terms: &[String]) -> Result<Vec<LiteratureRecord>, ProviderError> {
        let mut hits = Vec::new();
        for batch in batching::batch(terms) {
            let raw = self
                .transport
                .search_json(&batching::query(&batch), batch.len(), RECORD_FIELDS)
                .await
                .map_err(ProviderError::Retrieval)?;
            hits.extend(
                mapping::parse_search(&raw)
                    .map_err(ProviderError::Mapping)?
                    .hits
                    .hits,
            );
        }
        Ok(hits)
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

impl RemoteProvider for InspireProvider {
    fn name(&self) -> Provider {
        self.name
    }

    /// Resolve locators in batches, matching results back by identifier.
    ///
    /// A JSON record carries its own arXiv id, DOI, and control number, so the
    /// association between a result and the locator that asked for it is in the
    /// payload rather than inferred — no join protocol is needed here. Matching
    /// runs on the wire record against every identifier it declares: a record
    /// with two DOIs is found by either, and a hit that cannot be mapped is
    /// irrelevant unless a locator actually asked for it. Two locators may
    /// legitimately resolve to one record; both return it, and the
    /// application's duplicate policy collapses them.
    async fn resolve(&self, locators: &[Locator]) -> Result<Vec<Resolution>, ProviderError> {
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

        let hits = self.search_hits(&wanted).await?;
        let matched = locators
            .iter()
            .zip(&terms)
            .map(|(locator, term)| {
                term.as_ref()
                    .and_then(|_| hits.iter().find(|hit| identifies_wire(hit, locator)))
            })
            .collect::<Vec<_>>();

        // Map each distinct matched record once; a record a locator asked
        // for that cannot be mapped cannot be stored, and inventing a
        // placeholder for it is exactly what bibi refuses to do.
        let mut distinct = matched.iter().flatten().copied().collect::<Vec<_>>();
        distinct.sort_by_key(|hit| hit.record_id());
        distinct.dedup_by_key(|hit| hit.record_id());
        let mut records = Vec::with_capacity(distinct.len());
        for hit in distinct {
            records.push(mapping::map_record(hit).map_err(ProviderError::Mapping)?);
        }
        // One payload request covers every record any locator resolved to.
        records.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
        let payloads = self.payloads(&records.iter().collect::<Vec<_>>()).await?;

        locators
            .iter()
            .zip(terms)
            .zip(matched)
            .map(|((_, term), hit)| match (term, hit) {
                (None, _) => Ok(Resolution::UnsupportedLocator),
                (Some(_), None) => Ok(Resolution::NotFound),
                (Some(_), Some(hit)) => {
                    let record = records
                        .iter()
                        .find(|record| {
                            hit.record_id()
                                .is_some_and(|id| record.provider_id.as_str() == id.to_string())
                        })
                        .expect("every matched hit was mapped above");
                    let payload = payloads.get(&record.provider_id).cloned().ok_or_else(|| {
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
                            self.name,
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
    }

    /// Examine records by stable id, requesting only the narrowed field set.
    ///
    /// Every failure is per item, as the contract requires: an id that is not
    /// a control number, a batch that cannot be retrieved, and a record that
    /// cannot be mapped each fail their own items and no others, because one
    /// bad record must not stop a refresh of everything else.
    async fn refresh_metadata(&self, requests: &[RefreshRequest]) -> Vec<RefreshItem> {
        let mut mapped: HashMap<String, MappedRecord> = HashMap::new();
        let mut failures: HashMap<String, ProviderError> = HashMap::new();

        let mut queryable = Vec::new();
        for request in requests {
            if control_number(request.provider_id.as_str()).is_some() {
                queryable.push(request.provider_id.clone());
            } else {
                failures.insert(
                    request.provider_id.as_str().to_owned(),
                    ProviderError::Mapping(error::invalid_value(
                        "INSPIRE record id",
                        request.provider_id.as_str(),
                    )),
                );
            }
        }

        for batch in batching::batch_ids(&queryable) {
            let terms = control_number_terms(&batch).expect("every id was validated above");
            let raw = match self
                .transport
                .search_json(&batching::query(&terms), terms.len(), RECORD_FIELDS)
                .await
            {
                Ok(raw) => raw,
                Err(error) => {
                    // A batch that cannot be retrieved fails its own
                    // records; the others still answer.
                    let message = error.to_string();
                    for id in &batch {
                        failures.insert(
                            id.as_str().to_owned(),
                            ProviderError::Retrieval(error::transport(&message)),
                        );
                    }
                    continue;
                }
            };
            let response = match mapping::parse_search(&raw) {
                Ok(response) => response,
                Err(error) => {
                    let message = error.to_string();
                    for id in &batch {
                        failures.insert(
                            id.as_str().to_owned(),
                            ProviderError::Mapping(MappingError::ContractViolation {
                                provider: error::provider(),
                                message: message.clone(),
                            }),
                        );
                    }
                    continue;
                }
            };
            for hit in &response.hits.hits {
                let wire_id = hit.record_id();
                match mapping::map_record(hit) {
                    Ok(record) => {
                        mapped.insert(record.provider_id.as_str().to_owned(), record);
                    }
                    // A record that cannot be mapped fails its own item.
                    // One without a readable id answers no request at all,
                    // and its requester reports it absent.
                    Err(error) => {
                        if let Some(id) = wire_id {
                            failures.insert(id.to_string(), ProviderError::Mapping(error));
                        }
                    }
                }
            }
        }

        requests
            .iter()
            .map(|request| RefreshItem {
                bibi_id: request.bibi_id,
                result: match mapped.get(request.provider_id.as_str()) {
                    Some(record) => Ok(RefreshState::Metadata(Box::new(ProviderMetadata {
                        provider_id: record.provider_id.clone(),
                        revision: record.revision.clone(),
                        identifiers: record.identifiers.clone(),
                        description: record.description.clone(),
                        join_tokens: record.texkeys.clone(),
                    }))),
                    // Absence is a per-record no-op with a warning, not a
                    // failure: identifiers change and records get merged.
                    None => match failures.remove(request.provider_id.as_str()) {
                        Some(error) => Err(error),
                        None => Ok(RefreshState::Missing),
                    },
                },
            })
            .collect()
    }

    async fn fetch_payloads(
        &self,
        requests: &[PayloadRequest],
    ) -> Result<Vec<PayloadItem>, ProviderError> {
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

/// The query term that asks for one locator, if INSPIRE handles its kind.
fn query_term(locator: &Locator) -> Option<String> {
    match locator {
        Locator::Key(key) => Some(format!("texkeys:{key}")),
        Locator::Arxiv(id) => Some(format!("arxiv:{id}")),
        Locator::Doi(doi) => Some(format!("doi:{doi}")),
        Locator::ProviderIdentity(_, id) => {
            control_number(id.as_str()).map(|id| format!("control_number:{id}"))
        }
        Locator::Opaque(value) => control_number(value).map(|id| format!("control_number:{id}")),
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

/// Does this unmapped wire record answer that locator?
///
/// Matching runs against every identifier the record declares, not only the
/// canonical one mapping keeps: a record with two DOIs is found by either of
/// them. Values bibi cannot normalize cannot match, which is the same policy
/// mapping applies when projecting the canonical identifiers.
fn identifies_wire(record: &LiteratureRecord, locator: &Locator) -> bool {
    match locator {
        Locator::Key(key) => record
            .metadata
            .texkeys
            .iter()
            .any(|texkey| texkey == key.as_str()),
        Locator::Arxiv(id) => record
            .metadata
            .arxiv_eprints
            .iter()
            .any(|eprint| ArxivId::new(&eprint.value).is_ok_and(|candidate| candidate == *id)),
        Locator::Doi(doi) => record
            .metadata
            .dois
            .iter()
            .any(|declared| Doi::new(&declared.value).is_ok_and(|candidate| candidate == *doi)),
        Locator::ProviderIdentity(_, id) => {
            control_number(id.as_str()).is_some_and(|id| record.record_id() == Some(id))
        }
        Locator::Opaque(value) => {
            control_number(value).is_some_and(|id| record.record_id() == Some(id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_its_own_id_syntax_and_nothing_else() {
        assert!(control_number("1124337").is_some());
        assert!(control_number("https://inspirehep.net/literature/1124337").is_some());
        assert!(control_number("https://inspirehep.net/literature/42/").is_some());
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
            assert!(control_number(value).is_none(), "{value}");
        }
    }

    #[test]
    fn builds_the_query_term_each_locator_kind_deserves() {
        assert_eq!(
            query_term(&Locator::Key(
                bibi_bibtex::CitationKey::new("Aad:2012tfa").unwrap()
            )),
            Some("texkeys:Aad:2012tfa".to_owned())
        );
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
            query_term(&Locator::ProviderIdentity(
                bibi_core::Provider::Inspire,
                bibi_core::ProviderId::new("1124337").unwrap()
            )),
            Some("control_number:1124337".to_owned())
        );
        assert_eq!(
            query_term(&Locator::Opaque("1124337".into())),
            Some("control_number:1124337".to_owned())
        );
        // An id INSPIRE cannot read is unsupported, not not-found.
        assert_eq!(query_term(&Locator::Opaque("2024ApJ...1X".into())), None);
    }
}
