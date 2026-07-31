//! Provider-neutral remote-provider contract and result values.
//!
//! Concrete provider crates implement [`RemoteProvider`]. Selection,
//! construction, and conditional-refresh policy live above this module.

use crate::{
    BibiId, Description, Identifiers, Locator, Provider, ProviderId, ProviderOwned, Revision,
};
pub use bibi_bibtex::{BibtexEntry, parse_file};
use std::future::Future;
use thiserror::Error as ThisError;

/// A remote metadata and BibTeX provider.
///
/// Every operation is plural because only the implementation knows its API's
/// batching limits. The returned futures are statically dispatched and
/// `Send`; the contract is deliberately not object-safe.
pub trait RemoteProvider: Send + Sync {
    /// The stable name written into record provenance.
    fn name(&self) -> Provider;

    /// Resolve locators, returning exactly one positional result per locator.
    fn resolve(
        &self,
        locators: &[Locator],
    ) -> impl Future<Output = Result<Vec<Resolution>, ProviderError>> + Send;

    /// Fetch complete current metadata for managed records, omitting only
    /// fields that are genuinely absent at the provider.
    fn refresh_metadata(
        &self,
        requests: &[RefreshRequest],
    ) -> impl Future<Output = Vec<RefreshItem>> + Send;

    /// Fetch BibTeX for records whose metadata requires it.
    fn fetch_payloads(
        &self,
        requests: &[PayloadRequest],
    ) -> impl Future<Output = Result<Vec<PayloadItem>, ProviderError>> + Send;
}

/// One record a refresh should examine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    /// Which record this is, so results can be matched back exactly.
    pub bibi_id: BibiId,
    /// The provider's stable handle.
    pub provider_id: ProviderId,
    /// The token stored at the last refresh, if any.
    pub stored_revision: Option<Revision>,
}

/// The provider's complete current metadata: everything but the payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderMetadata {
    /// The provider's handle, as reported now.
    pub provider_id: ProviderId,
    /// The provider's current opaque revision.
    pub revision: Option<Revision>,
    /// Canonical normalized identifiers.
    pub identifiers: Identifiers,
    /// Advisory display data.
    pub description: Description,
    /// Ephemeral provider-private hints for the matching payload request.
    pub join_tokens: Vec<String>,
}

/// One record whose BibTeX should be fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadRequest {
    /// The provider's stable handle.
    pub provider_id: ProviderId,
    /// Join hints echoed from the matching metadata.
    pub join_tokens: Vec<String>,
}

/// What resolving one locator produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    /// A complete mapped record.
    Found(Box<ProviderOwned>),
    /// The provider understands the locator and holds no record for it.
    NotFound,
    /// The provider cannot resolve that locator.
    UnsupportedLocator,
}

/// What examining one managed record produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshState {
    /// The record's complete current provider metadata.
    Metadata(Box<ProviderMetadata>),
    /// The provider no longer holds this record.
    Missing,
}

/// One record's metadata outcome, correlated by bibi id.
#[derive(Debug)]
pub struct RefreshItem {
    /// Which record this answers for.
    pub bibi_id: BibiId,
    /// The outcome, or why it could not be produced.
    pub result: Result<RefreshState, ProviderError>,
}

/// One record's BibTeX, or its unambiguous absence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadItem {
    /// Which provider record this answers for.
    pub provider_id: ProviderId,
    /// The entry, when one arrived.
    pub payload: Option<BibtexEntry>,
}

/// A provider failed to produce a result.
#[derive(Debug, ThisError)]
pub enum ProviderError {
    /// The response could not be obtained. Retrying may help.
    #[error(transparent)]
    Retrieval(#[from] RetrievalError),
    /// The response could not be mapped. Retrying the same content cannot help.
    #[error(transparent)]
    Mapping(#[from] MappingError),
}

/// Why a provider response could not be obtained.
#[derive(Debug, ThisError)]
pub enum RetrievalError {
    /// The request did not complete.
    #[error("{provider}: request failed: {message}")]
    Transport {
        /// The provider that failed.
        provider: Provider,
        /// What went wrong.
        message: String,
    },
    /// The provider returned an unsuccessful status.
    #[error("{provider}: returned HTTP {status}: {body}")]
    Status {
        /// The provider that failed.
        provider: Provider,
        /// HTTP status code.
        status: u16,
        /// Response body retained for diagnostics.
        body: String,
    },
    /// The provider rejected or requires credentials.
    #[error("{provider}: authentication failed: {message}")]
    Authentication {
        /// The provider that failed.
        provider: Provider,
        /// What went wrong.
        message: String,
    },
    /// Retries were exhausted against a rate limit.
    #[error("{provider}: rate limit not cleared after {attempts} retries")]
    RateLimited {
        /// The provider that failed.
        provider: Provider,
        /// Number of retries attempted.
        attempts: usize,
    },
    /// The provider is misconfigured.
    #[error("{provider}: {message}")]
    Configuration {
        /// The provider that failed.
        provider: Provider,
        /// What is missing or wrong.
        message: String,
    },
}

/// Why a response could not be turned into record fields.
#[derive(Debug, ThisError)]
pub enum MappingError {
    /// A required field is absent.
    #[error("{provider}: response is missing `{field}`")]
    MissingField {
        /// The provider that failed.
        provider: Provider,
        /// The absent field.
        field: &'static str,
    },
    /// A field is not a valid identifier.
    #[error("{provider}: `{value}` is not a valid {kind}")]
    InvalidValue {
        /// The provider that failed.
        provider: Provider,
        /// What the value should have been.
        kind: &'static str,
        /// The offending value.
        value: String,
    },
    /// Returned BibTeX is unusable.
    #[error("{provider}: returned unusable BibTeX: {message}")]
    InvalidPayload {
        /// The provider that failed.
        provider: Provider,
        /// What was wrong.
        message: String,
    },
    /// A response cannot be paired with its request.
    #[error("{provider}: could not pair the response with the request: {message}")]
    AmbiguousJoin {
        /// The provider that failed.
        provider: Provider,
        /// What could not be paired.
        message: String,
    },
    /// The implementation broke the neutral contract.
    #[error("{provider}: violated the provider contract: {message}")]
    ContractViolation {
        /// The provider that failed.
        provider: Provider,
        /// Which promise was broken.
        message: String,
    },
}

impl ProviderError {
    /// The provider that produced this failure.
    pub fn provider(&self) -> Provider {
        match self {
            Self::Retrieval(error) => match error {
                RetrievalError::Transport { provider, .. }
                | RetrievalError::Status { provider, .. }
                | RetrievalError::Authentication { provider, .. }
                | RetrievalError::RateLimited { provider, .. }
                | RetrievalError::Configuration { provider, .. } => *provider,
            },
            Self::Mapping(error) => match error {
                MappingError::MissingField { provider, .. }
                | MappingError::InvalidValue { provider, .. }
                | MappingError::InvalidPayload { provider, .. }
                | MappingError::AmbiguousJoin { provider, .. }
                | MappingError::ContractViolation { provider, .. } => *provider,
            },
        }
    }

    /// Construct a contract violation attributed to `provider`.
    pub fn contract(provider: Provider, message: impl Into<String>) -> Self {
        Self::Mapping(MappingError::ContractViolation {
            provider,
            message: message.into(),
        })
    }
}

/// Generic fakes and contract verification for provider crates.
pub mod testing;
