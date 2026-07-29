//! The object-safe provider contract.

use crate::{
    error::ProviderError,
    outcome::{PayloadItem, RefreshItem, RefreshRequest, Resolution},
};
use bibi_bibtex::BibtexEntry;
use bibi_core::{Locator, ProviderId, ProviderName};
use std::{future::Future, pin::Pin};

/// A boxed future, so heterogeneous providers can live behind one trait object.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a provider can do.
///
/// Commands branch on capability, never on a provider's name. That is what
/// keeps the local provider from being a special case: it is an ordinary
/// provider whose capabilities happen to be "ingest, never refresh", and the
/// rest of bibi asks about capabilities rather than testing for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCapabilities {
    /// Resolves arXiv identifiers.
    pub arxiv: bool,
    /// Resolves DOIs.
    pub doi: bool,
    /// Resolves its own record ids.
    pub provider_id: bool,
    /// Refreshes the records it owns.
    pub refresh: bool,
    /// Turns user-supplied BibTeX into a record.
    pub ingest: bool,
}

impl ProviderCapabilities {
    /// A provider that resolves every neutral identifier and refreshes.
    pub const NETWORK: Self = Self {
        arxiv: true,
        doi: true,
        provider_id: true,
        refresh: true,
        ingest: false,
    };

    /// A provider that only ingests what the user supplies.
    pub const INGEST_ONLY: Self = Self {
        arxiv: false,
        doi: false,
        provider_id: false,
        refresh: false,
        ingest: true,
    };

    /// True when this provider can be asked to resolve `locator`.
    pub fn supports(&self, locator: &Locator) -> bool {
        match locator {
            Locator::Arxiv(_) => self.arxiv,
            Locator::Doi(_) => self.doi,
            Locator::ProviderId(_) => self.provider_id,
        }
    }

    /// True when this provider ingests records it will never refresh.
    ///
    /// This is what `list --local` asks about. It is a question about
    /// capability, which is why no command has to know the name `local`.
    pub fn ingest_without_refresh(&self) -> bool {
        self.ingest && !self.refresh
    }
}

/// Everything bibi asks of a metadata provider.
///
/// **Every entry point is plural.** Both resolution and payload retrieval are
/// driven by operations that routinely carry hundreds of items — importing a
/// colleague's bibliography, forcing a refresh over a whole project — and a
/// provider is the only party that knows how its own API batches. A provider
/// that cannot batch loops internally; batching above this trait would push
/// provider-specific limits into the application layer.
pub trait Provider: Send + Sync {
    /// The name this provider is stored under and addressed by.
    fn name(&self) -> &ProviderName;

    /// What this provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// True when `value` is unambiguously this provider's own id syntax.
    ///
    /// Used only to place an *unqualified* bare id. If two providers claim one
    /// value the registry refuses rather than guessing, because a wrong guess
    /// permanently selects that provider's BibTeX.
    fn recognizes_unqualified_id(&self, value: &str) -> bool;

    /// Resolve locators, returning one outcome per input, in input order.
    fn resolve<'a>(
        &'a self,
        locators: &'a [Locator],
    ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>>;

    /// Examine managed records, returning one outcome per request.
    ///
    /// Failures are per item rather than per call, because one unmappable
    /// record must not stop a refresh of everything else.
    fn refresh_metadata<'a>(
        &'a self,
        requests: &'a [RefreshRequest],
    ) -> ProviderFuture<'a, Vec<RefreshItem>>;

    /// Fetch BibTeX for records whose metadata changed.
    fn fetch_payloads<'a>(
        &'a self,
        provider_ids: &'a [ProviderId],
    ) -> ProviderFuture<'a, Result<Vec<PayloadItem>, ProviderError>>;

    /// Turn user-supplied BibTeX into a record of this provider's own.
    ///
    /// Network providers return [`Resolution::UnsupportedLocator`]: they have
    /// nothing to say about an entry the user wrote.
    fn ingest(&self, entry: BibtexEntry) -> Result<Resolution, ProviderError>;
}
