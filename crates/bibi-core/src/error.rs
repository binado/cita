use crate::provenance::Provider;
use thiserror::Error as ThisError;

fn installed_providers() -> String {
    Provider::ALL
        .iter()
        .map(|provider| provider.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every failure this crate can produce.
#[derive(Clone, Debug, Eq, PartialEq, ThisError)]
pub enum Error {
    /// A record id is not a canonical UUID.
    #[error("invalid bibi record id `{value}`; expected a canonical UUID")]
    InvalidBibiId {
        /// The rejected value.
        value: String,
    },
    /// A provider name is not one this build carries.
    #[error(
        "unknown provider `{value}`; installed providers: {}",
        installed_providers()
    )]
    UnknownProvider {
        /// The rejected value.
        value: String,
    },
    /// A provider id or revision is empty.
    #[error("a provider {what} must not be empty")]
    EmptyOpaqueValue {
        /// Which opaque value was empty.
        what: &'static str,
    },
    /// A DOI is not a syntactically valid DOI.
    #[error("invalid DOI `{value}`; expected `10.<registrant>/<suffix>`")]
    InvalidDoi {
        /// The rejected value.
        value: String,
    },
    /// An arXiv identifier is neither a modern nor a legacy identifier.
    #[error("invalid arXiv identifier `{value}`; expected `2401.00001` or `hep-th/9901001`")]
    InvalidArxivId {
        /// The rejected value.
        value: String,
    },
    /// A locator string is not a supported locator.
    #[error(
        "invalid locator `{value}`; use an arXiv id, a DOI, an explicit `arxiv:`, `doi:`, or `<provider>:` locator, or a canonical URL"
    )]
    InvalidLocator {
        /// The rejected value.
        value: String,
    },
    /// A selector string denotes nothing bibi can look up.
    #[error(
        "invalid selector `{value}`; expected a citation key, DOI, arXiv id, or `<provider>:<id>`"
    )]
    InvalidSelector {
        /// The rejected value.
        value: String,
    },
    /// A record was constructed without a title.
    #[error("record `{key}` has no title")]
    MissingTitle {
        /// The record's local key.
        key: String,
    },
    /// A record carries a revision but no provider id to refresh with.
    #[error("record `{key}` carries a revision without a provider id")]
    RevisionWithoutProviderId {
        /// The record's local key.
        key: String,
    },
    /// A record's payload cannot carry its local key.
    #[error("record `{key}` cannot be re-keyed: {source}")]
    Payload {
        /// The record's local key.
        key: String,
        /// The underlying BibTeX failure.
        #[source]
        source: bibi_bibtex::Error,
    },
}
