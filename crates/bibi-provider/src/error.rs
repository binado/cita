//! Provider failures, split by whether retrying could help.

use bibi_core::ProviderName;
use thiserror::Error as ThisError;

/// A provider failed to produce a result.
///
/// Not-found and unsupported-locator are outcomes rather than errors: they are
/// answers about the world, and they permit fallback to the next provider. The
/// two variants here are answers about the *attempt*, and they stop fallback,
/// because a temporary outage must never silently determine which provider
/// permanently owns a record's BibTeX and texkey.
#[derive(Debug, ThisError)]
pub enum ProviderError {
    /// The provider's response could not be obtained. Retrying may help.
    #[error(transparent)]
    Retrieval(#[from] RetrievalError),
    /// The response arrived but could not be turned into record fields.
    /// Retrying cannot help: the same content will fail the same way.
    #[error(transparent)]
    Mapping(#[from] MappingError),
}

/// Why a provider's response could not be obtained.
#[derive(Debug, ThisError)]
pub enum RetrievalError {
    /// The request did not complete.
    #[error("{provider}: request failed: {message}")]
    Transport {
        /// The provider that failed.
        provider: ProviderName,
        /// What went wrong.
        message: String,
    },
    /// The provider returned an unsuccessful status.
    #[error("{provider}: returned HTTP {status}: {body}")]
    Status {
        /// The provider that failed.
        provider: ProviderName,
        /// The HTTP status code.
        status: u16,
        /// The response body, retained for diagnostics.
        body: String,
    },
    /// The provider rejected or requires credentials.
    #[error("{provider}: authentication failed: {message}")]
    Authentication {
        /// The provider that failed.
        provider: ProviderName,
        /// What went wrong.
        message: String,
    },
    /// Retries were exhausted against a rate limit.
    #[error("{provider}: rate limit not cleared after {attempts} retries")]
    RateLimited {
        /// The provider that failed.
        provider: ProviderName,
        /// How many retries were attempted.
        attempts: usize,
    },
    /// The provider is misconfigured, typically a missing credential.
    ///
    /// Distinct from not-found on purpose: a missing API token says nothing
    /// about whether a work exists, so it must not trigger fallback or become
    /// a local record.
    #[error("{provider}: {message}")]
    Configuration {
        /// The provider that failed.
        provider: ProviderName,
        /// What is missing or wrong.
        message: String,
    },
}

/// Why a response could not be turned into record fields.
#[derive(Debug, ThisError)]
pub enum MappingError {
    /// A field the record model requires is absent or empty.
    #[error("{provider}: response is missing `{field}`")]
    MissingField {
        /// The provider that failed.
        provider: ProviderName,
        /// The absent field.
        field: &'static str,
    },
    /// A value that should be an identifier is not one.
    #[error("{provider}: `{value}` is not a valid {kind}")]
    InvalidValue {
        /// The provider that failed.
        provider: ProviderName,
        /// What the value should have been.
        kind: &'static str,
        /// The offending value.
        value: String,
    },
    /// The provider's BibTeX is not one well-formed standalone entry.
    #[error("{provider}: returned unusable BibTeX: {message}")]
    InvalidPayload {
        /// The provider that failed.
        provider: ProviderName,
        /// What was wrong with it.
        message: String,
    },
    /// The response cannot be paired with what was requested.
    ///
    /// This is the ambiguous-join case: an entry that matches no requested
    /// record, or one texkey claimed by two records. No pairing in such a
    /// response is trustworthy, so none is kept.
    #[error("{provider}: could not pair the response with the request: {message}")]
    AmbiguousJoin {
        /// The provider that failed.
        provider: ProviderName,
        /// What could not be paired.
        message: String,
    },
    /// The provider broke the contract it is dispatched behind.
    #[error("{provider}: violated the provider contract: {message}")]
    ContractViolation {
        /// The offending provider.
        provider: ProviderName,
        /// Which promise was broken.
        message: String,
    },
}

impl ProviderError {
    /// The provider that produced this failure.
    pub fn provider(&self) -> &ProviderName {
        match self {
            Self::Retrieval(error) => match error {
                RetrievalError::Transport { provider, .. }
                | RetrievalError::Status { provider, .. }
                | RetrievalError::Authentication { provider, .. }
                | RetrievalError::RateLimited { provider, .. }
                | RetrievalError::Configuration { provider, .. } => provider,
            },
            Self::Mapping(error) => match error {
                MappingError::MissingField { provider, .. }
                | MappingError::InvalidValue { provider, .. }
                | MappingError::InvalidPayload { provider, .. }
                | MappingError::AmbiguousJoin { provider, .. }
                | MappingError::ContractViolation { provider, .. } => provider,
            },
        }
    }

    /// A contract violation by `provider`.
    pub fn contract(provider: &ProviderName, message: impl Into<String>) -> Self {
        Self::Mapping(MappingError::ContractViolation {
            provider: provider.clone(),
            message: message.into(),
        })
    }
}
