//! Strict schema-1 conversion.

use crate::Error;
use bibi_core::{
    ArxivId, Bibliography, Bibtex, Description, Doi, Identifiers, ProviderId, ProviderName, Record,
    RecordId, RecordState, Source,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, str::FromStr};

/// The bibliography schema this build reads and writes.
pub const SCHEMA: u64 = 1;

#[derive(Deserialize)]
struct SchemaProbe {
    schema: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    records: Vec<RecordWire>,
}

/// Field order is the canonical serialized order.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordWire {
    id: String,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    doi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arxiv: Option<String>,
    title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    collaborations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    texkey: String,
    bibtex: String,
}

fn check_schema(path: &Path, source: &str) -> Result<(), Error> {
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
    fn from_record(record: &Record) -> Self {
        let state = record.state();
        let (source, provider_id) = match state.source() {
            Source::Local => ("local".to_owned(), None),
            Source::Managed { provider, id } => (provider.to_string(), Some(id.to_string())),
        };
        Self {
            id: record.id().to_string(),
            source,
            provider_id,
            doi: state.identifiers().doi().map(ToString::to_string),
            arxiv: state.identifiers().arxiv().map(ToString::to_string),
            title: state.description().title().to_owned(),
            authors: state.description().authors().to_vec(),
            collaborations: state.description().collaborations().to_vec(),
            year: state.description().year(),
            texkey: state.texkey().to_owned(),
            bibtex: state.bibtex().source().to_owned(),
        }
    }

    fn into_pair(self) -> Result<(RecordId, RecordState), Error> {
        let label = self.id.clone();
        let field = |field: &'static str| {
            let record = label.clone();
            move |source| Error::InvalidField {
                record,
                field,
                source,
            }
        };
        let id = RecordId::from_str(&self.id).map_err(field("id"))?;
        let source = match (self.source.as_str(), self.provider_id) {
            ("local", None) => Source::Local,
            ("local", Some(_)) => {
                return Err(Error::InvalidSource {
                    record: label,
                    message: "`provider_id` is forbidden for local records",
                });
            }
            (provider, Some(provider_id)) => Source::Managed {
                provider: ProviderName::from_str(provider).map_err(field("source"))?,
                id: ProviderId::new(provider_id).map_err(field("provider_id"))?,
            },
            (_, None) => {
                return Err(Error::InvalidSource {
                    record: label,
                    message: "`provider_id` is required for managed records",
                });
            }
        };
        let identifiers = Identifiers::new(
            self.doi.map(Doi::new).transpose().map_err(field("doi"))?,
            self.arxiv
                .map(ArxivId::new)
                .transpose()
                .map_err(field("arxiv"))?,
        );
        let bibtex = Bibtex::new(self.bibtex, self.texkey);
        let state = RecordState::new(
            source,
            identifiers,
            Description::new(self.title, self.authors, self.collaborations, self.year),
            bibtex,
        )?;
        Ok((id, state))
    }
}

pub(crate) fn render(bibliography: &Bibliography) -> Result<String, Error> {
    let document = Document {
        schema: SCHEMA,
        records: bibliography
            .records()
            .iter()
            .map(RecordWire::from_record)
            .collect(),
    };
    let mut rendered = toml::to_string_pretty(&document)?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

pub(crate) fn parse(path: &Path, source: &str) -> Result<Bibliography, Error> {
    check_schema(path, source)?;
    let document: Document = toml::from_str(source).map_err(|source| Error::Decode {
        path: path.to_owned(),
        source,
    })?;
    Bibliography::restore(
        document
            .records
            .into_iter()
            .map(RecordWire::into_pair)
            .collect::<Result<_, _>>()?,
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_checked_before_record_shape() {
        let path = Path::new("bibi.toml");
        let newer = "schema = 2\n\n[[records]]\nfuture = true\n";
        assert!(matches!(
            check_schema(path, newer),
            Err(Error::UnsupportedSchema { found: 2, .. })
        ));
    }

    #[test]
    fn empty_round_trip_is_canonical() {
        let rendered = render(&Bibliography::empty()).unwrap();
        assert_eq!(rendered, "schema = 1\n");
        assert!(parse(Path::new("bibi.toml"), &rendered).unwrap().is_empty());
    }
}
