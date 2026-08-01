//! Parsing a file of standalone entries.

use crate::{
    adapter,
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

/// One entry a lenient parse could not place, with what went wrong.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryFailure {
    /// The entry's own byte span within the source, for a caller that wants
    /// to show the offending text.
    pub span: std::ops::Range<usize>,
    /// Why the entry did not import.
    pub error: Error,
}

/// Parse a file containing only entries and whitespace, in source order.
///
/// No metadata is projected, so an entry without a title parses successfully
/// and is resolved against a provider like any other. Duplicate source keys are
/// rejected: the file is ambiguous about which entry a key names, and bibi
/// would have to choose. One malformed entry fails the whole file; callers
/// that instead want every other entry to survive use [`parse_file_partial`].
pub fn parse_file(source: &str) -> Result<Vec<ImportedEntry>, Error> {
    let mut keys = BTreeSet::new();
    let mut entries = Vec::new();
    for span in scanner::split(source) {
        let raw = adapter::parse_entry(source, span)?;
        let payload = BibtexEntry::from_raw(source, raw);
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

/// Parse a file leniently: an entry that fails becomes its own [`EntryFailure`]
/// rather than sinking every other entry in the file.
///
/// This is what `add -f` needs against a colleague's `.bib`, where one
/// malformed or unsupported entry (an `@string` macro, a stray directive)
/// should not cost the entries around it. A duplicate source key is likewise
/// only that entry's failure, not the whole batch's: the first occurrence of
/// the key is kept, and every later entry claiming it fails instead.
pub fn parse_file_partial(source: &str) -> (Vec<ImportedEntry>, Vec<EntryFailure>) {
    let mut keys = BTreeSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for span in scanner::split(source) {
        let raw = match adapter::parse_entry(source, span.clone()) {
            Ok(raw) => raw,
            Err(error) => {
                failures.push(EntryFailure { span, error });
                continue;
            }
        };
        let payload = BibtexEntry::from_raw(source, raw);
        let key = payload.source_key().clone();
        if !keys.insert(key.clone()) {
            failures.push(EntryFailure {
                span,
                error: Error::DuplicateKey {
                    key: key.into_string(),
                },
            });
            continue;
        }
        entries.push(ImportedEntry { key, payload });
    }
    (entries, failures)
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
    fn rejects_duplicate_keys_and_unsupported_directives() {
        assert!(matches!(
            parse_file("@misc{A,title={One}}\n\n@misc{A,title={Two}}"),
            Err(Error::DuplicateKey { key }) if key == "A"
        ));
        // `@string` parses as an abbreviation, not an entry, so the strict
        // parser has nothing to place it as and the whole file fails.
        assert!(parse_file("@string{x = {y}}\n@misc{A,title={T}}").is_err());
    }

    #[test]
    fn partial_parsing_places_every_entry_a_directive_could_sink() {
        let source = "@misc{A,title={One}}\n\n@string{x = {y}}\n\n@misc{B,title={Two}}";
        let (entries, failures) = parse_file_partial(source);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(failures.len(), 1);
        assert!(matches!(failures[0].error, Error::InvalidGrammar { .. }));
    }

    #[test]
    fn partial_parsing_reports_a_duplicate_key_as_only_that_entrys_failure() {
        let source = "@misc{A,title={One}}\n\n@misc{A,title={Two}}\n\n@misc{B,title={Three}}";
        let (entries, failures) = parse_file_partial(source);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(failures.len(), 1);
        assert!(matches!(&failures[0].error, Error::DuplicateKey { key } if key == "A"));
    }
}
