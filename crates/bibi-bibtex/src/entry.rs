//! Citation keys and validated standalone entries.

use crate::{
    error::Error,
    local_metadata::{self, LocalMetadata},
    scanner::{self, RawEntry, RawField},
};
use std::{fmt, ops::Range, str::FromStr};

/// A citation key in the safe grammar `[A-Za-z0-9._:+-]+`.
///
/// The grammar is deliberately narrower than BibTeX's, so that a key can always
/// be written back into an entry by replacing its token and nothing else.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CitationKey(String);

impl CitationKey {
    /// Validate and construct a citation key.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if scanner::is_safe_key(&value) {
            Ok(Self(value))
        } else {
            Err(Error::InvalidKey { key: value })
        }
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the key, returning its text.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for CitationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for CitationKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for CitationKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Identifiers read syntactically out of an entry's fields.
///
/// This is a *candidate* read: values are returned as written, and validating
/// or normalizing them belongs to the domain layer. It requires no field and
/// cannot fail, because import must be able to resolve an entry that carries a
/// DOI and nothing else.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdentifierCandidates {
    /// The `doi` field, if present.
    pub doi: Option<String>,
    /// The `eprint` field, if present and not disclaimed by an archive prefix
    /// naming something other than arXiv.
    pub arxiv: Option<String>,
}

/// One validated, standalone BibTeX entry with its bytes preserved.
///
/// [`BibtexEntry::source`] is the exact entry span, from `@` through the
/// matching closing delimiter. Whitespace outside that span belongs to the
/// response or file that contained the entry, not to the entry; every byte
/// inside it, including internal comments and formatting, is preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BibtexEntry {
    source: String,
    source_key: CitationKey,
    key_span: Range<usize>,
    /// Field spans relative to `source`, in source order.
    fields: Vec<RawField>,
}

impl BibtexEntry {
    /// Validate one standalone entry, which may be surrounded by whitespace.
    pub fn parse_one(source: String) -> Result<Self, Error> {
        let mut entries = scanner::scan(&source)?;
        if entries.len() != 1 {
            return Err(Error::NotStandalone {
                found: entries.len(),
            });
        }
        Ok(Self::from_raw(&source, entries.remove(0)))
    }

    /// Build an entry from a located span of a larger source.
    pub(crate) fn from_raw(source: &str, raw: RawEntry) -> Self {
        let start = raw.entry.start;
        let key_span = (raw.key.start - start)..(raw.key.end - start);
        let fields = raw
            .fields
            .into_iter()
            .map(|field| RawField {
                name: (field.name.start - start)..(field.name.end - start),
                value: (field.value.start - start)..(field.value.end - start),
            })
            .collect();
        let source = source[raw.entry].to_owned();
        let source_key = CitationKey(source[key_span.clone()].to_owned());
        Self {
            source,
            source_key,
            key_span,
            fields,
        }
    }

    /// The exact entry bytes, without surrounding whitespace.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The citation key as the source wrote it.
    pub fn source_key(&self) -> &CitationKey {
        &self.source_key
    }

    /// Render this entry with `key` in place of its source key.
    ///
    /// Only the key token changes. The result is re-scanned and compared with
    /// the original outside the key span, so a re-key that would have disturbed
    /// any other byte is refused rather than published.
    pub fn rekey(&self, key: &CitationKey) -> Result<String, Error> {
        let mut rekeyed = String::with_capacity(self.source.len() + key.as_str().len());
        rekeyed.push_str(&self.source[..self.key_span.start]);
        rekeyed.push_str(key.as_str());
        rekeyed.push_str(&self.source[self.key_span.end..]);
        let scanned = Self::parse_one(rekeyed.clone()).map_err(|error| Error::UnsafeRekey {
            key: key.as_str().to_owned(),
            reason: error.to_string(),
        })?;
        let unsafe_rekey = |reason: &str| Error::UnsafeRekey {
            key: key.as_str().to_owned(),
            reason: reason.to_owned(),
        };
        if scanned.source_key != *key {
            return Err(unsafe_rekey(
                "the written key is not the key that was read back",
            ));
        }
        if self.source[..self.key_span.start] != scanned.source[..scanned.key_span.start]
            || self.source[self.key_span.end..] != scanned.source[scanned.key_span.end..]
        {
            return Err(unsafe_rekey("bytes outside the citation key would change"));
        }
        Ok(rekeyed)
    }

    /// Read the DOI and arXiv identifiers this entry declares, if any.
    pub fn identifier_candidates(&self) -> IdentifierCandidates {
        let field = |name: &str| {
            self.fields
                .iter()
                .find(|field| self.source[field.name.clone()].eq_ignore_ascii_case(name))
                .map(|field| field_text(&self.source[field.value.clone()]))
                .filter(|value| !value.is_empty())
        };
        // An `archiveprefix` naming something other than arXiv means the eprint
        // is somebody else's identifier, so it is not offered as an arXiv id.
        let archive = field("archiveprefix").or_else(|| field("eprinttype"));
        let arxiv = field("eprint")
            .filter(|_| archive.is_none_or(|prefix| prefix.eq_ignore_ascii_case("arxiv")));
        IdentifierCandidates {
            doi: field("doi"),
            arxiv,
        }
    }

    /// Project full metadata from an entry that is itself the original source.
    ///
    /// This is the only semantic read in the crate, and it exists for the local
    /// provider: where the user supplied the BibTeX, deriving metadata from it
    /// interprets nothing that a structured record would have said better.
    pub fn local_metadata(&self) -> Result<LocalMetadata, Error> {
        local_metadata::project(
            &self.source,
            self.source_key.as_str(),
            self.identifier_candidates(),
        )
    }
}

/// Strip one layer of value delimiters and surrounding whitespace.
///
/// A concatenated or macro value is returned as written; deciding whether it is
/// a usable identifier belongs to the domain layer, which validates it anyway.
fn field_text(value: &str) -> String {
    let value = value.trim();
    if is_concatenated(value) {
        return value.to_owned();
    }
    let inner = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(value);
    inner.trim().to_owned()
}

/// True when a `#` concatenates at the value's top level.
///
/// Delimiter stripping is only safe on one whole delimited token: `{10.1/x} #
/// {junk}` starts with `{` and ends with `}` but is two tokens, and stripping
/// would corrupt it. A `#` inside braces or quotes is literal text.
fn is_concatenated(value: &str) -> bool {
    let mut depth = 0usize;
    let mut quoted = false;
    for byte in value.bytes() {
        match byte {
            _ if quoted => {
                if byte == b'"' {
                    quoted = false;
                }
            }
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'"' if depth == 0 => quoted = true,
            b'#' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> CitationKey {
        CitationKey::new(value).unwrap()
    }

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    #[test]
    fn citation_keys_accept_only_the_safe_grammar() {
        for value in ["Aad:2012tfa", "a", "A.b_c:d+e-f", "2012"] {
            assert!(CitationKey::new(value).is_ok(), "{value}");
        }
        for value in ["", "a b", "a{b", "a/b", "café"] {
            assert!(CitationKey::new(value).is_err(), "{value}");
        }
    }

    #[test]
    fn parse_one_keeps_the_entry_span_and_drops_surrounding_whitespace() {
        let entry = entry("\n  @misc{A, title = {T} }\n\n");
        assert_eq!(entry.source(), "@misc{A, title = {T} }");
        assert_eq!(entry.source_key().as_str(), "A");
    }

    #[test]
    fn parse_one_rejects_zero_or_several_entries() {
        assert!(matches!(
            BibtexEntry::parse_one("   ".to_owned()),
            Err(Error::NotStandalone { found: 0 })
        ));
        assert!(matches!(
            BibtexEntry::parse_one("@misc{A,title={T}}\n@misc{B,title={T}}".to_owned()),
            Err(Error::NotStandalone { found: 2 })
        ));
    }

    #[test]
    fn rekey_changes_only_the_key_token() {
        let original = entry("@misc{Old, title = {Old is not a key} }");
        assert_eq!(
            original.rekey(&key("New")).unwrap(),
            "@misc{New, title = {Old is not a key} }"
        );
        // Re-keying is a pure function of the stored entry, so it is repeatable.
        assert_eq!(
            original.rekey(&key("Old")).unwrap(),
            original.source().to_owned()
        );
    }

    #[test]
    fn identifier_candidates_read_fields_without_requiring_a_title() {
        let untitled = entry("@article{A, doi = {10.1/ABC}, eprint = \"1207.7214\"}");
        let candidates = untitled.identifier_candidates();
        assert_eq!(candidates.doi.as_deref(), Some("10.1/ABC"));
        assert_eq!(candidates.arxiv.as_deref(), Some("1207.7214"));
        assert!(untitled.local_metadata().is_err());
    }

    #[test]
    fn identifier_candidates_are_case_insensitive_and_tolerate_absence() {
        let entry = entry("@misc{A, DOI = {10.1/x}, Title = {T}}");
        assert_eq!(entry.identifier_candidates().doi.as_deref(), Some("10.1/x"));
        assert_eq!(entry.identifier_candidates().arxiv, None);
        assert_eq!(
            self::entry("@misc{A}").identifier_candidates(),
            IdentifierCandidates::default()
        );
    }

    #[test]
    fn a_foreign_archive_prefix_withholds_the_eprint() {
        let foreign = entry("@misc{A, eprint = {ADS:1}, archiveprefix = {ADS}}");
        assert_eq!(foreign.identifier_candidates().arxiv, None);
        let arxiv = entry("@misc{A, eprint = {1207.7214}, archivePrefix = {arXiv}}");
        assert_eq!(
            arxiv.identifier_candidates().arxiv.as_deref(),
            Some("1207.7214")
        );
    }

    #[test]
    fn concatenated_values_are_returned_as_written() {
        // Two tokens joined by `#` are not one delimited value; stripping the
        // outer braces would corrupt them.
        let concatenated = entry("@misc{A, doi = {10.1/x} # {junk}}");
        assert_eq!(
            concatenated.identifier_candidates().doi.as_deref(),
            Some("{10.1/x} # {junk}")
        );
        let quoted = entry("@misc{A, doi = \"10.1/x\" # {junk}}");
        assert_eq!(
            quoted.identifier_candidates().doi.as_deref(),
            Some("\"10.1/x\" # {junk}")
        );
        // A `#` inside one delimited token is literal text, not concatenation.
        let literal = entry("@misc{A, doi = {10.1/x # y}}");
        assert_eq!(
            literal.identifier_candidates().doi.as_deref(),
            Some("10.1/x # y")
        );
    }

    #[test]
    fn field_spans_survive_a_rekey() {
        let original = entry("@misc{Old, doi = {10.1/x}, eprint = {1207.7214}}");
        let rekeyed = BibtexEntry::parse_one(original.rekey(&key("Longer")).unwrap()).unwrap();
        assert_eq!(
            rekeyed.identifier_candidates(),
            original.identifier_candidates()
        );
    }
}
