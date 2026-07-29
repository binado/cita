//! Failures, stamped with the provider that produced them.

use bibi_core::ProviderName;
use bibi_provider::{MappingError, RetrievalError};

/// The name INSPIRE records are stored under.
pub const INSPIRE_PROVIDER: &str = "inspire";

/// The provider name, constructed once.
pub(crate) fn provider() -> ProviderName {
    ProviderName::new(INSPIRE_PROVIDER).expect("the INSPIRE provider name is valid")
}

pub(crate) fn transport(message: impl std::fmt::Display) -> RetrievalError {
    RetrievalError::Transport {
        provider: provider(),
        message: message.to_string(),
    }
}

pub(crate) fn status(status: u16, body: String) -> RetrievalError {
    RetrievalError::Status {
        provider: provider(),
        status,
        body,
    }
}

pub(crate) fn rate_limited(attempts: usize) -> RetrievalError {
    RetrievalError::RateLimited {
        provider: provider(),
        attempts,
    }
}

pub(crate) fn configuration(message: impl std::fmt::Display) -> RetrievalError {
    RetrievalError::Configuration {
        provider: provider(),
        message: message.to_string(),
    }
}

pub(crate) fn missing_field(field: &'static str) -> MappingError {
    MappingError::MissingField {
        provider: provider(),
        field,
    }
}

pub(crate) fn invalid_value(kind: &'static str, value: impl Into<String>) -> MappingError {
    MappingError::InvalidValue {
        provider: provider(),
        kind,
        value: value.into(),
    }
}

pub(crate) fn invalid_payload(message: impl std::fmt::Display) -> MappingError {
    MappingError::InvalidPayload {
        provider: provider(),
        message: message.to_string(),
    }
}

pub(crate) fn ambiguous_join(message: impl std::fmt::Display) -> MappingError {
    MappingError::AmbiguousJoin {
        provider: provider(),
        message: message.to_string(),
    }
}
