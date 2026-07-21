use cita_bibliography::project_bibtex;
use cita_core::{
    Identifiers, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use std::collections::{BTreeMap, BTreeSet};

/// A durable INSPIRE record: authoritative BibTeX plus the canonical identifiers
/// that BibTeX cannot express on its own. Bibliographic content is projected
/// from the BibTeX; only identity and refresh bookkeeping travel alongside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspireRecord {
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

/// The strict subset selected from a permissive INSPIRE wire record. Holds only
/// what the client needs: identity, refresh bookkeeping, a display title for
/// transient resolution, and texkeys for BibTeX matching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectedRecord {
    pub(crate) record_id: u64,
    pub(crate) updated: String,
    pub(crate) texkeys: Vec<String>,
    pub(crate) title: Option<String>,
    pub(crate) arxiv: Vec<String>,
    pub(crate) doi: Vec<String>,
}

impl SelectedRecord {
    pub(crate) fn record_id(&self) -> u64 {
        self.record_id
    }

    pub(crate) fn texkeys(&self) -> &[String] {
        &self.texkeys
    }

    fn normalized_arxiv(&self) -> Vec<String> {
        unique(self.arxiv.iter().map(|value| normalize_arxiv(value)))
    }

    fn normalized_doi(&self) -> Vec<String> {
        unique(self.doi.iter().map(|value| normalize_doi(value)))
    }

    /// Whether the record's supplied identifiers agree with a BibTeX projection.
    /// An identifier-free record relies on the already-validated texkey match.
    pub(crate) fn identity_matches(&self, bib: &Reference) -> bool {
        let arxiv = self.normalized_arxiv();
        let doi = self.normalized_doi();
        (arxiv.is_empty() && doi.is_empty())
            || doi.iter().any(|id| bib.identifiers.dois.contains(id))
            || arxiv.iter().any(|id| bib.identifiers.arxiv.contains(id))
    }

    /// Attach authoritative BibTeX to build the durable record. The caller has
    /// already cross-checked the BibTeX key and identity against this record.
    /// Curated arXiv/DOI are the lexicographically first ID in the intersection
    /// of normalized JSON identifiers with the BibTeX projection (or `None`).
    pub(crate) fn into_record(
        self,
        texkey: String,
        bibtex: String,
        bib: &Reference,
    ) -> InspireRecord {
        let arxiv = curated(self.normalized_arxiv(), &bib.identifiers.arxiv);
        let doi = curated(self.normalized_doi(), &bib.identifiers.dois);
        InspireRecord {
            record_id: self.record_id,
            updated: self.updated,
            texkey,
            bibtex,
            arxiv,
            doi,
        }
    }
}

impl ReferenceSource for SelectedRecord {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let title = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ProjectionError::MissingTitle)?
            .to_owned();
        let mut providers = BTreeMap::new();
        providers.insert("inspire".to_owned(), vec![self.record_id.to_string()]);
        Ok(Reference {
            title,
            identifiers: Identifiers {
                dois: self.normalized_doi(),
                arxiv: self.normalized_arxiv(),
                providers,
            },
            ..Reference::default()
        })
    }
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

impl ReferenceSource for InspireRecord {
    fn project(&self) -> Result<Reference, ProjectionError> {
        project_inspire(
            &self.bibtex,
            self.arxiv.as_deref(),
            self.doi.as_deref(),
            self.record_id,
        )
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn curated(json: Vec<String>, bib: &[String]) -> Option<String> {
    unique(json.into_iter().filter(|id| bib.contains(id)))
        .into_iter()
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(arxiv: &[&str], doi: &[&str]) -> SelectedRecord {
        SelectedRecord {
            record_id: 42,
            updated: "2026-01-01".into(),
            texkeys: vec!["Key:2026".into()],
            title: Some("JSON title".into()),
            arxiv: arxiv.iter().map(|value| (*value).into()).collect(),
            doi: doi.iter().map(|value| (*value).into()).collect(),
        }
    }

    #[test]
    fn transient_projection_uses_json_identity_only() {
        let reference = selected(&["2401.00001v2"], &["10.1/X"]).project().unwrap();
        assert_eq!(reference.title, "JSON title");
        assert_eq!(reference.identifiers.arxiv, ["2401.00001"]);
        assert_eq!(reference.identifiers.dois, ["10.1/x"]);
        assert_eq!(
            reference.identifiers.providers.get("inspire").unwrap(),
            &["42".to_owned()]
        );
    }

    #[test]
    fn durable_projection_derives_content_from_bibtex_and_overrides_identity() {
        let bibtex = "@article{Key:2026,title={Real title},author={Doe, Jane},year={2024},eprint={2401.00001},doi={10.1/X}}";
        let bib = project_bibtex(bibtex).unwrap();
        let record = selected(&["2401.00001v2"], &["10.1/X"]).into_record(
            "Key:2026".into(),
            bibtex.into(),
            &bib,
        );
        let reference = record.project().unwrap();
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

    #[test]
    fn curated_ids_come_from_json_bibtex_intersection_not_lex_first_json() {
        let bibtex = "@misc{Key:2026,title={T},doi={10.1/zzz},eprint={2401.00002}}";
        let bib = project_bibtex(bibtex).unwrap();
        let record = selected(&["2401.00001", "2401.00002"], &["10.1/zzz", "10.1/aaa"])
            .into_record("Key:2026".into(), bibtex.into(), &bib);
        assert_eq!(record.doi.as_deref(), Some("10.1/zzz"));
        assert_eq!(record.arxiv.as_deref(), Some("2401.00002"));
    }

    #[test]
    fn curated_ids_are_none_when_json_and_bibtex_have_no_identifiers() {
        let bibtex = "@misc{Key:2026,title={T}}";
        let bib = project_bibtex(bibtex).unwrap();
        let record = selected(&[], &[]).into_record("Key:2026".into(), bibtex.into(), &bib);
        assert_eq!(record.arxiv, None);
        assert_eq!(record.doi, None);
    }

    #[test]
    fn identity_matching_accepts_shared_or_absent_identifiers() {
        let bib = project_bibtex("@misc{Key:2026,title={T},eprint={2401.00001}}").unwrap();
        assert!(selected(&["2401.00001v2"], &[]).identity_matches(&bib));
        assert!(selected(&[], &[]).identity_matches(&bib));
        assert!(!selected(&[], &["10.1/absent"]).identity_matches(&bib));
    }
}
