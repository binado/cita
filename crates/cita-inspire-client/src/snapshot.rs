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
    pub record_id: u64,
    pub updated: String,
    /// The INSPIRE texkey used as the suggested local citation key.
    pub texkey: String,
    pub bibtex: String,
    pub arxiv: Option<String>,
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
    pub(crate) fn into_record(self, texkey: String, bibtex: String) -> InspireRecord {
        let arxiv = self.normalized_arxiv().into_iter().next();
        let doi = self.normalized_doi().into_iter().next();
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
            url: Some(format!(
                "https://inspirehep.net/literature/{}",
                self.record_id
            )),
            identifiers: Identifiers {
                dois: self.normalized_doi(),
                arxiv: self.normalized_arxiv(),
                providers,
            },
            ..Reference::default()
        })
    }
}

impl ReferenceSource for InspireRecord {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let mut reference = project_bibtex(&self.bibtex)
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        if let Some(arxiv) = &self.arxiv {
            reference.identifiers.arxiv = vec![normalize_arxiv(arxiv)];
        }
        if let Some(doi) = &self.doi {
            reference.identifiers.dois = vec![normalize_doi(doi)];
        }
        reference
            .identifiers
            .providers
            .insert("inspire".to_owned(), vec![self.record_id.to_string()]);
        Ok(reference)
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
        let record = selected(&["2401.00001v2"], &["10.1/X"]).into_record(
            "Key:2026".into(),
            "@article{Key:2026,title={Real title},author={Doe, Jane},year={2024}}".into(),
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
    fn identity_matching_accepts_shared_or_absent_identifiers() {
        let bib = project_bibtex("@misc{Key:2026,title={T},eprint={2401.00001}}").unwrap();
        assert!(selected(&["2401.00001v2"], &[]).identity_matches(&bib));
        assert!(selected(&[], &[]).identity_matches(&bib));
        assert!(!selected(&[], &["10.1/absent"]).identity_matches(&bib));
    }
}
