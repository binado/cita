//! Generic remote-provider fakes and contract verification.

use super::{
    MappingError, PayloadItem, PayloadRequest, ProviderError, ProviderMetadata, RefreshItem,
    RefreshRequest, RefreshState, RemoteProvider, Resolution, RetrievalError,
};
use crate::{
    Description, Identifiers, Locator, Provenance, Provider, ProviderId, ProviderOwned, Revision,
};
use bibi_bibtex::BibtexEntry;
use std::{collections::HashMap, future::Future, sync::Mutex};

/// One call a fake provider received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderCall {
    /// A resolution batch.
    Resolve(Vec<String>),
    /// A complete current metadata batch.
    RefreshMetadata(Vec<ProviderId>),
    /// A payload batch.
    FetchPayloads(Vec<ProviderId>),
}

#[derive(Clone, Debug)]
enum Answer {
    Records(HashMap<String, ProviderOwned>),
    Retrieval(String),
    Mapping(String),
}

/// A scriptable remote provider.
pub struct FakeProvider {
    name: Provider,
    answer: Answer,
    unsupported: Vec<String>,
    metadata: HashMap<ProviderId, RefreshState>,
    metadata_errors: HashMap<ProviderId, String>,
    payloads: HashMap<ProviderId, Option<BibtexEntry>>,
    payload_error: Option<String>,
    calls: Mutex<Vec<ProviderCall>>,
}

impl FakeProvider {
    /// Construct an empty fake.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.parse().expect("test provider name"),
            answer: Answer::Records(HashMap::new()),
            unsupported: Vec::new(),
            metadata: HashMap::new(),
            metadata_errors: HashMap::new(),
            payloads: HashMap::new(),
            payload_error: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Answer a locator with a record.
    pub fn with_record(mut self, locator: &str, record: ProviderOwned) -> Self {
        if let Answer::Records(records) = &mut self.answer {
            records.insert(locator.to_owned(), record);
        }
        self
    }

    /// Mark a locator as unsupported rather than absent.
    pub fn unsupported(mut self, locator: &str) -> Self {
        self.unsupported.push(locator.to_owned());
        self
    }

    /// Fail resolution as retrieval.
    pub fn failing_retrieval(mut self, message: &str) -> Self {
        self.answer = Answer::Retrieval(message.to_owned());
        self
    }

    /// Fail resolution as mapping.
    pub fn failing_mapping(mut self, message: &str) -> Self {
        self.answer = Answer::Mapping(message.to_owned());
        self
    }

    /// Answer refresh metadata for an id.
    pub fn with_refresh(mut self, provider_id: &str, state: RefreshState) -> Self {
        self.metadata
            .insert(ProviderId::new(provider_id).expect("test id"), state);
        self
    }

    /// Fail refresh metadata for an id.
    pub fn failing_refresh(mut self, provider_id: &str, message: &str) -> Self {
        self.metadata_errors.insert(
            ProviderId::new(provider_id).expect("test id"),
            message.to_owned(),
        );
        self
    }

    /// Answer a payload fetch.
    pub fn with_payload(mut self, provider_id: &str, payload: Option<BibtexEntry>) -> Self {
        self.payloads
            .insert(ProviderId::new(provider_id).expect("test id"), payload);
        self
    }

    /// Fail every payload fetch.
    pub fn failing_payloads(mut self, message: &str) -> Self {
        self.payload_error = Some(message.to_owned());
        self
    }

    /// Every call received.
    pub fn calls(&self) -> Vec<ProviderCall> {
        self.calls.lock().expect("call log").clone()
    }

    /// Count calls matching a predicate.
    pub fn call_count(&self, kind: fn(&ProviderCall) -> bool) -> usize {
        self.calls().iter().filter(|call| kind(call)).count()
    }

    fn retrieval(&self, message: &str) -> ProviderError {
        RetrievalError::Transport {
            provider: self.name,
            message: message.to_owned(),
        }
        .into()
    }

    fn mapping(&self, message: &str) -> ProviderError {
        MappingError::AmbiguousJoin {
            provider: self.name,
            message: message.to_owned(),
        }
        .into()
    }
}

impl RemoteProvider for FakeProvider {
    fn name(&self) -> Provider {
        self.name
    }

    fn resolve(
        &self,
        locators: &[Locator],
    ) -> impl Future<Output = Result<Vec<Resolution>, ProviderError>> + Send {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::Resolve(
                locators.iter().map(ToString::to_string).collect(),
            ));
        async move {
            match &self.answer {
                Answer::Retrieval(message) => Err(self.retrieval(message)),
                Answer::Mapping(message) => Err(self.mapping(message)),
                Answer::Records(records) => Ok(locators
                    .iter()
                    .map(|locator| {
                        let rendered = locator.to_string();
                        records
                            .get(&rendered)
                            .cloned()
                            .map(|record| Resolution::Found(Box::new(record)))
                            .unwrap_or_else(|| {
                                if self.unsupported.contains(&rendered) {
                                    Resolution::UnsupportedLocator
                                } else {
                                    Resolution::NotFound
                                }
                            })
                    })
                    .collect()),
            }
        }
    }

    fn refresh_metadata(
        &self,
        requests: &[RefreshRequest],
    ) -> impl Future<Output = Vec<RefreshItem>> + Send {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::RefreshMetadata(
                requests
                    .iter()
                    .map(|request| request.provider_id.clone())
                    .collect(),
            ));
        async move {
            requests
                .iter()
                .map(|request| RefreshItem {
                    bibi_id: request.bibi_id,
                    result: self
                        .metadata_errors
                        .get(&request.provider_id)
                        .map(|message| Err(self.mapping(message)))
                        .unwrap_or_else(|| {
                            Ok(self
                                .metadata
                                .get(&request.provider_id)
                                .cloned()
                                .unwrap_or(RefreshState::Missing))
                        }),
                })
                .collect()
        }
    }

    fn fetch_payloads(
        &self,
        requests: &[PayloadRequest],
    ) -> impl Future<Output = Result<Vec<PayloadItem>, ProviderError>> + Send {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::FetchPayloads(
                requests
                    .iter()
                    .map(|request| request.provider_id.clone())
                    .collect(),
            ));
        async move {
            if let Some(message) = &self.payload_error {
                return Err(self.mapping(message));
            }
            Ok(requests
                .iter()
                .map(|request| PayloadItem {
                    provider_id: request.provider_id.clone(),
                    payload: self.payloads.get(&request.provider_id).cloned().flatten(),
                })
                .collect())
        }
    }
}

/// Build a provider-owned record for tests.
pub fn provider_record(
    provider: &str,
    provider_id: &str,
    texkey: &str,
    title: &str,
) -> ProviderOwned {
    ProviderOwned {
        provenance: Provenance::managed(
            provider.parse().expect("test provider name"),
            ProviderId::new(provider_id).expect("test id"),
            Some(Revision::new("r1").expect("test revision")),
        ),
        identifiers: Identifiers::default(),
        description: Description {
            title: title.to_owned(),
            ..Description::default()
        },
        payload: BibtexEntry::parse_one(format!("@article{{{texkey},title={{{title}}}}}"))
            .expect("test payload"),
    }
}

/// Build a payload entry for tests.
pub fn payload(texkey: &str, title: &str) -> BibtexEntry {
    BibtexEntry::parse_one(format!("@article{{{texkey},title={{{title}}}}}")).expect("test payload")
}

/// Build narrowed provider metadata for tests.
pub fn provider_metadata(
    provider_id: &str,
    revision: Option<&str>,
    title: &str,
) -> ProviderMetadata {
    ProviderMetadata {
        provider_id: ProviderId::new(provider_id).expect("test id"),
        revision: revision.map(|value| Revision::new(value).expect("test revision")),
        identifiers: Identifiers::default(),
        description: Description {
            title: title.to_owned(),
            ..Description::default()
        },
        join_tokens: Vec::new(),
    }
}

/// Verify the shape, identity, provenance, and correlation promises.
pub async fn verify_contract<P: RemoteProvider>(provider: &P, locators: &[Locator]) {
    let name = provider.name();
    assert_eq!(provider.name(), name, "provider name must be stable");

    let resolutions = provider
        .resolve(locators)
        .await
        .expect("a conforming provider answers probe locators");
    assert_eq!(
        resolutions.len(),
        locators.len(),
        "resolve must answer every locator positionally"
    );
    let mut found = Vec::new();
    for resolution in &resolutions {
        if let Resolution::Found(record) = resolution {
            assert_eq!(record.provenance.provider, name);
            assert!(record.provenance.provider_id.is_some());
            assert!(!record.description.title.trim().is_empty());
            found.push(record);
        }
    }
    assert!(
        found.len() >= 2,
        "contract probes must resolve at least two records to verify multi-item correlation"
    );

    let requests = found
        .iter()
        .take(2)
        .map(|record| RefreshRequest {
            bibi_id: crate::BibiId::new(),
            provider_id: record
                .provenance
                .provider_id
                .clone()
                .expect("remote records carry provider ids"),
            stored_revision: record.provenance.revision.clone(),
        })
        .collect::<Vec<_>>();
    let items = provider.refresh_metadata(&requests).await;
    assert_eq!(items.len(), requests.len());
    for request in &requests {
        assert_eq!(
            items
                .iter()
                .filter(|item| item.bibi_id == request.bibi_id)
                .count(),
            1,
            "refresh must correlate every input exactly once"
        );
    }

    let payload_requests = items
        .iter()
        .map(|item| match &item.result {
            Ok(RefreshState::Metadata(metadata)) => {
                let request = requests
                    .iter()
                    .find(|request| request.bibi_id == item.bibi_id)
                    .expect("refresh result correlates to an input");
                assert_eq!(
                    metadata.provider_id, request.provider_id,
                    "refresh metadata must preserve provider identity"
                );
                PayloadRequest {
                    provider_id: metadata.provider_id.clone(),
                    join_tokens: metadata.join_tokens.clone(),
                }
            }
            Ok(RefreshState::Missing) => {
                panic!("a just-resolved contract probe must still exist at refresh")
            }
            Err(error) => panic!("a just-resolved contract probe must refresh: {error}"),
        })
        .collect::<Vec<_>>();
    let payloads = provider
        .fetch_payloads(&payload_requests)
        .await
        .expect("a conforming provider answers payload probes");
    assert_eq!(payloads.len(), payload_requests.len());
    for request in &payload_requests {
        assert_eq!(
            payloads
                .iter()
                .filter(|item| item.provider_id == request.provider_id)
                .count(),
            1,
            "payloads must correlate every input exactly once"
        );
    }
}
