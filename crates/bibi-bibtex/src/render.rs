//! Deterministic rendering of a sequence of entries.

use crate::{
    entry::{BibtexEntry, CitationKey},
    error::Error,
};

/// Render entries as a standalone BibTeX document.
///
/// Each entry is re-keyed to the key it is paired with, entries are separated
/// by exactly one blank line, and a non-empty document ends with exactly one
/// newline. An empty selection renders zero bytes.
///
/// Ordering and filtering belong to the caller: this function is a pure
/// function of the pairs it is given, so the same manifest and the same options
/// always produce the same bytes (I6). Nothing but the key and the stored
/// payload participates, so no description can reach a rendered artifact (I7).
pub fn render<'a>(
    entries: impl IntoIterator<Item = (&'a CitationKey, &'a BibtexEntry)>,
) -> Result<String, Error> {
    let mut output = String::new();
    for (key, entry) in entries {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&entry.rekey(key)?);
    }
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    fn key(value: &str) -> CitationKey {
        CitationKey::new(value).unwrap()
    }

    #[test]
    fn separates_entries_with_one_blank_line_and_one_final_newline() {
        let first = entry("@misc{a,title={A}}");
        let second = entry("@misc{b,\n  title = {B}\n}");
        let rendered = render([(&key("First"), &first), (&key("Second"), &second)]).unwrap();
        assert_eq!(
            rendered,
            "@misc{First,title={A}}\n\n@misc{Second,\n  title = {B}\n}\n"
        );
    }

    #[test]
    fn renders_an_empty_selection_as_zero_bytes() {
        assert_eq!(render([]).unwrap(), "");
    }

    #[test]
    fn renders_one_entry_with_exactly_one_trailing_newline() {
        let only = entry("  @misc{a,title={A}}\n\n\n");
        assert_eq!(
            render([(&key("Only"), &only)]).unwrap(),
            "@misc{Only,title={A}}\n"
        );
    }

    #[test]
    fn rendering_is_idempotent_over_its_own_output() {
        let source = "@article{x,\n  title = {A {NASA} result},\n  author = {Doe, Jane}\n}";
        let original = entry(source);
        let once = render([(&key("Key"), &original)]).unwrap();
        let reparsed = entry(&once);
        assert_eq!(render([(&key("Key"), &reparsed)]).unwrap(), once);
    }
}
