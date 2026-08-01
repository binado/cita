//! The byte-level splitter that owns entry boundaries, plus the safe
//! citation-key grammar.
//!
//! Key and field grammar are no longer bibi's to define: [`crate::adapter`]
//! hands each located slice to `biblatex`'s raw parser and adopts its
//! decisions. What stays here is the one thing a per-entry parser cannot do
//! for itself — finding where one entry ends and the next begins in a file
//! that may hold several — plus [`is_safe_key`], which is not a grammar
//! opinion but a write-then-read invariant: a key that cannot be written back
//! into an entry by replacing its token and nothing else must never reach the
//! manifest (see [`crate::entry::CitationKey`] and `rekey`).
//!
//! Scanning walks bytes rather than characters. Every delimiter it cares
//! about is ASCII, and a UTF-8 continuation byte is never an ASCII byte, so a
//! multi-byte character can never be mistaken for a delimiter and every
//! recorded offset lands on a character boundary.

use std::ops::Range;

/// Return true when `key` matches the safe citation-key grammar.
pub(crate) fn is_safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'+' | b'-')
        })
}

/// Byte ranges of top-level entries, each starting at `@`.
///
/// A range runs from `@` through the matching closing delimiter, inclusive.
/// Bytes that are not part of any such range — stray text, top-level `%`
/// comments, whitespace — are skipped without being classified: grammar
/// questions belong to `biblatex`, one entry at a time, not to this splitter.
///
/// An entry whose delimiter never closes still gets a span, running to the
/// end of the source, so that a truncated entry becomes a parse failure for
/// whoever consumes it rather than silently vanishing from the count.
pub(crate) fn split(source: &str) -> Vec<Range<usize>> {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut spans = Vec::new();
    while let Some(offset) = bytes[at..].iter().position(|&byte| byte == b'@') {
        let start = at + offset;
        match entry_end(bytes, start) {
            Some(end) => {
                spans.push(start..end);
                at = end;
            }
            // Not shaped like an entry at all: step past the `@` and keep
            // looking, rather than treating it as this file's problem.
            None => at = start + 1,
        }
    }
    spans
}

/// Find where the entry starting at `bytes[start]` (`@`) ends, if it opens
/// with a recognizable delimiter.
fn entry_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        at += 1;
    }
    at = skip_trivia(bytes, at);
    let close = match bytes.get(at)? {
        b'{' => b'}',
        b'(' => b')',
        _ => return None,
    };
    Some(body_end(bytes, at + 1, close))
}

/// From just past an entry's opening delimiter, find the index just past its
/// `close`, skipping nested `{...}` groups, `"..."` strings, and `%`-to-newline
/// comments so that none of their bytes are mistaken for the entry's own
/// close. A comment containing `}` is exactly why `%` must be tracked here
/// rather than left as ordinary bytes.
fn body_end(bytes: &[u8], mut at: usize, close: u8) -> usize {
    loop {
        match bytes.get(at) {
            None => return at,
            Some(&byte) if byte == close => return at + 1,
            Some(b'{') => at = skip_delimited(bytes, at),
            Some(b'"') => at = skip_quoted(bytes, at),
            Some(b'%') => {
                while bytes.get(at).is_some_and(|&byte| byte != b'\n') {
                    at += 1;
                }
            }
            Some(_) => at += 1,
        }
    }
}

/// Skip a balanced `{...}` group starting at `bytes[at]` (`{`).
fn skip_delimited(bytes: &[u8], mut at: usize) -> usize {
    let mut depth = 0usize;
    loop {
        match bytes.get(at) {
            None => return at,
            Some(b'{') => {
                depth += 1;
                at += 1;
            }
            Some(b'}') => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return at;
                }
            }
            Some(_) => at += 1,
        }
    }
}

/// Skip a `"..."` string starting at `bytes[at]` (`"`). A brace group inside
/// protects its quotes: a `"` at nonzero depth is literal.
fn skip_quoted(bytes: &[u8], mut at: usize) -> usize {
    at += 1; // opening quote
    let mut depth = 0usize;
    loop {
        match bytes.get(at) {
            None => return at,
            Some(b'{') => {
                depth += 1;
                at += 1;
            }
            Some(b'}') => {
                depth = depth.saturating_sub(1);
                at += 1;
            }
            Some(b'"') if depth == 0 => return at + 1,
            Some(_) => at += 1,
        }
    }
}

/// Skip whitespace and `%` comments between an entry's type and its opening
/// delimiter.
fn skip_trivia(bytes: &[u8], mut at: usize) -> usize {
    loop {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if bytes.get(at) != Some(&b'%') {
            return at;
        }
        while bytes.get(at).is_some_and(|&byte| byte != b'\n') {
            at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(source: &str) -> Vec<&str> {
        split(source)
            .into_iter()
            .map(|span| &source[span])
            .collect()
    }

    #[test]
    fn locates_entries_in_source_order() {
        let source = "\n@misc{Zed, title = {Z} }\n\n@article{Alpha,title={A}}\n";
        assert_eq!(
            spans(source),
            ["@misc{Zed, title = {Z} }", "@article{Alpha,title={A}}"]
        );
    }

    #[test]
    fn accepts_paren_delimiters_and_nested_braces_and_quotes() {
        assert_eq!(
            spans("@misc(A, title = {a {B} c})"),
            ["@misc(A, title = {a {B} c})"]
        );
        assert_eq!(
            spans(r#"@misc{A, title = "a {\"b\"} c"}"#),
            [r#"@misc{A, title = "a {\"b\"} c"}"#]
        );
        // A brace group inside a quoted value protects its quotes.
        assert_eq!(
            spans(r#"@misc{A, title = "say {"} hi"}"#),
            [r#"@misc{A, title = "say {"} hi"}"#]
        );
    }

    #[test]
    fn treats_percent_as_trivia_inside_an_entry_only() {
        let source = "@misc{A,\n  title = {T},  % note\n  year = 2025\n}";
        assert_eq!(spans(source), [source]);
    }

    #[test]
    fn skips_non_entry_content_between_entries_silently() {
        let source =
            "% leading notes\n@misc{A,title={T}}\nsome stray text\n@misc{B,title={U}}\n% trailing";
        assert_eq!(spans(source), ["@misc{A,title={T}}", "@misc{B,title={U}}"]);
    }

    #[test]
    fn an_unterminated_entry_still_gets_a_span_to_end_of_source() {
        // A truncated entry must not silently vanish from the count: it gets
        // a span reaching to EOF, so the adapter can report it as a failure
        // rather than dropping it without a trace.
        let source = "@misc{A,title={T}";
        assert_eq!(spans(source), [source]);
        let two = "@misc{A,title={T}\n@misc{B,title={U}}";
        // The unterminated first entry swallows the rest of the source: there
        // is no reliable way to tell where it "should" have ended.
        assert_eq!(spans(two), [two]);
    }

    #[test]
    fn keeps_offsets_on_character_boundaries_with_multibyte_content() {
        let source = "@misc{A,\n  title = {Schrödinger — “quoted”},\n  note = {é}\n}";
        assert_eq!(spans(source), [source]);
    }

    #[test]
    fn is_safe_key_accepts_only_the_narrow_grammar() {
        for value in ["Aad:2012tfa", "a", "A.b_c:d+e-f", "2012"] {
            assert!(is_safe_key(value), "{value}");
        }
        for value in ["", "a b", "a{b", "a/b", "café"] {
            assert!(!is_safe_key(value), "{value}");
        }
    }
}
