//! Parsing a file of standalone entries.

use crate::{
    entry::{BibtexEntry, CitationKey},
    error::Error,
    scanner,
};
use std::collections::BTreeSet;

/// One entry read out of a file, with the key the file gave it.
///
/// The key is carried separately because import preserves it: a colleague's
/// bibliography must not have its `\cite{}` commands invalidated by resolving
/// its entries against a provider (I4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedEntry {
    /// The citation key as the file wrote it.
    pub key: CitationKey,
    /// The entry itself.
    pub payload: BibtexEntry,
}

/// Parse a file containing only entries and whitespace, in source order.
///
/// No metadata is projected, so an entry without a title parses successfully
/// and is resolved against a provider like any other. Duplicate source keys are
/// rejected: the file is ambiguous about which entry a key names, and bibi
/// would have to choose.
pub fn parse_file(source: &str) -> Result<Vec<ImportedEntry>, Error> {
    let raw = scanner::scan(source)?;
    let mut keys = BTreeSet::new();
    let mut entries = Vec::with_capacity(raw.len());
    for entry in raw {
        let payload = BibtexEntry::from_raw(source, entry);
        let key = payload.source_key().clone();
        if !keys.insert(key.clone()) {
            return Err(Error::DuplicateKey {
                key: key.into_string(),
            });
        }
        entries.push(ImportedEntry { key, payload });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_entries_in_source_order_with_exact_bytes() {
        let first = "@misc{Zed,\n  title = {Zed}\n}";
        let second = "@article{Alpha,title={Alpha}}";
        let source = format!(" \n{first}\n\t\n{second}\n ");
        let entries = parse_file(&source).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["Zed", "Alpha"]
        );
        assert_eq!(entries[0].payload.source(), first);
        assert_eq!(entries[1].payload.source(), second);
    }

    #[test]
    fn accepts_an_untitled_entry_that_carries_an_identifier() {
        let entries = parse_file("@article{A, doi = {10.1/x}}").unwrap();
        assert_eq!(
            entries[0].payload.identifier_candidates().doi.as_deref(),
            Some("10.1/x")
        );
    }

    #[test]
    fn accepts_an_empty_file() {
        assert!(parse_file("").unwrap().is_empty());
        assert!(parse_file("\n \t\n").unwrap().is_empty());
    }

    #[test]
    fn rejects_duplicate_keys_and_non_entry_content() {
        assert!(matches!(
            parse_file("@misc{A,title={One}}\n\n@misc{A,title={Two}}"),
            Err(Error::DuplicateKey { key }) if key == "A"
        ));
        assert!(parse_file("% notes\n@misc{A,title={T}}").is_err());
        assert!(parse_file("@string{x = {y}}\n@misc{A,title={T}}").is_err());
    }
}
