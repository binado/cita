//! Exhaustive dispatch over the providers compiled into this build.

use crate::local;
use bibi_bibtex::BibtexEntry;
pub use bibi_core::Provider;
use bibi_core::{
    BibiId, Locator, Provenance, ProviderId, ProviderOwned, Revision,
    remote::{
        MappingError, PayloadItem, PayloadRequest, ProviderError, ProviderMetadata, RefreshItem,
        RefreshRequest, RefreshState, RemoteProvider, Resolution, RetrievalError,
    },
};
use bibi_inspire::{InspireProvider, RetryEvent, RetryObserver, Transport};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};
use thiserror::Error as ThisError;

/// A usage or construction failure at the closed facade.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The requested provider is not installed.
    #[error(
        "provider `{name}` is not available in this build; installed providers: {}",
        Provider::installed_list()
    )]
    UnknownProvider {
        /// The name supplied.
        name: String,
    },
    /// A qualifier conflicts with the explicit selection.
    #[error("`{locator}` names provider `{qualified}`, but `--provider {requested}` was given")]
    ProviderConflict {
        /// The locator as written.
        locator: String,
        /// Provider named by the locator.
        qualified: Provider,
        /// Provider named by the flag.
        requested: Provider,
    },
    /// One invocation contains multiple provider qualifiers.
    #[error(
        "one add invocation must use one provider, but both `{first}` and `{second}` were named"
    )]
    MixedProviders {
        /// First provider encountered.
        first: Provider,
        /// Conflicting provider encountered.
        second: Provider,
    },
    /// Local ingestion was selected without a BibTeX entry.
    #[error("provider `local` requires `add -f FILE`; it cannot resolve locators")]
    LocalResolution,
    /// A remote operation was requested from a non-remote owner.
    #[error("provider `{provider}` has no remote implementation")]
    NotRemote {
        /// The selected owner.
        provider: Provider,
    },
    /// INSPIRE construction failed.
    #[error(transparent)]
    Construction(#[from] RetrievalError),
}

/// One positional resolution result after facade validation.
#[derive(Debug)]
pub enum ResolveItem {
    /// A complete record.
    Found(Box<ProviderOwned>),
    /// The selected provider holds no record for the locator.
    NotFound,
    /// The selected provider does not support the locator.
    UnsupportedLocator,
    /// The selected provider failed for this item.
    Failed(ProviderError),
}

/// The stored fields a conditional refresh needs.
#[derive(Clone, Debug)]
pub struct RefreshTarget {
    /// Immutable bibi identity.
    pub bibi_id: BibiId,
    /// Stable provider handle.
    pub provider_id: ProviderId,
    /// Stored opaque revision.
    pub stored_revision: Option<Revision>,
}

/// Conditional refresh options.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefreshOptions {
    /// Fetch a payload even when revisions compare equal.
    pub force: bool,
}

/// A complete refresh decision. Narrow metadata never escapes this facade.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    /// Stored revision and current revision compare equal.
    Unchanged,
    /// A complete, validated provider-owned replacement.
    Updated(Box<ProviderOwned>),
    /// The provider no longer holds the record.
    Missing,
    /// Metadata changed but no payload arrived.
    PayloadMissing,
}

/// One correlated conditional-refresh result.
#[derive(Debug)]
pub struct RefreshedItem {
    /// The record this answers for.
    pub bibi_id: BibiId,
    /// Complete outcome, or a provider/contract failure.
    pub result: Result<RefreshOutcome, ProviderError>,
}

enum Backend {
    Inspire(InspireProvider),
    Scripted(Arc<crate::testing::FakeProvider>),
}

impl RemoteProvider for Backend {
    fn name(&self) -> Provider {
        match self {
            Backend::Inspire(remote) => remote.name(),
            Backend::Scripted(remote) => remote.name(),
        }
    }

    async fn resolve(&self, locators: &[Locator]) -> Result<Vec<Resolution>, ProviderError> {
        match self {
            Backend::Inspire(remote) => remote.resolve(locators).await,
            Backend::Scripted(remote) => remote.resolve(locators).await,
        }
    }

    async fn refresh_metadata(&self, requests: &[RefreshRequest]) -> Vec<RefreshItem> {
        match self {
            Backend::Inspire(remote) => remote.refresh_metadata(requests).await,
            Backend::Scripted(remote) => remote.refresh_metadata(requests).await,
        }
    }

    async fn fetch_payloads(
        &self,
        requests: &[PayloadRequest],
    ) -> Result<Vec<PayloadItem>, ProviderError> {
        match self {
            Backend::Inspire(remote) => remote.fetch_payloads(requests).await,
            Backend::Scripted(remote) => remote.fetch_payloads(requests).await,
        }
    }
}

/// Closed provider facade.
pub struct Providers {
    remote: Backend,
}

impl fmt::Debug for Providers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Providers")
            .field("installed", &Provider::ALL)
            .finish_non_exhaustive()
    }
}

impl Providers {
    /// Start constructing the production facade.
    pub fn builder() -> ProvidersBuilder {
        ProvidersBuilder::default()
    }

    /// Construct production providers with default transport settings.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// Parse an exact installed name.
    pub fn installed(value: &str) -> Result<Provider, Error> {
        value.parse().map_err(|_| Error::UnknownProvider {
            name: value.to_owned(),
        })
    }

    /// Resolve all locators through exactly one selected remote provider.
    pub async fn resolve(
        &self,
        selected: Option<Provider>,
        locators: &[Locator],
    ) -> Result<Vec<ResolveItem>, Error> {
        let provider = preflight(selected, locators)?;
        if !provider.is_remote() {
            return Err(Error::LocalResolution);
        }
        let result = self.remote.resolve(locators).await;
        Ok(validate_resolutions(provider, locators.len(), result))
    }

    /// Ingest one entry explicitly as local provenance.
    pub fn ingest_local(&self, entry: BibtexEntry) -> Result<ProviderOwned, ProviderError> {
        local::ingest(entry)
    }

    /// Perform a complete conditional refresh for one installed owner.
    pub async fn refresh(
        &self,
        owner: Provider,
        targets: &[RefreshTarget],
        options: RefreshOptions,
    ) -> Result<Vec<RefreshedItem>, Error> {
        if !owner.is_remote() {
            return Err(Error::NotRemote { provider: owner });
        }
        let requests = targets
            .iter()
            .map(|target| RefreshRequest {
                bibi_id: target.bibi_id,
                provider_id: target.provider_id.clone(),
                stored_revision: target.stored_revision.clone(),
            })
            .collect::<Vec<_>>();
        let metadata = self.remote.refresh_metadata(&requests).await;
        Ok(self.finish_refresh(owner, targets, options, metadata).await)
    }

    async fn finish_refresh(
        &self,
        owner: Provider,
        targets: &[RefreshTarget],
        options: RefreshOptions,
        metadata_items: Vec<RefreshItem>,
    ) -> Vec<RefreshedItem> {
        if !metadata_correlates(targets, &metadata_items) {
            return fail_targets(
                targets,
                ProviderError::contract(
                    owner,
                    format!(
                        "returned {} refresh results for {} requests",
                        metadata_items.len(),
                        targets.len()
                    ),
                ),
            );
        }

        let targets_by_id = targets
            .iter()
            .map(|target| (target.bibi_id, target))
            .collect::<BTreeMap<_, _>>();
        let mut results = BTreeMap::new();
        let mut changed = Vec::new();
        for item in metadata_items {
            let target = targets_by_id[&item.bibi_id];
            match item.result {
                Err(error) => {
                    results.insert(target.bibi_id, Err(error));
                }
                Ok(RefreshState::Missing) => {
                    results.insert(target.bibi_id, Ok(RefreshOutcome::Missing));
                }
                Ok(RefreshState::Metadata(metadata)) => {
                    if metadata.provider_id != target.provider_id {
                        results.insert(
                            target.bibi_id,
                            Err(ProviderError::contract(
                                owner,
                                format!(
                                    "reported provider id `{}` for requested id `{}`",
                                    metadata.provider_id, target.provider_id
                                ),
                            )),
                        );
                    } else if !options.force
                        && target.stored_revision.is_some()
                        && metadata.revision.is_some()
                        && target.stored_revision == metadata.revision
                    {
                        results.insert(target.bibi_id, Ok(RefreshOutcome::Unchanged));
                    } else {
                        changed.push((target, metadata));
                    }
                }
            }
        }

        if !changed.is_empty() {
            let requests = changed
                .iter()
                .map(|(target, metadata)| PayloadRequest {
                    provider_id: target.provider_id.clone(),
                    join_tokens: metadata.join_tokens.clone(),
                })
                .collect::<Vec<_>>();
            let fetched = self.remote.fetch_payloads(&requests).await;
            match fetched {
                Err(error) => {
                    let count = changed.len();
                    for ((target, _), error) in
                        changed.into_iter().zip(broadcast(owner, error, count))
                    {
                        results.insert(target.bibi_id, Err(error));
                    }
                }
                Ok(payloads) if !payloads_correlate(&requests, &payloads) => {
                    let message = format!(
                        "returned {} payload results for {} requests",
                        payloads.len(),
                        requests.len()
                    );
                    for (target, _) in changed {
                        results.insert(
                            target.bibi_id,
                            Err(ProviderError::contract(owner, message.clone())),
                        );
                    }
                }
                Ok(payloads) => {
                    let mut by_id = payloads
                        .into_iter()
                        .map(|item| (item.provider_id, item.payload))
                        .collect::<BTreeMap<_, _>>();
                    for (target, metadata) in changed {
                        let payload = by_id.remove(&target.provider_id).flatten();
                        let outcome = match payload {
                            None => Ok(RefreshOutcome::PayloadMissing),
                            Some(payload) => build_update(owner, &metadata, payload)
                                .map(|owned| RefreshOutcome::Updated(Box::new(owned))),
                        };
                        results.insert(target.bibi_id, outcome);
                    }
                }
            }
        }

        targets
            .iter()
            .map(|target| RefreshedItem {
                bibi_id: target.bibi_id,
                result: results
                    .remove(&target.bibi_id)
                    .expect("every refresh target received an outcome"),
            })
            .collect()
    }
}

/// Production construction options.
#[derive(Default)]
pub struct ProvidersBuilder {
    inspire_base_url: Option<String>,
    retry_observer: Option<RetryObserver>,
}

impl ProvidersBuilder {
    /// Override the INSPIRE base URL.
    pub fn inspire_base_url(mut self, value: impl Into<String>) -> Self {
        self.inspire_base_url = Some(value.into());
        self
    }

    /// Observe INSPIRE rate-limit retries.
    pub fn on_retry(mut self, observer: impl Fn(&RetryEvent) + Send + Sync + 'static) -> Self {
        self.retry_observer = Some(Arc::new(observer));
        self
    }

    /// Build the closed facade.
    pub fn build(self) -> Result<Providers, Error> {
        let mut transport = Transport::builder();
        if let Some(base_url) = self.inspire_base_url {
            transport = transport.base_url(base_url);
        }
        if let Some(observer) = self.retry_observer {
            transport = transport.on_retry(move |event| observer(event));
        }
        Ok(Providers {
            remote: Backend::Inspire(InspireProvider::with_transport(transport.build()?)),
        })
    }
}

fn preflight(selected: Option<Provider>, locators: &[Locator]) -> Result<Provider, Error> {
    let mut qualified = None;
    for locator in locators {
        let Locator::ProviderIdentity(provider, _) = locator else {
            continue;
        };
        let provider = *provider;
        if let Some(requested) = selected
            && requested != provider
        {
            return Err(Error::ProviderConflict {
                locator: locator.to_string(),
                qualified: provider,
                requested,
            });
        }
        if let Some(first) = qualified
            && first != provider
        {
            return Err(Error::MixedProviders {
                first,
                second: provider,
            });
        }
        qualified = Some(provider);
    }
    Ok(selected.or(qualified).unwrap_or_default())
}

fn validate_resolutions(
    owner: Provider,
    expected: usize,
    result: Result<Vec<Resolution>, ProviderError>,
) -> Vec<ResolveItem> {
    match result {
        Err(error) => broadcast(owner, error, expected)
            .map(ResolveItem::Failed)
            .collect(),
        Ok(items) if items.len() != expected => (0..expected)
            .map(|_| {
                ResolveItem::Failed(ProviderError::contract(
                    owner,
                    format!(
                        "returned {} resolutions for {} locators",
                        items.len(),
                        expected
                    ),
                ))
            })
            .collect(),
        Ok(items) => items
            .into_iter()
            .map(|item| match item {
                Resolution::NotFound => ResolveItem::NotFound,
                Resolution::UnsupportedLocator => ResolveItem::UnsupportedLocator,
                Resolution::Found(record) => match validate_record(owner, &record) {
                    Ok(()) => ResolveItem::Found(record),
                    Err(error) => ResolveItem::Failed(error),
                },
            })
            .collect(),
    }
}

fn validate_record(owner: Provider, record: &ProviderOwned) -> Result<(), ProviderError> {
    if record.provenance.provider != owner {
        return Err(ProviderError::contract(
            owner,
            format!(
                "returned a record owned by `{}`",
                record.provenance.provider
            ),
        ));
    }
    if record.provenance.provider_id.is_none() {
        return Err(ProviderError::contract(
            owner,
            "returned a remote record without a provider id",
        ));
    }
    if record.description.title.trim().is_empty() {
        return Err(ProviderError::contract(
            owner,
            "returned a record without a title",
        ));
    }
    Ok(())
}

fn metadata_correlates(targets: &[RefreshTarget], items: &[RefreshItem]) -> bool {
    if targets.len() != items.len() {
        return false;
    }
    let expected = targets
        .iter()
        .map(|target| target.bibi_id)
        .collect::<BTreeSet<_>>();
    let returned = items
        .iter()
        .map(|item| item.bibi_id)
        .collect::<BTreeSet<_>>();
    expected.len() == targets.len() && expected == returned
}

fn payloads_correlate(requests: &[PayloadRequest], items: &[PayloadItem]) -> bool {
    if requests.len() != items.len() {
        return false;
    }
    let expected = requests
        .iter()
        .map(|request| &request.provider_id)
        .collect::<BTreeSet<_>>();
    let returned = items
        .iter()
        .map(|item| &item.provider_id)
        .collect::<BTreeSet<_>>();
    expected.len() == requests.len() && expected == returned
}

fn build_update(
    owner: Provider,
    metadata: &ProviderMetadata,
    payload: BibtexEntry,
) -> Result<ProviderOwned, ProviderError> {
    if metadata.description.title.trim().is_empty() {
        return Err(ProviderError::contract(
            owner,
            "returned refresh metadata without a title",
        ));
    }
    Ok(ProviderOwned {
        provenance: Provenance::managed(
            owner,
            metadata.provider_id.clone(),
            metadata.revision.clone(),
        ),
        identifiers: metadata.identifiers.clone(),
        description: metadata.description.clone(),
        payload,
    })
}

fn fail_targets(targets: &[RefreshTarget], error: ProviderError) -> Vec<RefreshedItem> {
    let provider = error.provider();
    broadcast(provider, error, targets.len())
        .zip(targets)
        .map(|(error, target)| RefreshedItem {
            bibi_id: target.bibi_id,
            result: Err(error),
        })
        .collect()
}

/// Produce `count` copies of `error`. `ProviderError` is not `Clone`, so the
/// original is returned first and the rest are reconstructed from its
/// rendered message.
fn broadcast(
    provider: Provider,
    error: ProviderError,
    count: usize,
) -> impl Iterator<Item = ProviderError> {
    let rendered = error.to_string();
    let retrieval = matches!(error, ProviderError::Retrieval(_));
    let mut first = Some(error);
    (0..count).map(move |_| {
        first
            .take()
            .unwrap_or_else(|| restated(provider, &rendered, retrieval))
    })
}

fn restated(provider: Provider, message: &str, retrieval: bool) -> ProviderError {
    if retrieval {
        RetrievalError::Transport {
            provider,
            message: message.to_owned(),
        }
        .into()
    } else {
        MappingError::ContractViolation {
            provider,
            message: message.to_owned(),
        }
        .into()
    }
}

impl Providers {
    pub(crate) fn scripted(remote: Arc<crate::testing::FakeProvider>) -> Self {
        Self {
            remote: Backend::Scripted(remote),
        }
    }
}
