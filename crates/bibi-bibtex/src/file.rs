//! Parsing a file of standalone entries.

use crate::{adapter, entry::BibtexEntry, error::Error, scanner};
use std::collections::BTreeSet;

/// Parse a file containing only entries and whitespace, in source order.
///
/// No metadata is projected, so an entry without a title parses successfully
/// and is resolved against a provider like any other. Duplicate source keys are
/// rejected: the file is ambiguous about which entry a key names, and bibi
/// would have to choose. One malformed entry fails the whole file; callers
/// One malformed entry or repeated texkey rejects the complete input.
pub fn parse_file(source: &str) -> Result<Vec<BibtexEntry>, Error> {
    let mut keys = BTreeSet::new();
    let mut entries = Vec::new();
    for span in scanner::split(source) {
        let raw = adapter::parse_entry(source, span)?;
        let payload = BibtexEntry::from_raw(source, raw);
        let key = payload.texkey().to_owned();
        if !keys.insert(key.clone()) {
            return Err(Error::DuplicateKey { key });
        }
        entries.push(payload);
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
            entries.iter().map(BibtexEntry::texkey).collect::<Vec<_>>(),
            ["Zed", "Alpha"]
        );
        assert_eq!(entries[0].source(), first);
        assert_eq!(entries[1].source(), second);
    }

    #[test]
    fn accepts_an_untitled_entry_that_carries_an_identifier() {
        let entries = parse_file("@article{A, doi = {10.1/x}}").unwrap();
        assert_eq!(
            entries[0].identifier_candidates().doi.as_deref(),
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
}
