//! Converts one bibi-located entry slice into key and field spans.
//!
//! This is the only place `biblatex`'s raw grammar is consulted for structure.
//! [`crate::scanner::split`] has already decided where the entry begins and
//! ends; what `biblatex` decides here is only what counts as the citation key
//! and the fields inside that slice. A slice `biblatex` rejects — or that
//! yields anything other than exactly one entry, such as an `@string` or
//! `@comment` directive, which `biblatex` parses as something other than an
//! entry — becomes this one entry's failure, never the whole file's.

use crate::{error::Error, scanner};
use biblatex::RawBibliography;
use std::ops::Range;

/// One located entry, described entirely by byte ranges into the source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawEntry {
    /// From `@` through the matching closing delimiter, inclusive.
    pub(crate) entry: Range<usize>,
    /// The citation-key token, exclusive of surrounding whitespace.
    pub(crate) key: Range<usize>,
    /// Fields in source order.
    pub(crate) fields: Vec<RawField>,
}

/// One located `name = value` field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawField {
    /// The field name token.
    pub(crate) name: Range<usize>,
    /// The complete value, including delimiters and any concatenation.
    pub(crate) value: Range<usize>,
}

/// Parse one entry slice, as located by [`scanner::split`], into a
/// [`RawEntry`] whose spans are relative to `source` rather than the slice.
pub(crate) fn parse_entry(source: &str, span: Range<usize>) -> Result<RawEntry, Error> {
    let slice = &source[span.clone()];
    let parsed = RawBibliography::parse(slice).map_err(|error| Error::InvalidGrammar {
        offset: span.start + error.span.start,
        message: error.to_string(),
    })?;
    let [entry] =
        <[_; 1]>::try_from(parsed.entries).map_err(|entries: Vec<_>| Error::InvalidGrammar {
            offset: span.start,
            message: format!("expected exactly one entry, found {}", entries.len()),
        })?;
    let raw = entry.v;
    let key = rebase(span.start, raw.key.span);
    let key_text = &source[key.clone()];
    if !scanner::is_safe_key(key_text) {
        return Err(Error::InvalidKey {
            key: key_text.to_owned(),
        });
    }
    let fields = raw
        .fields
        .into_iter()
        .map(|pair| RawField {
            name: rebase(span.start, pair.key.span),
            // `biblatex` includes trailing whitespace after a value's last
            // token in its span (it does not backtrack the whitespace it eats
            // while checking for a `#` continuation); trimming here keeps the
            // recorded span exact, matching every other span in this crate.
            value: trim_trailing_ws(source, rebase(span.start, pair.value.span)),
        })
        .collect();
    Ok(RawEntry {
        entry: span,
        key,
        fields,
    })
}

/// Shift a span reported relative to a slice back to the full source.
fn rebase(offset: usize, span: Range<usize>) -> Range<usize> {
    (offset + span.start)..(offset + span.end)
}

/// Shrink a span to exclude trailing ASCII whitespace.
fn trim_trailing_ws(source: &str, span: Range<usize>) -> Range<usize> {
    let end = span.start
        + source[span.clone()]
            .trim_end_matches(|byte: char| byte.is_ascii_whitespace())
            .len();
    span.start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> RawEntry {
        let span = scanner::split(source).into_iter().next().unwrap();
        parse_entry(source, span).unwrap()
    }

    #[test]
    fn extracts_the_key_and_field_spans() {
        let source = "@misc{A,\n  title = {T},\n  doi = \"10.1/x\"\n}";
        let raw = entry(source);
        assert_eq!(&source[raw.key.clone()], "A");
        let fields = raw
            .fields
            .iter()
            .map(|field| (&source[field.name.clone()], &source[field.value.clone()]))
            .collect::<Vec<_>>();
        assert_eq!(fields, [("title", "{T}"), ("doi", "\"10.1/x\"")]);
    }

    #[test]
    fn rejects_keys_outside_the_safe_grammar() {
        // These reach `is_safe_key`: `biblatex`'s own key grammar accepts an
        // empty key and a key with an internal `/`, so bibi's narrower check
        // is what actually refuses them.
        for source in ["@misc{,title={T}}", "@misc{a/b,}"] {
            let span = scanner::split(source).into_iter().next().unwrap();
            assert!(
                matches!(parse_entry(source, span), Err(Error::InvalidKey { .. })),
                "{source}"
            );
        }
    }

    #[test]
    fn a_key_containing_whitespace_fails_at_the_grammar_stage() {
        // `biblatex` requires a comma directly after the key token, and its
        // key grammar excludes whitespace, so "bad key" is rejected before
        // bibi's own check ever sees it — a diagnostic-quality regression
        // decision 3 accepted rather than working around.
        let source = "@misc{bad key,title={T}}";
        let span = scanner::split(source).into_iter().next().unwrap();
        assert!(matches!(
            parse_entry(source, span),
            Err(Error::InvalidGrammar { .. })
        ));
    }

    #[test]
    fn a_slice_biblatex_rejects_fails_only_that_entry() {
        // `biblatex`'s raw parser requires a comma after the key.
        let source = "@misc{A}";
        let span = scanner::split(source).into_iter().next().unwrap();
        assert!(matches!(
            parse_entry(source, span),
            Err(Error::InvalidGrammar { .. })
        ));
    }

    #[test]
    fn a_directive_produces_no_entry_and_fails() {
        for source in ["@string{x = {y}}", "@preamble{\"x\"}", "@comment{x}"] {
            let span = scanner::split(source).into_iter().next().unwrap();
            assert!(
                matches!(parse_entry(source, span), Err(Error::InvalidGrammar { .. })),
                "{source}"
            );
        }
    }
}
