use cita_core::{ProjectionError, Reference, ReferenceSource};

/// A durable INSPIRE snapshot: the authoritative projected reference plus the
/// complete BibTeX blob and refresh bookkeeping.
///
/// Bibliographic content comes from the INSPIRE JSON record, projected once at
/// attach time (see `ApiLiteratureRecord::attach_bibtex`); `bibtex` rides
/// along verbatim as opaque cargo for `references.bib` rendering and is never
/// re-parsed to derive it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspireSnapshot {
    /// Stable numeric INSPIRE record identifier used for refresh.
    pub record_id: u64,
    /// Provider update timestamp.
    pub updated: String,
    /// The INSPIRE texkey used as the suggested local citation key.
    pub texkey: String,
    /// Complete authoritative standalone BibTeX entry.
    pub bibtex: String,
    /// The reference projected from the INSPIRE JSON record.
    pub reference: Reference,
}

impl ReferenceSource for InspireSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        Ok(self.reference.clone())
    }
}
