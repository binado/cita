//! Provider interface and typed external failures.

use crate::{Locator, ProviderName, RecordState};
use std::future::Future;
use thiserror::Error as ThisError;

/// Resolve complete current record state through one external provider.
pub trait Provider: Send + Sync {
    /// Stable provider name.
    fn name(&self) -> ProviderName;

    /// Resolve exactly one complete positional state per locator.
    fn resolve(
        &self,
        locators: &[Locator],
    ) -> impl Future<Output = Result<Vec<RecordState>, ProviderError>> + Send;
}

/// A provider failed to produce a complete strict batch.
#[derive(Debug, ThisError)]
#[allow(missing_docs)]
pub enum ProviderError {
    /// A locator kind is unsupported.
    #[error("{provider}: unsupported locator `{locator}`")]
    Unsupported {
        provider: ProviderName,
        locator: String,
    },
    /// No record exists for a locator.
    #[error("{provider}: no record found for `{locator}`")]
    NotFound {
        provider: ProviderName,
        locator: String,
    },
    /// Retrieval failed.
    #[error(transparent)]
    Retrieval(#[from] RetrievalError),
    /// Mapping failed.
    #[error(transparent)]
    Mapping(#[from] MappingError),
}

/// Why a provider response could not be obtained.
#[derive(Clone, Debug, Eq, PartialEq, ThisError)]
#[allow(missing_docs)]
pub enum RetrievalError {
    #[error("{provider}: request failed: {message}")]
    Transport {
        provider: ProviderName,
        message: String,
    },
    #[error("{provider}: returned HTTP {status}: {body}")]
    Status {
        provider: ProviderName,
        status: u16,
        body: String,
    },
    #[error("{provider}: authentication failed: {message}")]
    Authentication {
        provider: ProviderName,
        message: String,
    },
    #[error("{provider}: rate limit not cleared after {attempts} retries")]
    RateLimited {
        provider: ProviderName,
        attempts: usize,
    },
    #[error("{provider}: {message}")]
    Configuration {
        provider: ProviderName,
        message: String,
    },
}

/// Why external data could not become complete state.
#[derive(Clone, Debug, Eq, PartialEq, ThisError)]
#[allow(missing_docs)]
pub enum MappingError {
    #[error("{provider}: response is missing `{field}`")]
    MissingField {
        provider: ProviderName,
        field: &'static str,
    },
    #[error("{provider}: `{value}` is not a valid {kind}")]
    InvalidValue {
        provider: ProviderName,
        kind: &'static str,
        value: String,
    },
    #[error("{provider}: returned unusable BibTeX: {message}")]
    InvalidPayload {
        provider: ProviderName,
        message: String,
    },
    #[error("{provider}: could not pair response with request: {message}")]
    AmbiguousJoin {
        provider: ProviderName,
        message: String,
    },
    #[error("{provider}: violated the provider contract: {message}")]
    ContractViolation {
        provider: ProviderName,
        message: String,
    },
}

impl ProviderError {
    /// Provider attributed to the error.
    pub fn provider(&self) -> ProviderName {
        match self {
            Self::Unsupported { provider, .. } | Self::NotFound { provider, .. } => *provider,
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

    /// Construct a contract failure.
    pub fn contract(provider: ProviderName, message: impl Into<String>) -> Self {
        Self::Mapping(MappingError::ContractViolation {
            provider,
            message: message.into(),
        })
    }
}

/// Provider test support.
pub mod testing;
