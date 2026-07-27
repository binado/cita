use thiserror::Error;

/// The source-neutral semantic view used by commands and identity checks.
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
}

/// A source snapshot that can produce cita's deliberately trimmed semantic view.
pub trait ReferenceSource {
    /// Project the authoritative source into a reference.
    fn project(&self) -> Result<Reference, ProjectionError>;
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
