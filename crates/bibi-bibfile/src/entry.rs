//! One entry, and the tool-owned fields that ride along inside it.

use crate::Error;
use bibi_bibliography::{field, insert_field, project_bibtex, remove_field, rename_entry};
use bibi_core::{Reference, normalize_arxiv, normalize_doi};
use bibi_inspire_client::{InspireRecord, project_inspire};

/// Namespace owned by bibi. Fields under it are bookkeeping, never authored.
pub const FIELD_PREFIX: &str = "x-bibi-";

/// Stable INSPIRE record id. Its presence is what makes an entry *managed*.
pub const INSPIRE_ID_FIELD: &str = "x-bibi-inspire-id";

/// Provider update timestamp, used to skip refreshes that would change nothing.
pub const INSPIRE_UPDATED_FIELD: &str = "x-bibi-inspire-updated";

/// Curated normalized arXiv identifier, overriding the projected one.
pub const ARXIV_FIELD: &str = "x-bibi-arxiv";

/// Curated normalized DOI, overriding the projected one.
pub const DOI_FIELD: &str = "x-bibi-doi";

/// Marks an entry the user owns outright: never refreshed, never resolved.
pub const FROZEN_FIELD: &str = "x-bibi-frozen";

/// One complete BibTeX entry as it appears in the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Local citation key, the same token that appears in `\cite{}`.
    pub key: String,
    /// The complete raw `@type{...}` entry, tool-owned fields included.
    pub bibtex: String,
}

impl Entry {
    /// Build an entry from an INSPIRE record stored under `key`.
    ///
    /// The record's BibTeX is re-keyed to the local key and its bookkeeping is
    /// spliced in as tool-owned fields, so the entry carries everything needed
    /// to refresh it later without consulting anything else.
    pub fn from_inspire(key: &str, record: &InspireRecord) -> Result<Self, Error> {
        let mut bibtex = rename_entry(&record.bibtex, key)?;
        bibtex = insert_field(&bibtex, INSPIRE_ID_FIELD, &record.record_id.to_string())?;
        bibtex = insert_field(&bibtex, INSPIRE_UPDATED_FIELD, &record.updated)?;
        if let Some(arxiv) = &record.arxiv {
            bibtex = insert_field(&bibtex, ARXIV_FIELD, &normalize_arxiv(arxiv))?;
        }
        if let Some(doi) = &record.doi {
            bibtex = insert_field(&bibtex, DOI_FIELD, &normalize_doi(doi))?;
        }
        Ok(Self {
            key: key.to_owned(),
            bibtex,
        })
    }

    /// Build an entry from raw BibTeX that already carries its own key.
    pub fn new(key: String, bibtex: String) -> Self {
        Self { key, bibtex }
    }

    /// Read one tool-owned field.
    fn tool_field(&self, name: &str) -> Option<String> {
        field(&self.bibtex, name).ok().flatten()
    }

    /// Stable INSPIRE record id, when this entry is managed.
    ///
    /// A malformed or zero id reads as absent rather than failing: a hand-typed
    /// `x-bibi-inspire-id = {oops}` should leave the entry unmanaged, not make
    /// the whole file unreadable.
    pub fn inspire_record_id(&self) -> Option<u64> {
        self.tool_field(INSPIRE_ID_FIELD)?
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|id| *id != 0)
    }

    /// Provider update timestamp recorded at the last refresh.
    pub fn inspire_updated(&self) -> Option<String> {
        self.tool_field(INSPIRE_UPDATED_FIELD)
    }

    /// Whether the user has taken this entry out of bibi's hands.
    pub fn is_frozen(&self) -> bool {
        self.tool_field(FROZEN_FIELD)
            .is_some_and(|value| !matches!(value.trim(), "false" | "no" | "0"))
    }

    /// Whether bibi refreshes this entry from INSPIRE.
    pub fn is_managed(&self) -> bool {
        self.inspire_record_id().is_some()
    }

    /// Project the entry into bibi's source-neutral reference fields.
    ///
    /// Curated identifiers override what the BibTeX says, matching how INSPIRE
    /// snapshots have always resolved identity.
    pub fn project(&self) -> Result<Reference, Error> {
        let arxiv = self.tool_field(ARXIV_FIELD);
        let doi = self.tool_field(DOI_FIELD);
        let invalid = |message: String| Error::InvalidEntry {
            key: self.key.clone(),
            message,
        };
        match self.inspire_record_id() {
            Some(record_id) => {
                project_inspire(&self.bibtex, arxiv.as_deref(), doi.as_deref(), record_id)
                    .map_err(|error| invalid(error.to_string()))
            }
            None => {
                let mut reference =
                    project_bibtex(&self.bibtex).map_err(|error| invalid(error.to_string()))?;
                if let Some(arxiv) = arxiv {
                    reference.identifiers.arxiv = vec![normalize_arxiv(&arxiv)];
                }
                if let Some(doi) = doi {
                    reference.identifiers.dois = vec![normalize_doi(&doi)];
                }
                Ok(reference)
            }
        }
    }

    /// Change the entry's citation key, touching only that token.
    pub fn rekey(&mut self, new_key: &str) -> Result<(), Error> {
        self.bibtex = rename_entry(&self.bibtex, new_key)?;
        self.key = new_key.to_owned();
        Ok(())
    }

    /// Replace the entry's content with a refreshed INSPIRE record.
    ///
    /// The local key is preserved and every tool-owned field the entry carried
    /// is re-established, because the refreshed BibTeX arrives without them.
    /// A frozen entry is never passed here; callers filter first.
    pub fn refresh(&mut self, record: &InspireRecord) -> Result<(), Error> {
        let refreshed = Self::from_inspire(&self.key, record)?;
        self.bibtex = refreshed.bibtex;
        Ok(())
    }

    /// Drop one tool-owned field, if present.
    pub fn clear_field(&mut self, name: &str) -> Result<(), Error> {
        self.bibtex = remove_field(&self.bibtex, name)?;
        Ok(())
    }
}
