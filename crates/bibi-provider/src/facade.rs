//! Exhaustive dispatch over the providers compiled into this build.

use crate::local;
use bibi_bibtex::BibtexEntry;
use bibi_core::{
    BibiId, IdentifierChange, Identifiers, Provenance, ProviderId, ProviderName, ProviderOwned,
    QualifiedLocator, Revision,
    provider::{
        MappingError, PayloadItem, PayloadRequest, ProviderError, ProviderMetadata, RefreshItem,
        RefreshRequest, RefreshState, RemoteProvider, Resolution, RetrievalError,
    },
};
use bibi_inspire::{InspireProvider, RetryEvent, RetryObserver, Transport};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
    sync::Arc,
};
use thiserror::Error as ThisError;

/// An installed provider owner.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Provider {
    /// The INSPIRE literature service.
    #[default]
    Inspire,
    /// User-supplied BibTeX, stored without a remote handle.
    Local,
}

impl Provider {
    /// Every provider installed in this build.
    pub const ALL: [Self; 2] = [Self::Inspire, Self::Local];

    /// The exact manifest and command-line name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspire => "inspire",
            Self::Local => local::LOCAL_PROVIDER,
        }
    }

    /// Convert to the open-ended manifest provenance type.
    pub fn name(self) -> ProviderName {
        ProviderName::new(self.as_str()).expect("installed provider names are valid")
    }

    /// Whether this owner has a remote implementation.
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Inspire)
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "inspire" => Ok(Self::Inspire),
            "local" => Ok(Self::Local),
            _ => Err(Error::UnknownProvider {
                name: value.to_owned(),
            }),
        }
    }
}

/// A usage or construction failure at the closed facade.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The requested provider is not installed.
    #[error("provider `{name}` is not available in this build")]
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
    /// Stored identifiers, used to validate additions versus replacements.
    pub identifiers: Identifiers,
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

/// Closed provider facade.
pub struct Providers {
    remote: Backend,
}

impl fmt::Debug for Providers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Providers")
            .field("installed", &Self::installed_names())
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

    /// Names installed in this build, in stable display order.
    pub fn installed_names() -> Vec<ProviderName> {
        Provider::ALL.into_iter().map(Provider::name).collect()
    }

    /// Parse an exact installed name.
    pub fn installed(value: &str) -> Result<Provider, Error> {
        value.parse()
    }

    /// Whether an open-ended manifest owner is installed.
    pub fn contains(&self, name: &ProviderName) -> bool {
        Provider::ALL
            .into_iter()
            .any(|provider| provider.as_str() == name.as_str())
    }

    /// Convert a stored installed owner into closed selection.
    pub fn owner(&self, name: &ProviderName) -> Option<Provider> {
        name.as_str().parse().ok()
    }

    /// Resolve all locators through exactly one selected remote provider.
    pub async fn resolve(
        &self,
        selected: Option<Provider>,
        locators: &[QualifiedLocator],
    ) -> Result<Vec<ResolveItem>, Error> {
        let provider = preflight(selected, locators)?;
        if provider == Provider::Local {
            return Err(Error::LocalResolution);
        }
        let requested = locators
            .iter()
            .map(|locator| locator.locator.clone())
            .collect::<Vec<_>>();
        let result = match &self.remote {
            Backend::Inspire(remote) => remote.resolve(&requested).await,
            Backend::Scripted(remote) => remote.resolve(&requested).await,
        };
        Ok(validate_resolutions(provider, requested.len(), result))
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
        let metadata = match &self.remote {
            Backend::Inspire(remote) => remote.refresh_metadata(&requests).await,
            Backend::Scripted(remote) => remote.refresh_metadata(&requests).await,
        };
        Ok(self.finish_refresh(owner, targets, options, metadata).await)
    }

    async fn finish_refresh(
        &self,
        owner: Provider,
        targets: &[RefreshTarget],
        options: RefreshOptions,
        metadata_items: Vec<RefreshItem>,
    ) -> Vec<RefreshedItem> {
        let provider_name = owner.name();
        if !metadata_correlates(targets, &metadata_items) {
            return fail_targets(
                targets,
                ProviderError::contract(
                    &provider_name,
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
                                &provider_name,
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
            let fetched = match &self.remote {
                Backend::Inspire(remote) => remote.fetch_payloads(&requests).await,
                Backend::Scripted(remote) => remote.fetch_payloads(&requests).await,
            };
            match fetched {
                Err(error) => {
                    let rendered = error.to_string();
                    let retrieval = matches!(error, ProviderError::Retrieval(_));
                    let mut first = Some(error);
                    for (target, _) in changed {
                        results.insert(
                            target.bibi_id,
                            Err(first
                                .take()
                                .unwrap_or_else(|| restated(&provider_name, &rendered, retrieval))),
                        );
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
                            Err(ProviderError::contract(&provider_name, message.clone())),
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
                            Some(payload) => build_update(owner, target, &metadata, payload)
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

fn preflight(selected: Option<Provider>, locators: &[QualifiedLocator]) -> Result<Provider, Error> {
    let mut qualified = None;
    for locator in locators {
        let Some(name) = &locator.provider else {
            continue;
        };
        let provider = Provider::from_str(name.as_str())?;
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
    let provider_name = owner.name();
    match result {
        Err(error) => {
            let rendered = error.to_string();
            let retrieval = matches!(error, ProviderError::Retrieval(_));
            let mut first = Some(error);
            (0..expected)
                .map(|_| {
                    ResolveItem::Failed(
                        first
                            .take()
                            .unwrap_or_else(|| restated(&provider_name, &rendered, retrieval)),
                    )
                })
                .collect()
        }
        Ok(items) if items.len() != expected => (0..expected)
            .map(|_| {
                ResolveItem::Failed(ProviderError::contract(
                    &provider_name,
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
    let expected = owner.name();
    if record.provenance.provider != expected {
        return Err(ProviderError::contract(
            &expected,
            format!(
                "returned a record owned by `{}`",
                record.provenance.provider
            ),
        ));
    }
    if record.provenance.provider_id.is_none() {
        return Err(ProviderError::contract(
            &expected,
            "returned a remote record without a provider id",
        ));
    }
    if record.description.title.trim().is_empty() {
        return Err(ProviderError::contract(
            &expected,
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
        .map(|request| request.provider_id.clone())
        .collect::<BTreeSet<_>>();
    let returned = items
        .iter()
        .map(|item| item.provider_id.clone())
        .collect::<BTreeSet<_>>();
    expected.len() == requests.len() && expected == returned
}

fn build_update(
    owner: Provider,
    target: &RefreshTarget,
    metadata: &ProviderMetadata,
    payload: BibtexEntry,
) -> Result<ProviderOwned, ProviderError> {
    if metadata.description.title.trim().is_empty() {
        return Err(ProviderError::contract(
            &owner.name(),
            "returned refresh metadata without a title",
        ));
    }
    let doi = IdentifierChange::classify(
        target.identifiers.doi.as_ref(),
        metadata.identifiers.doi.as_ref(),
    );
    let arxiv = IdentifierChange::classify(
        target.identifiers.arxiv.as_ref(),
        metadata.identifiers.arxiv.as_ref(),
    );
    if let Some(message) = replacement(&doi, "DOI").or_else(|| replacement(&arxiv, "arXiv id")) {
        return Err(ProviderError::contract(&owner.name(), message));
    }
    Ok(ProviderOwned {
        provenance: Provenance::managed(
            owner.name(),
            metadata.provider_id.clone(),
            metadata.revision.clone(),
        ),
        identifiers: Identifiers {
            doi: metadata
                .identifiers
                .doi
                .clone()
                .or(target.identifiers.doi.clone()),
            arxiv: metadata
                .identifiers
                .arxiv
                .clone()
                .or(target.identifiers.arxiv.clone()),
        },
        description: metadata.description.clone(),
        payload,
    })
}

fn replacement<T: fmt::Display>(
    change: &IdentifierChange<T>,
    kind: &'static str,
) -> Option<String> {
    match change {
        IdentifierChange::Replaced { old, new } => Some(format!(
            "provider reports {kind} `{new}` where `{old}` is stored; identifiers do not change once set, so repair this with `add --overwrite`"
        )),
        _ => None,
    }
}

fn fail_targets(targets: &[RefreshTarget], error: ProviderError) -> Vec<RefreshedItem> {
    let name = error.provider().clone();
    let rendered = error.to_string();
    let retrieval = matches!(error, ProviderError::Retrieval(_));
    let mut first = Some(error);
    targets
        .iter()
        .map(|target| RefreshedItem {
            bibi_id: target.bibi_id,
            result: Err(first
                .take()
                .unwrap_or_else(|| restated(&name, &rendered, retrieval))),
        })
        .collect()
}

fn restated(provider: &ProviderName, message: &str, retrieval: bool) -> ProviderError {
    if retrieval {
        RetrievalError::Transport {
            provider: provider.clone(),
            message: message.to_owned(),
        }
        .into()
    } else {
        MappingError::ContractViolation {
            provider: provider.clone(),
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
