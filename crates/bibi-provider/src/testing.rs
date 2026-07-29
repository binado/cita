//! Fakes and a contract conformance suite.
//!
//! This module ships with the crate on purpose. A provider implementation lives
//! in its own crate, and the promises it has to keep — one result per request,
//! results in input order, a stable name, provenance that names itself — are
//! promises the registry makes on its behalf. Having one suite here means a new
//! provider crate proves them against the same checks rather than a rewritten
//! approximation of them.

use crate::{
    contract::{Provider, ProviderCapabilities, ProviderFuture},
    error::{MappingError, ProviderError, RetrievalError},
    outcome::{
        PayloadItem, PayloadRequest, ProviderMetadata, RefreshItem, RefreshRequest, RefreshState,
        Resolution,
    },
};
use bibi_bibtex::BibtexEntry;
use bibi_core::{
    Description, Identifiers, Locator, Provenance, ProviderId, ProviderName, ProviderOwned,
    Revision,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// One call a fake provider received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderCall {
    /// `resolve`, with the locators as rendered.
    Resolve(Vec<String>),
    /// `refresh_metadata`, with the provider ids requested.
    RefreshMetadata(Vec<ProviderId>),
    /// `fetch_payloads`, with the provider ids requested.
    FetchPayloads(Vec<ProviderId>),
}

/// How a fake provider should answer.
#[derive(Clone, Debug)]
enum Answer {
    Records(HashMap<String, ProviderOwned>),
    Retrieval(String),
    Mapping(String),
}

/// A scriptable provider for tests.
pub struct FakeProvider {
    name: ProviderName,
    capabilities: ProviderCapabilities,
    answer: Answer,
    recognized: Vec<String>,
    metadata: HashMap<ProviderId, RefreshState>,
    payloads: HashMap<ProviderId, Option<BibtexEntry>>,
    payload_error: Option<String>,
    calls: Mutex<Vec<ProviderCall>>,
}

impl FakeProvider {
    /// A provider that resolves neutral identifiers and refreshes.
    pub fn new(name: &str) -> Self {
        Self {
            name: ProviderName::new(name).expect("test provider name"),
            capabilities: ProviderCapabilities::NETWORK,
            answer: Answer::Records(HashMap::new()),
            recognized: Vec::new(),
            metadata: HashMap::new(),
            payloads: HashMap::new(),
            payload_error: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Override the declared capabilities.
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Answer `locator` with `record`.
    pub fn with_record(mut self, locator: &str, record: ProviderOwned) -> Self {
        if let Answer::Records(records) = &mut self.answer {
            records.insert(locator.to_owned(), record);
        }
        self
    }

    /// Fail every resolution with a retrieval error.
    pub fn failing_retrieval(mut self, message: &str) -> Self {
        self.answer = Answer::Retrieval(message.to_owned());
        self
    }

    /// Fail every resolution with a mapping error.
    pub fn failing_mapping(mut self, message: &str) -> Self {
        self.answer = Answer::Mapping(message.to_owned());
        self
    }

    /// Claim this bare id syntax as its own.
    pub fn recognizing(mut self, value: &str) -> Self {
        self.recognized.push(value.to_owned());
        self
    }

    /// Answer a refresh for `provider_id` with `state`.
    pub fn with_refresh(mut self, provider_id: &str, state: RefreshState) -> Self {
        self.metadata
            .insert(ProviderId::new(provider_id).expect("test id"), state);
        self
    }

    /// Answer a payload fetch for `provider_id`.
    pub fn with_payload(mut self, provider_id: &str, payload: Option<BibtexEntry>) -> Self {
        self.payloads
            .insert(ProviderId::new(provider_id).expect("test id"), payload);
        self
    }

    /// Fail every payload fetch, as an ambiguous join does.
    pub fn failing_payloads(mut self, message: &str) -> Self {
        self.payload_error = Some(message.to_owned());
        self
    }

    /// Every call this provider has received, in order.
    pub fn calls(&self) -> Vec<ProviderCall> {
        self.calls.lock().expect("call log").clone()
    }

    /// How many calls of each kind this provider has received.
    pub fn call_count(&self, kind: fn(&ProviderCall) -> bool) -> usize {
        self.calls().iter().filter(|call| kind(call)).count()
    }

    fn retrieval(&self, message: &str) -> ProviderError {
        ProviderError::Retrieval(RetrievalError::Transport {
            provider: self.name.clone(),
            message: message.to_owned(),
        })
    }

    fn mapping(&self, message: &str) -> ProviderError {
        ProviderError::Mapping(MappingError::AmbiguousJoin {
            provider: self.name.clone(),
            message: message.to_owned(),
        })
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    fn recognizes_unqualified_id(&self, value: &str) -> bool {
        self.recognized.iter().any(|claimed| claimed == value)
    }

    fn resolve<'a>(
        &'a self,
        locators: &'a [Locator],
    ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>> {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::Resolve(
                locators.iter().map(Locator::to_string).collect(),
            ));
        Box::pin(async move {
            match &self.answer {
                Answer::Retrieval(message) => Err(self.retrieval(message)),
                Answer::Mapping(message) => Err(self.mapping(message)),
                Answer::Records(records) => Ok(locators
                    .iter()
                    .map(|locator| match records.get(&locator.to_string()) {
                        Some(record) => Resolution::Found(Box::new(record.clone())),
                        None if self.capabilities.supports(locator) => Resolution::NotFound,
                        None => Resolution::UnsupportedLocator,
                    })
                    .collect()),
            }
        })
    }

    fn refresh_metadata<'a>(
        &'a self,
        requests: &'a [RefreshRequest],
    ) -> ProviderFuture<'a, Vec<RefreshItem>> {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::RefreshMetadata(
                requests
                    .iter()
                    .map(|request| request.provider_id.clone())
                    .collect(),
            ));
        Box::pin(async move {
            requests
                .iter()
                .map(|request| RefreshItem {
                    bibi_id: request.bibi_id,
                    result: Ok(self
                        .metadata
                        .get(&request.provider_id)
                        .cloned()
                        .unwrap_or(RefreshState::Missing)),
                })
                .collect()
        })
    }

    fn fetch_payloads<'a>(
        &'a self,
        requests: &'a [PayloadRequest],
    ) -> ProviderFuture<'a, Result<Vec<PayloadItem>, ProviderError>> {
        self.calls
            .lock()
            .expect("call log")
            .push(ProviderCall::FetchPayloads(
                requests
                    .iter()
                    .map(|request| request.provider_id.clone())
                    .collect(),
            ));
        Box::pin(async move {
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
        })
    }

    fn ingest(&self, _entry: BibtexEntry) -> Result<Resolution, ProviderError> {
        Ok(Resolution::UnsupportedLocator)
    }
}

/// Build a `ProviderOwned` for tests.
pub fn provider_record(
    provider: &str,
    provider_id: &str,
    texkey: &str,
    title: &str,
) -> ProviderOwned {
    ProviderOwned {
        provenance: Provenance::managed(
            ProviderName::new(provider).expect("test provider name"),
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

/// Build `ProviderMetadata` for tests.
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

/// Check the promises every provider owes the registry.
///
/// A provider crate calls this against a hermetically served instance. It
/// asserts shape rather than content: arity and order for `resolve`, one
/// outcome per request for `refresh_metadata`, self-naming provenance, and
/// capability coherence.
pub async fn verify_contract(provider: &dyn Provider, locators: &[Locator]) {
    let name = provider.name().clone();
    assert_eq!(*provider.name(), name, "a provider's name must be stable");

    let capabilities = provider.capabilities();
    assert!(
        capabilities.refresh || capabilities.ingest,
        "a provider that neither refreshes nor ingests can own no record"
    );

    // A call-level error here is not a per-locator outcome: the suite runs
    // against a hermetically served instance, so a conforming provider answers
    // every probe. Letting `Err` slide would certify a provider whose every
    // resolution fails in the app.
    let resolutions = provider.resolve(locators).await.expect(
        "a conforming provider answers probe locators, per locator, without a call-level error",
    );
    assert_eq!(
        resolutions.len(),
        locators.len(),
        "resolve must answer every locator, positionally"
    );
    for (locator, resolution) in locators.iter().zip(&resolutions) {
        match resolution {
            Resolution::Found(record) => {
                assert_eq!(
                    record.provenance.provider, name,
                    "a provider must name itself in the records it returns"
                );
                assert!(
                    !capabilities.refresh || record.provenance.provider_id.is_some(),
                    "a refreshing provider must supply a handle to refresh with"
                );
                assert!(
                    record.provenance.revision.is_none() || record.provenance.provider_id.is_some(),
                    "a revision without a provider id can never be used"
                );
                assert!(
                    !record.description.title.trim().is_empty(),
                    "a record with no title cannot be stored"
                );
            }
            Resolution::UnsupportedLocator => assert!(
                !capabilities.supports(locator),
                "a provider must resolve the locator kinds it declares"
            ),
            Resolution::NotFound => {}
        }
    }

    let requests = [RefreshRequest {
        bibi_id: bibi_core::BibiId::new(),
        provider_id: ProviderId::new("contract-probe").expect("test id"),
        stored_revision: None,
    }];
    let items = provider.refresh_metadata(&requests).await;
    assert_eq!(
        items.len(),
        1,
        "refresh must answer every requested record exactly once"
    );
    assert_eq!(
        items[0].bibi_id, requests[0].bibi_id,
        "refresh results must carry the id they answer for"
    );
    if !capabilities.refresh {
        assert!(
            matches!(items[0].result, Ok(RefreshState::Unrefreshable)),
            "a provider that does not refresh must say so rather than fail"
        );
    }
}

/// Wrap providers into a registry in roster order.
pub fn registry(providers: Vec<Arc<dyn Provider>>) -> crate::registry::ProviderRegistry {
    crate::registry::ProviderRegistry::new(providers)
}
