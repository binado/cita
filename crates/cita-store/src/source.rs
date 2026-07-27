use cita_bibliography::{BibtexSnapshot, project_bibtex};
use cita_core::{ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi};
use cita_inspire_client::InspireSnapshot;
use serde::{Deserialize, Serialize};

/// A stored reference tagged by the source that owns its refresh lifecycle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceSnapshot {
    /// A refreshable INSPIRE-managed snapshot.
    Inspire(InspireEntry),
    /// A source-preserving generic BibTeX import.
    Import(BibtexSnapshot),
}

/// An INSPIRE-managed reference and its refresh bookkeeping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireEntry {
    /// Stable INSPIRE record identifier.
    pub record_id: u64,
    /// Provider update timestamp.
    pub updated: String,
    /// Complete authoritative standalone BibTeX entry.
    pub bibtex: String,
    /// Canonical identifiers selected by INSPIRE.
    #[serde(default, skip_serializing_if = "HepIdentifiers::is_empty")]
    pub identifiers: HepIdentifiers,
}

/// Curated canonical HEP identifiers cross-checked against the BibTeX.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HepIdentifiers {
    /// Canonical normalized, versionless arXiv identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arxiv: Option<String>,
    /// Canonical normalized DOI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
}

impl HepIdentifiers {
    /// Normalize and construct canonical identifiers.
    pub fn new(arxiv: Option<String>, doi: Option<String>) -> Self {
        Self {
            arxiv: arxiv.as_deref().map(normalize_arxiv),
            doi: doi.as_deref().map(normalize_doi),
        }
    }

    /// Return whether neither canonical identifier is present.
    pub fn is_empty(&self) -> bool {
        self.arxiv.is_none() && self.doi.is_none()
    }
}

impl SourceSnapshot {
    /// Convert a durable provider result into a stored snapshot.
    pub fn inspire(record: InspireSnapshot) -> Self {
        Self::Inspire(InspireEntry {
            record_id: record.record_id,
            updated: record.updated,
            bibtex: record.bibtex,
            identifiers: HepIdentifiers::new(record.arxiv, record.doi),
        })
    }

    /// Return the exact authoritative BibTeX.
    pub fn raw_bibtex(&self) -> &str {
        match self {
            Self::Inspire(entry) => &entry.bibtex,
            Self::Import(entry) => &entry.bibtex,
        }
    }

    /// Return INSPIRE metadata for managed references.
    pub fn inspire_entry(&self) -> Option<&InspireEntry> {
        match self {
            Self::Inspire(entry) => Some(entry),
            Self::Import(_) => None,
        }
    }

    pub(crate) fn project_checked(&self) -> Result<Reference, ProjectionError> {
        let reference = project_bibtex(self.raw_bibtex())
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        if let Self::Inspire(entry) = self {
            if entry.record_id == 0 {
                return Err(ProjectionError::Invalid("INSPIRE record id is zero".into()));
            }
            if let Some(arxiv) = &entry.identifiers.arxiv
                && !reference
                    .identifiers
                    .arxiv
                    .contains(&normalize_arxiv(arxiv))
            {
                return Err(ProjectionError::Invalid(format!(
                    "canonical arXiv id {} is absent from stored BibTeX",
                    normalize_arxiv(arxiv)
                )));
            }
            if let Some(doi) = &entry.identifiers.doi
                && !reference.identifiers.dois.contains(&normalize_doi(doi))
            {
                return Err(ProjectionError::Invalid(format!(
                    "canonical DOI {} is absent from stored BibTeX",
                    normalize_doi(doi)
                )));
            }
        }
        Ok(reference)
    }
}

impl ReferenceSource for SourceSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        self.project_checked()
    }
}

/// Controls whether an add must use a key or may reuse an existing membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyRequest {
    /// Require this exact shelf-local citation key.
    Exact(String),
    /// Use this key unless the reference already belongs to the shelf.
    Suggested(String),
}

impl KeyRequest {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Exact(value) | Self::Suggested(value) => value,
        }
    }

    pub(crate) fn into_string(self) -> String {
        match self {
            Self::Exact(value) | Self::Suggested(value) => value,
        }
    }
}

/// A validated source snapshot waiting to be attached to a shelf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingReference {
    /// Requested shelf-local citation key.
    pub key: KeyRequest,
    /// Authoritative source snapshot.
    pub source: SourceSnapshot,
}
