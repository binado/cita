//! Stable records around complete replaceable state.

use crate::{ArxivId, Doi, Error, RecordId, Source};
use bibi_bibtex::BibtexEntry;

/// Canonical provider-neutral identifiers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identifiers {
    doi: Option<Doi>,
    arxiv: Option<ArxivId>,
}

impl Identifiers {
    /// Construct canonical optional identifiers.
    pub fn new(doi: Option<Doi>, arxiv: Option<ArxivId>) -> Self {
        Self { doi, arxiv }
    }

    /// Canonical DOI.
    pub fn doi(&self) -> Option<&Doi> {
        self.doi.as_ref()
    }

    /// Canonical, versionless arXiv identifier.
    pub fn arxiv(&self) -> Option<&ArxivId> {
        self.arxiv.as_ref()
    }
}

/// Advisory data used for listing and filtering.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Description {
    title: String,
    authors: Vec<String>,
    collaborations: Vec<String>,
    year: Option<i32>,
}

impl Description {
    /// Construct advisory display metadata.
    pub fn new(
        title: impl Into<String>,
        authors: Vec<String>,
        collaborations: Vec<String>,
        year: Option<i32>,
    ) -> Self {
        Self {
            title: title.into(),
            authors,
            collaborations,
            year,
        }
    }

    /// Work title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Authors in source order.
    pub fn authors(&self) -> &[String] {
        &self.authors
    }

    /// Collaborations in source order.
    pub fn collaborations(&self) -> &[String] {
        &self.collaborations
    }

    /// Chosen year.
    pub fn year(&self) -> Option<i32> {
        self.year
    }
}

/// Complete state that may atomically replace a resident record's state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordState {
    source: Source,
    identifiers: Identifiers,
    description: Description,
    bibtex: BibtexEntry,
}

impl RecordState {
    /// Construct and validate complete state.
    pub fn new(
        source: Source,
        identifiers: Identifiers,
        description: Description,
        bibtex: BibtexEntry,
    ) -> Result<Self, Error> {
        let state = Self {
            source,
            identifiers,
            description,
            bibtex,
        };
        state.validate()?;
        Ok(state)
    }

    /// Construct local state from user-supplied BibTeX.
    pub fn local(bibtex: BibtexEntry) -> Result<Self, Error> {
        let texkey = bibtex.texkey().to_owned();
        let metadata = bibtex
            .local_metadata()
            .map_err(|source| Error::LocalMetadata { texkey, source })?;
        let identifiers = Identifiers::new(
            metadata
                .doi
                .as_deref()
                .and_then(|value| Doi::new(value).ok()),
            metadata
                .arxiv
                .as_deref()
                .and_then(|value| ArxivId::new(value).ok()),
        );
        Self::new(
            Source::Local,
            identifiers,
            Description::new(
                metadata.title,
                metadata.authors,
                metadata.collaborations,
                metadata.year,
            ),
            bibtex,
        )
    }

    /// Re-check state invariants.
    pub fn validate(&self) -> Result<(), Error> {
        if self.description.title().trim().is_empty() {
            return Err(Error::MissingTitle {
                texkey: self.texkey().to_owned(),
            });
        }
        Ok(())
    }

    /// State source.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Canonical identifiers.
    pub fn identifiers(&self) -> &Identifiers {
        &self.identifiers
    }

    /// Advisory description.
    pub fn description(&self) -> &Description {
        &self.description
    }

    /// Exact BibTeX.
    pub fn bibtex(&self) -> &BibtexEntry {
        &self.bibtex
    }

    /// Texkey derived from exact BibTeX.
    pub fn texkey(&self) -> &str {
        self.bibtex.texkey()
    }
}

/// A resident bibliography entity with stable local identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    id: RecordId,
    state: RecordState,
}

impl Record {
    pub(crate) fn restore(id: RecordId, state: RecordState) -> Self {
        Self { id, state }
    }

    /// Stable local id.
    pub fn id(&self) -> RecordId {
        self.id
    }

    /// Complete replaceable state.
    pub fn state(&self) -> &RecordState {
        &self.state
    }

    /// Derived texkey.
    pub fn texkey(&self) -> &str {
        self.state.texkey()
    }

    pub(crate) fn replace(&mut self, state: RecordState) {
        self.state = state;
    }
}
