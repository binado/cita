use crate::Locator;
use std::{collections::BTreeMap, future::Future};
use thiserror::Error;

/// The provider-neutral semantic view used by commands and identity checks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reference {
    /// Display title.
    pub title: String,
    /// Individual author names in source order.
    pub authors: Vec<String>,
    /// Collaboration names in source order.
    pub collaborations: Vec<String>,
    /// Publication year, when present.
    pub year: Option<i32>,
    /// Normalized identifiers used for lookup and deduplication.
    pub identifiers: Identifiers,
}

/// Source-neutral identifiers associated with a reference.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identifiers {
    /// Normalized DOI values, in whatever order the producing source stored.
    pub dois: Vec<String>,
    /// Normalized, versionless arXiv identifiers.
    pub arxiv: Vec<String>,
    /// Namespaced provider identities, e.g. `inspire = ["1124337"]`.
    pub providers: BTreeMap<String, Vec<String>>,
}

/// A source snapshot that can produce bibi's deliberately trimmed semantic view.
pub trait ReferenceSource {
    /// Project the authoritative source into a reference.
    fn project(&self) -> Result<Reference, ProjectionError>;
}

/// Provider contract. Snapshots remain provider-owned; callers consume their
/// neutral projection and store the complete snapshot separately.
pub trait MetadataProvider: Send + Sync {
    /// The authoritative snapshot returned by this provider.
    type Snapshot: ReferenceSource + Clone + Send + Sync;

    /// Resolve one locator to a provider-owned snapshot.
    fn resolve(
        &self,
        locator: &Locator,
    ) -> impl Future<Output = Result<Self::Snapshot, ProviderError>> + Send;

    /// Refresh snapshots by stable provider identifier.
    fn refresh(
        &self,
        provider_ids: &[String],
    ) -> impl Future<Output = Result<Vec<Self::Snapshot>, ProviderError>> + Send;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
/// Failure to project an authoritative source into a reference.
pub enum ProjectionError {
    /// The source does not contain a title.
    #[error("source has no title")]
    MissingTitle,
    /// Other source metadata is malformed or inconsistent.
    #[error("invalid source metadata: {0}")]
    Invalid(String),
}

#[derive(Debug, Error)]
/// Failure while resolving or refreshing provider metadata.
pub enum ProviderError {
    /// The provider does not accept the locator.
    #[error("invalid locator: {0}")]
    InvalidLocator(String),
    /// The provider found no matching record.
    #[error("no record found for {0}")]
    NotFound(String),
    /// The provider request could not be completed.
    #[error("metadata provider request failed: {0}")]
    Request(String),
    /// The provider response could not be interpreted.
    #[error("metadata provider returned malformed data: {0}")]
    Malformed(String),
}
