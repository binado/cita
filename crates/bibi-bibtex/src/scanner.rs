//! The byte-level scanner that owns entry boundaries, key spans, and field
//! spans.
//!
//! Nothing here interprets BibTeX as bibliographic metadata: the scanner
//! locates structure and reports byte ranges, and every public operation that
//! preserves bytes is expressed as a range of the original source. Semantic
//! parsing exists only in [`crate::local_metadata`], where the entry is the
//! original rather than a rendering of something else.
//!
//! Scanning walks bytes rather than characters. Every delimiter the grammar
//! cares about is ASCII, and a UTF-8 continuation byte is never an ASCII byte,
//! so a multi-byte character can never be mistaken for a delimiter and every
//! recorded offset lands on a character boundary.

use crate::error::Error;
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

/// Directives carry global state that a standalone entry cannot reproduce.
const DIRECTIVES: [&str; 3] = ["string", "preamble", "comment"];

/// Locate every entry in `source`, rejecting anything else.
pub(crate) fn scan(source: &str) -> Result<Vec<RawEntry>, Error> {
    Scanner::new(source).run()
}

/// Return true when `key` matches the safe citation-key grammar.
pub(crate) fn is_safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'+' | b'-')
        })
}

struct Scanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            at: 0,
        }
    }

    fn run(mut self) -> Result<Vec<RawEntry>, Error> {
        let mut entries = Vec::new();
        loop {
            self.skip_whitespace();
            let Some(byte) = self.peek() else {
                return Ok(entries);
            };
            if byte != b'@' {
                return Err(Error::UnexpectedContent { offset: self.at });
            }
            entries.push(self.entry()?);
        }
    }

    fn entry(&mut self) -> Result<RawEntry, Error> {
        let start = self.at;
        self.at += 1; // `@`
        let kind_start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            self.at += 1;
        }
        let kind = &self.source[kind_start..self.at];
        if kind.is_empty() {
            return Err(self.malformed("an entry type must follow `@`"));
        }
        if DIRECTIVES
            .iter()
            .any(|directive| kind.eq_ignore_ascii_case(directive))
        {
            return Err(Error::UnsupportedDirective {
                directive: kind.to_owned(),
                offset: start,
            });
        }
        self.skip_trivia();
        let close = match self.peek() {
            Some(b'{') => b'}',
            Some(b'(') => b')',
            _ => return Err(self.malformed("an entry type must be followed by `{` or `(`")),
        };
        self.at += 1;
        let key = self.key(close)?;
        let fields = self.fields(close)?;
        // `fields` stops on the closing delimiter, which is the entry's last byte.
        debug_assert_eq!(self.peek(), Some(close));
        self.at += 1;
        Ok(RawEntry {
            entry: start..self.at,
            key,
            fields,
        })
    }

    /// Read the citation key that opens an entry body.
    ///
    /// The token runs to the comma or closing delimiter rather than to the
    /// first space, so that `@misc{bad key,...}` is reported as the unsafe key
    /// it is instead of as a structural break several bytes later. Trailing
    /// whitespace is excluded from the span, which is what keeps a re-key from
    /// swallowing it.
    fn key(&mut self, close: u8) -> Result<Range<usize>, Error> {
        self.skip_trivia();
        let start = self.at;
        while let Some(byte) = self.peek() {
            if byte == close
                || matches!(
                    byte,
                    b',' | b'=' | b'{' | b'}' | b'(' | b')' | b'"' | b'%' | b'#'
                )
            {
                break;
            }
            self.at += 1;
        }
        // The key grammar is ASCII, so only ASCII whitespace is trimmed here:
        // a trailing NBSP must stay in the span and fail the safe-key check,
        // not vanish into a recorded key that differs from the file's bytes.
        let end = start
            + self.source[start..self.at]
                .trim_end_matches(|c: char| c.is_ascii_whitespace())
                .len();
        let key = &self.source[start..end];
        if !is_safe_key(key) {
            return Err(Error::InvalidKey {
                key: key.to_owned(),
            });
        }
        let span = start..end;
        self.skip_trivia();
        match self.peek() {
            Some(byte) if byte == close => Ok(span),
            Some(b',') => {
                self.at += 1;
                Ok(span)
            }
            _ => {
                Err(self
                    .malformed("a citation key must be followed by `,` or the closing delimiter"))
            }
        }
    }

    /// Read fields until the entry's closing delimiter, which is left unconsumed.
    fn fields(&mut self, close: u8) -> Result<Vec<RawField>, Error> {
        let mut fields = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.malformed("entry has no closing delimiter")),
                // A trailing or repeated comma introduces no field.
                Some(b',') => {
                    self.at += 1;
                    continue;
                }
                Some(byte) if byte == close => return Ok(fields),
                Some(_) => {}
            }
            let name_start = self.at;
            while let Some(byte) = self.peek() {
                if byte.is_ascii_whitespace()
                    || matches!(
                        byte,
                        b'=' | b',' | b'{' | b'}' | b'(' | b')' | b'"' | b'#' | b'%'
                    )
                {
                    break;
                }
                self.at += 1;
            }
            if self.at == name_start {
                return Err(self.malformed("expected a field name"));
            }
            let name = name_start..self.at;
            self.skip_trivia();
            if self.peek() != Some(b'=') {
                return Err(self.malformed("a field name must be followed by `=`"));
            }
            self.at += 1;
            let value = self.value()?;
            fields.push(RawField { name, value });
        }
    }

    /// Read one field value: a `#`-concatenated sequence of tokens.
    fn value(&mut self) -> Result<Range<usize>, Error> {
        self.skip_trivia();
        let start = self.at;
        let mut end;
        loop {
            end = self.token()?;
            let mark = self.at;
            self.skip_trivia();
            if self.peek() == Some(b'#') {
                self.at += 1;
                self.skip_trivia();
                continue;
            }
            self.at = mark;
            break;
        }
        Ok(start..end)
    }

    /// Read one value token, returning the offset just past it.
    fn token(&mut self) -> Result<usize, Error> {
        match self.peek() {
            Some(b'{') => self.delimited(b'{', b'}'),
            Some(b'"') => self.quoted(),
            Some(byte)
                if !byte.is_ascii_whitespace() && !matches!(byte, b',' | b'}' | b')' | b'#') =>
            {
                while let Some(byte) = self.peek() {
                    if byte.is_ascii_whitespace()
                        || matches!(byte, b',' | b'}' | b')' | b'#' | b'%')
                    {
                        break;
                    }
                    self.at += 1;
                }
                Ok(self.at)
            }
            _ => Err(self.malformed("expected a field value")),
        }
    }

    /// Read a balanced `{...}` group. Braces nest; quotes inside are literal.
    fn delimited(&mut self, open: u8, close: u8) -> Result<usize, Error> {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == open {
                depth += 1;
            } else if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(self.at);
                }
            }
        }
        Err(self.malformed("unbalanced braces in a field value"))
    }

    /// Read a `"..."` string. A quote inside a brace group is literal, which is
    /// how BibTeX protects a quoted value containing quotation marks.
    fn quoted(&mut self) -> Result<usize, Error> {
        self.at += 1; // opening quote
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            self.at += 1;
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| self.malformed("unbalanced braces in a quoted value"))?;
                }
                b'"' if depth == 0 => return Ok(self.at),
                _ => {}
            }
        }
        Err(self.malformed("unterminated quoted value"))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    /// Skip whitespace and `%` comments, which are only trivia *inside* an
    /// entry. Between entries a `%` is unsupported content, because a file bibi
    /// re-reads must contain nothing it would have to reproduce.
    fn skip_trivia(&mut self) {
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'%') {
                return;
            }
            while self.peek().is_some_and(|byte| byte != b'\n') {
                self.at += 1;
            }
        }
    }

    fn malformed(&self, reason: &str) -> Error {
        Error::MalformedEntry {
            offset: self.at,
            reason: reason.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(source: &str) -> Vec<&str> {
        scan(source)
            .unwrap()
            .into_iter()
            .map(|entry| &source[entry.key])
            .collect()
    }

    fn spans(source: &str) -> Vec<&str> {
        scan(source)
            .unwrap()
            .into_iter()
            .map(|entry| &source[entry.entry])
            .collect()
    }

    #[test]
    fn locates_entries_and_keys_in_source_order() {
        let source = "\n@misc{Zed, title = {Z} }\n\n@article{Alpha,title={A}}\n";
        assert_eq!(keys(source), ["Zed", "Alpha"]);
        assert_eq!(
            spans(source),
            ["@misc{Zed, title = {Z} }", "@article{Alpha,title={A}}"]
        );
    }

    #[test]
    fn accepts_paren_delimiters_nested_braces_and_quotes() {
        assert_eq!(keys("@misc(A, title = {a {B} c})"), ["A"]);
        assert_eq!(keys(r#"@misc{A, title = "a {\"b\"} c"}"#), ["A"]);
        // A brace group inside a quoted value protects its quotes.
        assert_eq!(keys(r#"@misc{A, title = "say {"} hi"}"#), ["A"]);
        assert_eq!(keys("@misc{A, month = jan # \" 2025\"}"), ["A"]);
        assert_eq!(keys("@misc{A, year = 2025}"), ["A"]);
        assert_eq!(keys("@misc{A, title = {T},}"), ["A"]);
        assert_eq!(keys("@misc{A}"), ["A"]);
    }

    #[test]
    fn treats_percent_as_trivia_inside_an_entry_only() {
        let source = "@misc{A,\n  title = {T},  % note\n  year = 2025\n}";
        assert_eq!(spans(source), [source]);
        assert!(matches!(
            scan("% leading\n@misc{A,title={T}}"),
            Err(Error::UnexpectedContent { offset: 0 })
        ));
        assert!(matches!(
            scan("@misc{A,title={T}}\n% trailing"),
            Err(Error::UnexpectedContent { .. })
        ));
    }

    #[test]
    fn records_field_spans_for_every_field() {
        let source = "@misc{A,\n  title = {T},\n  doi = \"10.1/x\"\n}";
        let entry = &scan(source).unwrap()[0];
        let fields = entry
            .fields
            .iter()
            .map(|field| (&source[field.name.clone()], &source[field.value.clone()]))
            .collect::<Vec<_>>();
        assert_eq!(fields, [("title", "{T}"), ("doi", "\"10.1/x\"")]);
    }

    #[test]
    fn rejects_directives_and_unsupported_content() {
        assert!(matches!(
            scan("@string{x = {y}}"),
            Err(Error::UnsupportedDirective { .. })
        ));
        assert!(matches!(
            scan("@preamble{\"x\"}"),
            Err(Error::UnsupportedDirective { .. })
        ));
        assert!(matches!(
            scan("@COMMENT{x}"),
            Err(Error::UnsupportedDirective { .. })
        ));
        assert!(matches!(
            scan("@misc{A,title={T}} trailing"),
            Err(Error::UnexpectedContent { .. })
        ));
    }

    #[test]
    fn rejects_broken_structure() {
        for source in [
            "@misc{A,title={T}",     // no closing delimiter
            "@misc{A,title={T}",     // unbalanced group
            "@misc{A,title=}",       // no value
            "@misc{A,=}",            // no field name
            "@misc{A title={T}}",    // key not terminated
            "@{A,title={T}}",        // no entry type
            "@misc A,title={T}}",    // no opening delimiter
            "@misc{A,title=\"open}", // unterminated quote
        ] {
            assert!(scan(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_keys_outside_the_safe_grammar() {
        for source in [
            "@misc{bad key,title={T}}",
            "@misc{,title={T}}",
            "@misc{a/b}",
            // Trailing non-ASCII whitespace is part of the key, not trivia:
            // trimming it would record a key the file's bytes do not contain.
            "@misc{A\u{a0},title={T}}",
        ] {
            assert!(
                matches!(scan(source), Err(Error::InvalidKey { .. })),
                "{source}"
            );
        }
    }

    #[test]
    fn keeps_offsets_on_character_boundaries_with_multibyte_content() {
        let source = "@misc{A,\n  title = {Schrödinger — “quoted”},\n  note = {é}\n}";
        assert_eq!(spans(source), [source]);
    }
}
