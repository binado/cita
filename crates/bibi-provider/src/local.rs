//! Direct ingestion of user-supplied BibTeX.

use bibi_bibtex::BibtexEntry;
use bibi_core::{
    ArxivId, Description, Doi, Identifiers, Provenance, ProviderName, ProviderOwned,
    provider::{MappingError, ProviderError},
};

/// The manifest provenance name for explicitly local entries.
pub const LOCAL_PROVIDER: &str = "local";

pub(crate) fn ingest(entry: BibtexEntry) -> Result<ProviderOwned, ProviderError> {
    let name = ProviderName::new(LOCAL_PROVIDER).expect("the local provider name is valid");
    let metadata = entry.local_metadata().map_err(|error| {
        ProviderError::Mapping(MappingError::InvalidPayload {
            provider: name.clone(),
            message: error.to_string(),
        })
    })?;
    let identifiers = Identifiers {
        doi: metadata.doi.as_deref().and_then(|doi| Doi::new(doi).ok()),
        arxiv: metadata
            .arxiv
            .as_deref()
            .and_then(|arxiv| ArxivId::new(arxiv).ok()),
    };
    Ok(ProviderOwned {
        provenance: Provenance::unmanaged(name),
        identifiers,
        description: Description {
            title: metadata.title,
            authors: metadata.authors,
            collaborations: metadata.collaborations,
            year: metadata.year,
        },
        payload: entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    #[test]
    fn ingestion_projects_metadata_and_preserves_bytes() {
        let source =
            "@software{tool,  title={A tool}, author={Roe, Richard}, year=2024, note={  x  }}";
        let record = ingest(entry(source)).unwrap();
        assert_eq!(record.description.title, "A tool");
        assert_eq!(record.description.authors, ["Roe, Richard"]);
        assert_eq!(record.description.year, Some(2024));
        assert_eq!(record.payload.source(), source);
        assert_eq!(record.provenance.provider.as_str(), LOCAL_PROVIDER);
        assert!(record.provenance.provider_id.is_none());
    }

    #[test]
    fn ingestion_requires_describable_bibtex() {
        assert!(ingest(entry("@misc{k,doi={10.1/x}}")).is_err());
    }
}
