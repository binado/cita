//! Stable records around complete replaceable state.

use crate::{ArxivId, Bibtex, Doi, Error, RecordId, Source};

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
    bibtex: Bibtex,
}

impl RecordState {
    /// Construct and validate complete state.
    pub fn new(
        source: Source,
        identifiers: Identifiers,
        description: Description,
        bibtex: Bibtex,
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

    /// Construct local state from parser-projected user input.
    pub fn local(
        bibtex: Bibtex,
        identifiers: Identifiers,
        description: Description,
    ) -> Result<Self, Error> {
        Self::new(Source::Local, identifiers, description, bibtex)
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
    pub fn bibtex(&self) -> &Bibtex {
        &self.bibtex
    }

    /// Stored texkey derived from the exact BibTeX at the parser boundary.
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

    /// Stored texkey.
    pub fn texkey(&self) -> &str {
        self.state.texkey()
    }

    pub(crate) fn replace(&mut self, state: RecordState) {
        self.state = state;
    }
}
