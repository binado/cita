//! The five identity indexes, rebuilt from the records on every load.
//!
//! Indexes are never serialized. Rebuilding them keeps the record list
//! authoritative and makes a duplicate introduced by a hand-resolved merge
//! conflict visible at load time rather than at some later lookup.

use crate::error::Error;
use bibi_bibtex::CitationKey;
use bibi_core::{ArxivId, BibiId, Doi, Provider, ProviderId, Record};
use std::collections::HashMap;

/// Position lookups for one validated record list.
#[derive(Debug, Default)]
pub(crate) struct Indexes {
    pub(crate) by_id: HashMap<BibiId, usize>,
    pub(crate) by_key: HashMap<CitationKey, usize>,
    pub(crate) by_provider_identity: HashMap<(Provider, ProviderId), usize>,
    pub(crate) by_doi: HashMap<Doi, usize>,
    pub(crate) by_arxiv: HashMap<ArxivId, usize>,
}

impl Indexes {
    /// Build every index, rejecting a value claimed by two records.
    pub(crate) fn build(records: &[Record]) -> Result<Self, Error> {
        let mut indexes = Self::default();
        for (position, record) in records.iter().enumerate() {
            insert(
                &mut indexes.by_id,
                record.id,
                position,
                records,
                "record id",
                record.id.to_string(),
            )?;
            insert(
                &mut indexes.by_key,
                record.key.clone(),
                position,
                records,
                "citation key",
                record.key.to_string(),
            )?;
            if let Some((provider, id)) = record.provenance.identity() {
                insert(
                    &mut indexes.by_provider_identity,
                    (provider, id.clone()),
                    position,
                    records,
                    "provider identity",
                    format!("{provider}:{id}"),
                )?;
            }
            if let Some(doi) = &record.identifiers.doi {
                insert(
                    &mut indexes.by_doi,
                    doi.clone(),
                    position,
                    records,
                    "DOI",
                    doi.to_string(),
                )?;
            }
            if let Some(arxiv) = &record.identifiers.arxiv {
                insert(
                    &mut indexes.by_arxiv,
                    arxiv.clone(),
                    position,
                    records,
                    "arXiv id",
                    arxiv.to_string(),
                )?;
            }
        }
        Ok(indexes)
    }
}

fn insert<K: std::hash::Hash + Eq>(
    index: &mut HashMap<K, usize>,
    key: K,
    position: usize,
    records: &[Record],
    kind: &'static str,
    value: String,
) -> Result<(), Error> {
    if let Some(first) = index.insert(key, position) {
        return Err(Error::Duplicate {
            kind,
            value,
            first: records[first].key.to_string(),
            second: records[position].key.to_string(),
        });
    }
    Ok(())
}
