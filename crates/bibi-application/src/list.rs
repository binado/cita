//! Listing and JSON projection.

use crate::Error;
use bibi_core::{Bibliography, Record, RecordFilter, Source};
use serde::Serialize;

/// Listing filter.
#[derive(Clone, Debug, Default)]
pub struct ListRequest {
    /// Conjunctive record filter.
    pub filter: RecordFilter,
}

/// Return cloned records in derived-texkey order.
pub fn list(bibliography: &Bibliography, request: &ListRequest) -> Vec<Record> {
    bibliography.filter(&request.filter).cloned().collect()
}

/// Stable JSON projection without the BibTeX payload.
#[derive(Debug, Serialize)]
pub struct ListJsonRecord {
    id: String,
    key: String,
    source: String,
    provider_id: Option<String>,
    doi: Option<String>,
    arxiv: Option<String>,
    title: String,
    authors: Vec<String>,
    collaborations: Vec<String>,
    year: Option<i32>,
}

impl ListJsonRecord {
    fn from_record(record: &Record) -> Self {
        let state = record.state();
        let (source, provider_id) = match state.source() {
            Source::Local => ("local".to_owned(), None),
            Source::Managed { provider, id } => (provider.to_string(), Some(id.to_string())),
        };
        Self {
            id: record.id().to_string(),
            key: record.texkey().to_owned(),
            source,
            provider_id,
            doi: state.identifiers().doi().map(ToString::to_string),
            arxiv: state.identifiers().arxiv().map(ToString::to_string),
            title: state.description().title().to_owned(),
            authors: state.description().authors().to_vec(),
            collaborations: state.description().collaborations().to_vec(),
            year: state.description().year(),
        }
    }
}

/// Render a JSON array with one trailing newline.
pub fn to_json(records: &[Record]) -> Result<String, Error> {
    let mut output = serde_json::to_string_pretty(
        &records
            .iter()
            .map(ListJsonRecord::from_record)
            .collect::<Vec<_>>(),
    )?;
    output.push('\n');
    Ok(output)
}
