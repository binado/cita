//! Validated standalone entries with exact bytes and a derived texkey.

use crate::{
    adapter::{self, RawEntry, RawField},
    error::Error,
    local_metadata::{self, LocalMetadata},
    scanner,
};

/// Identifiers read syntactically out of an entry's fields.
///
/// This is a *candidate* read: values are returned as written, and validating
/// or normalizing them belongs to the domain layer. It requires no field and
/// cannot fail, because import must be able to resolve an entry that carries a
/// DOI and nothing else.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct IdentifierCandidates {
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
pub(crate) struct BibtexEntry {
    source: String,
    texkey: String,
    /// Field spans relative to `source`, in source order.
    fields: Vec<RawField>,
}

impl BibtexEntry {
    /// Convert the validated entry into the parser-agnostic core payload.
    pub(crate) fn into_core(self) -> bibi_core::Bibtex {
        bibi_core::Bibtex::new(self.source, self.texkey)
    }

    /// Validate one standalone entry, which may be surrounded by whitespace.
    pub(crate) fn parse_one(source: String) -> Result<Self, Error> {
        let mut spans = scanner::split(&source);
        if spans.len() != 1 {
            return Err(Error::NotStandalone { found: spans.len() });
        }
        let raw = adapter::parse_entry(&source, spans.remove(0))?;
        Ok(Self::from_raw(&source, raw))
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
        let texkey = source[key_span].to_owned();
        Self {
            source,
            texkey,
            fields,
        }
    }

    /// The exact entry bytes, without surrounding whitespace.
    #[allow(dead_code)]
    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// The texkey embedded in the exact source.
    #[allow(dead_code)]
    pub(crate) fn texkey(&self) -> &str {
        &self.texkey
    }

    /// Read the DOI and arXiv identifiers this entry declares, if any.
    pub(crate) fn identifier_candidates(&self) -> IdentifierCandidates {
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
    pub(crate) fn local_metadata(&self) -> Result<LocalMetadata, Error> {
        local_metadata::project(&self.source, &self.texkey, self.identifier_candidates())
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

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    #[test]
    fn parse_one_keeps_the_entry_span_and_drops_surrounding_whitespace() {
        let entry = entry("\n  @misc{A, title = {T} }\n\n");
        assert_eq!(entry.source(), "@misc{A, title = {T} }");
        assert_eq!(entry.texkey(), "A");
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
    fn adopts_keys_that_need_never_be_rewritten() {
        assert_eq!(entry("@misc{a/b,title={T}}").texkey(), "a/b");
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
            self::entry("@misc{A,}").identifier_candidates(),
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
    fn field_spans_and_texkey_are_derived_together() {
        let original = entry("@misc{Old, doi = {10.1/x}, eprint = {1207.7214}}");
        assert_eq!(original.texkey(), "Old");
        assert_eq!(
            original.identifier_candidates().doi.as_deref(),
            Some("10.1/x")
        );
    }
}
