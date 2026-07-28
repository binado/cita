use cita_bibliography::project_bibtex;
use cita_core::{ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi};

/// A durable INSPIRE snapshot: authoritative BibTeX plus the canonical
/// identifiers that BibTeX cannot express on its own.
///
/// Bibliographic content is projected from the BibTeX; only identity and
/// refresh bookkeeping travel alongside it.
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
    /// Curated, normalized, versionless arXiv identifier.
    pub arxiv: Option<String>,
    /// Curated, normalized DOI.
    pub doi: Option<String>,
}

/// Project authoritative INSPIRE BibTeX, then overlay curated arXiv/DOI and the
/// stable provider record id.
pub fn project_inspire(
    bibtex: &str,
    arxiv: Option<&str>,
    doi: Option<&str>,
    record_id: u64,
) -> Result<Reference, ProjectionError> {
    let mut reference =
        project_bibtex(bibtex).map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    if let Some(arxiv) = arxiv {
        reference.identifiers.arxiv = vec![normalize_arxiv(arxiv)];
    }
    if let Some(doi) = doi {
        reference.identifiers.dois = vec![normalize_doi(doi)];
    }
    reference
        .identifiers
        .providers
        .insert("inspire".to_owned(), vec![record_id.to_string()]);
    Ok(reference)
}

impl ReferenceSource for InspireSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        project_inspire(
            &self.bibtex,
            self.arxiv.as_deref(),
            self.doi.as_deref(),
            self.record_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_projection_derives_content_from_bibtex_and_overrides_identity() {
        let snapshot = InspireSnapshot {
            record_id: 42,
            updated: "2026-01-01".into(),
            texkey: "Key:2026".into(),
            bibtex: "@article{Key:2026,title={Real title},author={Doe, Jane},year={2024},eprint={2401.00001},doi={10.1/X}}".into(),
            arxiv: Some("2401.00001".into()),
            doi: Some("10.1/x".into()),
        };
        let reference = snapshot.project().unwrap();
        assert_eq!(reference.title, "Real title");
        assert_eq!(reference.authors, ["Jane Doe"]);
        assert_eq!(reference.year, Some(2024));
        assert_eq!(reference.identifiers.arxiv, ["2401.00001"]);
        assert_eq!(reference.identifiers.dois, ["10.1/x"]);
        assert_eq!(
            reference.identifiers.providers.get("inspire").unwrap(),
            &["42".to_owned()]
        );
    }
}
