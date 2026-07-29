//! The serialized schema-1 shape, and its conversion to and from records.
//!
//! Unknown fields are rejected in this direction, the opposite of how provider
//! responses are read. An unrecognized key means the file was written by a
//! newer bibi, and ignoring it would make the next write silently drop data
//! bibi did not understand — a lossy round-trip on the user's tracked file.

use crate::error::Error;
use bibi_bibtex::{BibtexEntry, CitationKey};
use bibi_core::{
    ArxivId, BibiId, Description, Doi, Identifiers, Provenance, ProviderId, ProviderName,
    ProviderOwned, Record, Revision,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, str::FromStr};

/// The manifest schema this build reads and writes.
pub const SCHEMA: u64 = 1;

/// A tolerant first read, used only to learn the schema number.
///
/// Reading the version before the records is what lets a file from a newer bibi
/// be diagnosed directly, rather than surfacing as a confusing complaint about
/// some field that version added.
#[derive(Deserialize)]
struct SchemaProbe {
    schema: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub(crate) schema: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) records: Vec<RecordWire>,
}

/// The serialized record. Field order here is the canonical file order.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordWire {
    id: String,
    key: String,
    provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    doi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arxiv: Option<String>,
    title: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    collaborations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    bibtex: String,
}

/// Read the schema number, rejecting anything this build cannot decode.
pub(crate) fn check_schema(path: &Path, source: &str) -> Result<(), Error> {
    let probe: SchemaProbe = toml::from_str(source).map_err(|source| Error::Decode {
        path: path.to_owned(),
        source,
    })?;
    match probe.schema {
        None => Err(Error::MissingSchema {
            path: path.to_owned(),
        }),
        Some(SCHEMA) => Ok(()),
        Some(found) => Err(Error::UnsupportedSchema {
            path: path.to_owned(),
            found,
            supported: SCHEMA,
        }),
    }
}

impl RecordWire {
    /// Project a record into its serialized shape.
    pub(crate) fn from_record(record: &Record) -> Self {
        Self {
            id: record.id.to_string(),
            key: record.key.to_string(),
            provider: record.provenance.provider.to_string(),
            provider_id: record
                .provenance
                .provider_id
                .as_ref()
                .map(ProviderId::to_string),
            revision: record.provenance.revision.as_ref().map(Revision::to_string),
            doi: record.identifiers.doi.as_ref().map(Doi::to_string),
            arxiv: record.identifiers.arxiv.as_ref().map(ArxivId::to_string),
            title: record.description.title.clone(),
            authors: record.description.authors.clone(),
            collaborations: record.description.collaborations.clone(),
            year: record.description.year,
            bibtex: record.payload.source().to_owned(),
        }
    }

    /// Convert a serialized record through validated domain constructors.
    pub(crate) fn into_record(self) -> Result<Record, Error> {
        let label = self.key.clone();
        let field = |field: &'static str| {
            let key = label.clone();
            move |source: bibi_core::Error| Error::InvalidField { key, field, source }
        };
        let id = BibiId::from_str(&self.id).map_err(field("id"))?;
        let key = CitationKey::new(self.key.clone()).map_err(|source| Error::InvalidKey {
            id: self.id.clone(),
            source,
        })?;
        let payload =
            BibtexEntry::parse_one(self.bibtex).map_err(|source| Error::InvalidPayload {
                key: label.clone(),
                source,
            })?;
        let provenance = Provenance {
            provider: ProviderName::new(self.provider).map_err(field("provider"))?,
            provider_id: self
                .provider_id
                .map(ProviderId::new)
                .transpose()
                .map_err(field("provider_id"))?,
            revision: self
                .revision
                .map(Revision::new)
                .transpose()
                .map_err(field("revision"))?,
        };
        let identifiers = Identifiers {
            doi: self.doi.map(Doi::new).transpose().map_err(field("doi"))?,
            arxiv: self
                .arxiv
                .map(ArxivId::new)
                .transpose()
                .map_err(field("arxiv"))?,
        };
        Ok(Record::new(
            id,
            key,
            ProviderOwned {
                provenance,
                identifiers,
                payload,
                description: Description {
                    title: self.title,
                    authors: self.authors,
                    collaborations: self.collaborations,
                    year: self.year,
                },
            },
        )?)
    }
}

/// Serialize records, which the caller has already sorted and validated.
pub(crate) fn render(records: &[Record]) -> Result<String, Error> {
    let document = Document {
        schema: SCHEMA,
        records: records.iter().map(RecordWire::from_record).collect(),
    };
    let mut rendered = toml::to_string_pretty(&document)?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

/// Decode a manifest's records, after its schema has been checked.
pub(crate) fn parse(path: &Path, source: &str) -> Result<Vec<Record>, Error> {
    check_schema(path, source)?;
    let document: Document = toml::from_str(source).map_err(|source| Error::Decode {
        path: path.to_owned(),
        source,
    })?;
    document
        .records
        .into_iter()
        .map(RecordWire::into_record)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> &'static Path {
        Path::new("bibi.toml")
    }

    #[test]
    fn the_schema_number_is_read_before_anything_else() {
        // A newer manifest is diagnosed by version, not by whichever field that
        // version added, even though the unknown field would also be rejected.
        let newer = "schema = 2\n\n[[records]]\nid = \"x\"\nfuture_field = true\n";
        assert!(matches!(
            check_schema(path(), newer),
            Err(Error::UnsupportedSchema { found: 2, .. })
        ));
        assert!(matches!(
            check_schema(path(), "records = []\n"),
            Err(Error::MissingSchema { .. })
        ));
        assert!(matches!(
            check_schema(path(), "schema = 0\n"),
            Err(Error::UnsupportedSchema { found: 0, .. })
        ));
        assert!(check_schema(path(), "schema = 1\n").is_ok());
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        for source in [
            "schema = 1\nextra = true\n",
            "schema = 1\n\n[[records]]\nid = \"d760f219-9098-4b49-9f62-10cbbcc22b11\"\nkey = \"K\"\nprovider = \"local\"\ntitle = \"T\"\nbibtex = \"@misc{K,title={T}}\"\nextra = 1\n",
        ] {
            assert!(
                matches!(parse(path(), source), Err(Error::Decode { .. })),
                "{source}"
            );
        }
    }

    #[test]
    fn an_empty_manifest_renders_and_parses() {
        let rendered = render(&[]).unwrap();
        assert_eq!(rendered, "schema = 1\n");
        assert!(parse(path(), &rendered).unwrap().is_empty());
    }

    #[test]
    fn a_stored_value_that_is_not_a_domain_value_is_reported_by_field() {
        let source = "schema = 1\n\n[[records]]\nid = \"d760f219-9098-4b49-9f62-10cbbcc22b11\"\nkey = \"K\"\nprovider = \"INSPIRE\"\ntitle = \"T\"\nbibtex = \"@misc{K,title={T}}\"\n";
        assert!(matches!(
            parse(path(), source),
            Err(Error::InvalidField {
                field: "provider",
                ..
            })
        ));
    }

    #[test]
    fn an_invalid_key_field_is_named_as_the_key_not_the_payload() {
        let source = "schema = 1\n\n[[records]]\nid = \"d760f219-9098-4b49-9f62-10cbbcc22b11\"\nkey = \"bad key\"\nprovider = \"local\"\ntitle = \"T\"\nbibtex = \"@misc{K,title={T}}\"\n";
        assert!(matches!(
            parse(path(), source),
            Err(Error::InvalidKey { .. })
        ));
    }
}
