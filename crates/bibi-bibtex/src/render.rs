//! Deterministic rendering of a sequence of entries.

use crate::entry::BibtexEntry;

/// Render entries as a standalone BibTeX document.
///
/// Exact entry sources are separated by one blank line, and a non-empty
/// document ends with one newline. An empty selection renders zero bytes.
///
/// Ordering and filtering belong to the caller: this function is a pure
/// function of the entries it is given. No metadata projection participates.
pub fn render<'a>(entries: impl IntoIterator<Item = &'a BibtexEntry>) -> String {
    let mut output = String::new();
    for entry in entries {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(entry.source());
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    #[test]
    fn separates_entries_with_one_blank_line_and_one_final_newline() {
        let first = entry("@misc{a,title={A}}");
        let second = entry("@misc{b,\n  title = {B}\n}");
        let rendered = render([&first, &second]);
        assert_eq!(
            rendered,
            "@misc{a,title={A}}\n\n@misc{b,\n  title = {B}\n}\n"
        );
    }

    #[test]
    fn renders_an_empty_selection_as_zero_bytes() {
        assert_eq!(render([]), "");
    }

    #[test]
    fn renders_one_entry_with_exactly_one_trailing_newline() {
        let only = entry("  @misc{a,title={A}}\n\n\n");
        assert_eq!(render([&only]), "@misc{a,title={A}}\n");
    }

    #[test]
    fn rendering_is_idempotent_over_its_own_output() {
        let source = "@article{x,\n  title = {A {NASA} result},\n  author = {Doe, Jane}\n}";
        let original = entry(source);
        let once = render([&original]);
        let reparsed = entry(&once);
        assert_eq!(render([&reparsed]), once);
    }
}
