//! Strict standalone BibTeX parsing and rendering support.
#![warn(missing_docs)]

use bibi_core::{
    Identifiers, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use biblatex::{
    Bibliography, ChunksExt, DateValue, Entry as BibEntry, PermissiveType, RawBibliography,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Validated standalone BibTeX entry text.
pub struct BibtexString {
    /// The complete raw BibTeX entry.
    pub bibtex: String,
}

impl BibtexString {
    /// Validate and construct entry text containing exactly one entry.
    pub fn new(bibtex: String) -> Result<Self, Error> {
        let entries = parse(&bibtex)?;
        if entries.len() != 1 {
            return Err(Error::InvalidBibtex(format!(
                "expected one standalone entry, found {}",
                entries.len()
            )));
        }
        Ok(Self { bibtex })
    }

    /// Return the entry's citation key.
    pub fn key(&self) -> Result<String, Error> {
        Ok(scan_raw_entries(&self.bibtex)?
            .into_iter()
            .map(|entry| entry.key)
            .next()
            .expect("BibtexString validation requires one entry"))
    }
}

impl ReferenceSource for BibtexString {
    fn project(&self) -> Result<Reference, ProjectionError> {
        project_bibtex(&self.bibtex).map_err(|error| ProjectionError::Invalid(error.to_string()))
    }
}

#[derive(Debug, Error)]
/// Error produced while validating, parsing, projecting, or rewriting BibTeX.
pub enum Error {
    /// BibTeX syntax or required semantic content is invalid.
    #[error("invalid BibTeX: {0}")]
    InvalidBibtex(String),
    /// The input contains content other than standalone entries and whitespace.
    #[error("unsupported bibliography content: {0}")]
    UnsupportedContent(String),
    /// A citation key contains unsupported characters.
    #[error("unsafe citation key `{0}`; allowed characters are A-Z, a-z, 0-9, ., _, :, +, and -")]
    UnsafeKey(String),
    /// More than one entry uses the same citation key.
    #[error("citation key conflict: `{0}` appears more than once")]
    KeyConflict(String),
}

/// Parse every standalone entry while retaining its exact raw entry block.
pub fn parse(source: &str) -> Result<BTreeMap<String, BibtexString>, Error> {
    let raw_entries = scan_raw_entries(source)?;
    let semantic =
        Bibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let mut entries = BTreeMap::new();
    for raw_entry in raw_entries {
        let key = raw_entry.key;
        let parsed = semantic
            .get(&key)
            .ok_or_else(|| Error::InvalidBibtex(format!("could not parse entry `{key}`")))?;
        validate_semantic(parsed)?;
        let entry = BibtexString {
            bibtex: source[raw_entry.entry_range].to_owned(),
        };
        let previous = entries.insert(key, entry);
        debug_assert!(previous.is_none(), "scanner rejects duplicate keys");
    }
    if semantic.len() != entries.len() {
        return Err(Error::UnsupportedContent(
            "macros requiring global context are not supported".into(),
        ));
    }
    Ok(entries)
}

/// Project one standalone entry into bibi's source-neutral reference fields.
pub fn project_bibtex(source: &str) -> Result<Reference, Error> {
    let raw = scan_raw_entries(source)?;
    if raw.len() != 1 {
        return Err(Error::InvalidBibtex(format!(
            "expected one standalone entry, found {}",
            raw.len()
        )));
    }
    let key = &raw[0].key;
    let semantic =
        Bibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let entry = semantic
        .get(key)
        .ok_or_else(|| Error::InvalidBibtex(format!("could not parse entry `{key}`")))?;
    validate_semantic(entry)?;
    let title = entry
        .title()
        .map(ChunksExt::format_verbatim)
        .map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let authors = entry
        .author()
        .unwrap_or_default()
        .iter()
        .map(format_person)
        .collect();
    let year = entry.date().ok().and_then(|date| match date {
        PermissiveType::Typed(date) => Some(match date.value {
            DateValue::At(value) | DateValue::After(value) | DateValue::Before(value) => value.year,
            DateValue::Between(value, _) => value.year,
        }),
        PermissiveType::Chunks(_) => None,
    });
    let dois = entry
        .doi()
        .ok()
        .into_iter()
        .map(|value| normalize_doi(&value))
        .collect();
    let arxiv = entry
        .eprint()
        .ok()
        .into_iter()
        .map(|value| normalize_arxiv(&value))
        .collect();
    Ok(Reference {
        title,
        authors,
        collaborations: chunks(entry, "collaboration").into_iter().collect(),
        year,
        identifiers: Identifiers {
            dois,
            arxiv,
            providers: BTreeMap::new(),
        },
    })
}

#[derive(Debug)]
struct RawEntry {
    key: String,
    entry_range: Range<usize>,
    key_range: Range<usize>,
    /// Fields in source order.
    fields: Vec<RawField>,
}

#[derive(Debug)]
struct RawField {
    name: String,
    /// From the start of the field's name to just past its value with trailing
    /// whitespace removed.
    span: Range<usize>,
}

impl RawEntry {
    fn field_names(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|field| field.name.as_str())
    }

    fn declares(&self, name: &str) -> bool {
        self.field_names()
            .any(|field| field.eq_ignore_ascii_case(name))
    }
}

/// Whether a source may carry bytes other than complete entries and whitespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Leniency {
    /// Reject directives, comments, and any other non-entry content. Standalone
    /// entries are rendered from scratch, so anything unmodelled would be lost.
    Strict,
    /// Ignore whatever sits between entries. A caller that only ever splices
    /// entry spans copies those bytes through untouched, so it can afford to
    /// leave them unmodelled.
    IgnoreNonEntryContent,
}

/// Locate and validate complete raw entries without applying BibTeX semantics.
/// This is the sole authority for all raw source and span invariants.
fn scan_raw_entries(source: &str) -> Result<Vec<RawEntry>, Error> {
    scan(source, Leniency::Strict)
}

fn scan(source: &str, leniency: Leniency) -> Result<Vec<RawEntry>, Error> {
    let raw =
        RawBibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let strict = leniency == Leniency::Strict;
    if strict && (!raw.preamble.is_empty() || !raw.abbreviations.is_empty()) {
        return Err(Error::UnsupportedContent(
            "directives are not supported".into(),
        ));
    }
    let mut entries = Vec::with_capacity(raw.entries.len());
    let mut keys = std::collections::BTreeSet::new();
    let mut cursor = 0;
    for item in raw.entries {
        let start = item.span.start;
        let end = item
            .span
            .end
            .checked_add(1)
            .filter(|end| start <= *end && *end <= source.len())
            .ok_or_else(|| Error::InvalidBibtex("entry has an invalid source range".into()))?;
        if strict
            && source
                .get(cursor..start)
                .is_none_or(|gap| !gap.trim().is_empty())
        {
            return Err(Error::UnsupportedContent(
                "only complete BibTeX entries and whitespace are allowed".into(),
            ));
        }
        if source.as_bytes().get(end - 1) != Some(&b'}') {
            return Err(Error::InvalidBibtex("entry has no closing brace".into()));
        }
        source
            .get(start..end)
            .ok_or_else(|| Error::InvalidBibtex("entry is not on UTF-8 boundaries".into()))?;
        let key_range = item.v.key.span.clone();
        let raw_key = source.get(key_range.clone()).ok_or_else(|| {
            Error::InvalidBibtex("citation key has an invalid source range".into())
        })?;
        let key = item.v.key.v.to_owned();
        if raw_key != key {
            return Err(Error::InvalidBibtex(
                "citation key source range does not match parsed key".into(),
            ));
        }
        validate_key(&key)?;
        if !keys.insert(key.clone()) {
            return Err(Error::KeyConflict(key));
        }
        let mut fields = Vec::with_capacity(item.v.fields.len());
        for pair in &item.v.fields {
            let field_start = pair.key.span.start;
            // `abbr_field` eats trailing whitespace before returning, so the
            // value span runs past the field itself.
            let head = source.get(..pair.value.span.end).ok_or_else(|| {
                Error::InvalidBibtex("field value has an invalid source range".into())
            })?;
            let field_end = head.trim_end().len();
            if !(start..end).contains(&field_start) || !(field_start..end).contains(&field_end) {
                return Err(Error::InvalidBibtex(
                    "entry field has an invalid source range".into(),
                ));
            }
            fields.push(RawField {
                name: pair.key.v.to_owned(),
                span: field_start..field_end,
            });
        }
        entries.push(RawEntry {
            key,
            entry_range: start..end,
            key_range,
            fields,
        });
        cursor = end;
    }
    if strict
        && source
            .get(cursor..)
            .is_none_or(|gap| !gap.trim().is_empty())
    {
        return Err(Error::UnsupportedContent(
            "comments, directives, and non-entry content are not supported".into(),
        ));
    }
    Ok(entries)
}

fn validate_semantic(entry: &BibEntry) -> Result<(), Error> {
    let title = entry
        .title()
        .map(ChunksExt::format_verbatim)
        .map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    if title.trim().is_empty() {
        return Err(Error::InvalidBibtex(format!(
            "entry `{}` has no title",
            entry.key
        )));
    }
    Ok(())
}

fn chunks(entry: &BibEntry, field: &str) -> Option<String> {
    entry
        .get(field)
        .map(ChunksExt::format_verbatim)
        .filter(|value| !value.trim().is_empty())
}

/// Validate a citation key against bibi's safe key character set.
pub fn validate_key(key: &str) -> Result<(), Error> {
    if !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'+' | b'-')
        })
    {
        Ok(())
    } else {
        Err(Error::UnsafeKey(key.to_owned()))
    }
}

/// Change only the citation-key token of one complete raw entry.
pub fn rename_entry(source: &str, new_key: &str) -> Result<String, Error> {
    validate_key(new_key)?;
    let raw = scan_raw_entries(source)?;
    if raw.len() != 1 {
        return Err(Error::InvalidBibtex(
            "expected exactly one entry to rename".into(),
        ));
    }
    let mut renamed = source.to_owned();
    renamed.replace_range(raw[0].key_range.clone(), new_key);
    BibtexString::new(renamed.clone())?;
    Ok(renamed)
}

/// Add one field to a complete raw entry without changing its other bytes.
///
/// The field is inserted immediately after the entry's last field, separated by
/// a comma and indented like that field; an entry written on one line stays on
/// one line. An entry that already defines `name` is returned unchanged, so an
/// authored value is never replaced and repeated calls are idempotent. Field
/// names are compared case-insensitively, as BibTeX does. If the last field
/// carries a trailing inline comment, that comment ends up trailing the
/// inserted field instead.
///
/// The value is written verbatim inside braces, so it must be non-empty and
/// must not contain `{`, `}`, `\`, `%`, or a control character.
pub fn insert_field(source: &str, name: &str, value: &str) -> Result<String, Error> {
    validate_field_name(name)?;
    validate_field_value(value)?;
    let raw = scan_raw_entries(source)?;
    let [entry] = raw.as_slice() else {
        return Err(Error::InvalidBibtex(
            "expected exactly one entry to extend".into(),
        ));
    };
    if entry.declares(name) {
        return Ok(source.to_owned());
    }
    // The splice point is the end of the last field's value, before any trailing
    // whitespace or inline comment, so the existing comma placement is exact and
    // the new field can never land inside a `%` comment's line scope. A trailing
    // comment therefore ends up documenting the inserted field, as the test shows.
    let last = entry.fields.last();
    let (insert, separator) = match last {
        Some(field) => (field.span.end, ","),
        None => (entry.entry_range.end - 1, ""),
    };
    let indent =
        last.and_then(|field| line_indent(&source[entry.entry_range.start..field.span.start]));
    let mut extended = String::with_capacity(source.len() + name.len() + value.len() + 8);
    extended.push_str(&source[..insert]);
    extended.push_str(separator);
    if let Some(indent) = indent {
        extended.push('\n');
        extended.push_str(indent);
    }
    extended.push_str(name);
    extended.push_str(" = {");
    extended.push_str(value);
    extended.push('}');
    extended.push_str(&source[insert..]);
    BibtexString::new(extended.clone())?;
    Ok(extended)
}

/// One complete entry located within a multi-entry source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntrySpan {
    /// The entry's citation key.
    pub key: String,
    /// Byte range of the complete `@type{...}` entry.
    pub span: Range<usize>,
    /// Field names in source order.
    pub field_names: Vec<String>,
}

impl EntrySpan {
    /// Whether the entry declares `name`, compared case-insensitively as BibTeX does.
    pub fn declares(&self, name: &str) -> bool {
        self.field_names
            .iter()
            .any(|field| field.eq_ignore_ascii_case(name))
    }
}

/// Locate every complete entry in a source, ignoring the bytes between them.
///
/// Unlike [`parse`], comments and directives are tolerated: a caller that only
/// ever replaces whole entry spans copies everything else through untouched, so
/// unmodelled bytes survive a rewrite rather than being lost by it. Citation
/// keys are still validated and duplicates still rejected, because those are
/// properties of the entries themselves.
pub fn scan_entries(source: &str) -> Result<Vec<EntrySpan>, Error> {
    Ok(scan(source, Leniency::IgnoreNonEntryContent)?
        .into_iter()
        .map(|entry| EntrySpan {
            key: entry.key,
            span: entry.entry_range,
            field_names: entry.fields.into_iter().map(|field| field.name).collect(),
        })
        .collect())
}

/// Read one field's value from a complete raw entry.
///
/// The name is matched case-insensitively and the value is returned with its
/// delimiters removed, so `x-bibi-inspire-id = {1229104}` yields `1229104`.
/// A field the entry does not declare, or one whose value is blank, is `None`.
pub fn field(source: &str, name: &str) -> Result<Option<String>, Error> {
    let raw = scan_raw_entries(source)?;
    let [entry] = raw.as_slice() else {
        return Err(Error::InvalidBibtex(
            "expected exactly one entry to read".into(),
        ));
    };
    if !entry.declares(name) {
        return Ok(None);
    }
    let semantic =
        Bibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let parsed = semantic
        .get(&entry.key)
        .ok_or_else(|| Error::InvalidBibtex(format!("could not parse entry `{}`", entry.key)))?;
    // `biblatex` lowercases field keys while parsing, so the semantic lookup
    // has to be lowercased even though the raw name matched case-insensitively.
    Ok(chunks(parsed, &name.to_ascii_lowercase()))
}

/// Remove one field from a complete raw entry without changing its other bytes.
///
/// An entry that does not declare `name` is returned unchanged, so repeated
/// calls are idempotent. The removal takes the field's separating comma with
/// it: a field with a predecessor is cut from the end of that predecessor's
/// value, and a leading field is cut through to the start of its successor, so
/// the surviving fields keep exact comma placement either way.
pub fn remove_field(source: &str, name: &str) -> Result<String, Error> {
    remove_fields(source, |field| field.eq_ignore_ascii_case(name))
}

/// Remove every field whose name begins with `prefix` from each entry.
///
/// The prefix is matched case-insensitively. This is the exact inverse of
/// [`insert_field`] over a whole bibliography: after inserting prefixed fields
/// with `insert_field`, stripping that prefix recovers the prior bytes.
pub fn strip_fields_with_prefix(source: &str, prefix: &str) -> Result<String, Error> {
    let prefix = prefix.to_ascii_lowercase();
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for entry in scan(source, Leniency::IgnoreNonEntryContent)? {
        let has_prefixed = entry
            .field_names()
            .any(|field| field.to_ascii_lowercase().starts_with(&prefix));
        if !has_prefixed {
            continue;
        }
        let text = &source[entry.entry_range.clone()];
        let stripped = remove_fields(text, |field| {
            field.to_ascii_lowercase().starts_with(&prefix)
        })?;
        output.push_str(&source[cursor..entry.entry_range.start]);
        output.push_str(&stripped);
        cursor = entry.entry_range.end;
    }
    if cursor == 0 {
        return Ok(source.to_owned());
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

/// Cut every field matching `discard` out of one complete raw entry.
///
/// Cuts run back to front so each span stays valid while earlier ones are still
/// pending, and the result is revalidated as a standalone entry so a cut that
/// produced something unparseable is reported rather than written.
fn remove_fields(source: &str, discard: impl Fn(&str) -> bool) -> Result<String, Error> {
    let raw = scan_raw_entries(source)?;
    let [entry] = raw.as_slice() else {
        return Err(Error::InvalidBibtex(
            "expected exactly one entry to trim".into(),
        ));
    };
    let doomed = entry
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| discard(&field.name))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if doomed.is_empty() {
        return Ok(source.to_owned());
    }
    let mut trimmed = source.to_owned();
    for index in doomed.into_iter().rev() {
        let field = &entry.fields[index];
        // Take the comma that joins this field to its neighbour: the one before
        // it when there is a predecessor, otherwise the one after it.
        let cut = match (index.checked_sub(1), entry.fields.get(index + 1)) {
            (Some(previous), _) => entry.fields[previous].span.end..field.span.end,
            (None, Some(next)) => field.span.start..next.span.start,
            (None, None) => field.span.start..field.span.end,
        };
        trimmed.replace_range(cut, "");
    }
    BibtexString::new(trimmed.clone())?;
    Ok(trimmed)
}

/// Leading whitespace of the final line of `head`, or `None` when `head` is one line.
fn line_indent(head: &str) -> Option<&str> {
    let line = head.rsplit_once('\n')?.1;
    Some(&line[..line.len() - line.trim_start().len()])
}

fn validate_field_name(name: &str) -> Result<(), Error> {
    let mut bytes = name.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        || !bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(Error::InvalidBibtex(format!("unsafe field name `{name}`")));
    }
    Ok(())
}

fn validate_field_value(value: &str) -> Result<(), Error> {
    // `%` starts a comment even inside braces once the value reaches LaTeX.
    if value.is_empty()
        || value
            .chars()
            .any(|c| matches!(c, '{' | '}' | '\\' | '%') || c.is_control())
    {
        return Err(Error::InvalidBibtex(format!(
            "unsafe field value `{value}`"
        )));
    }
    Ok(())
}

fn format_person(person: &biblatex::Person) -> String {
    let mut parts = [&person.given_name, &person.prefix, &person.name]
        .into_iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if !person.suffix.is_empty() {
        parts.push(person.suffix.clone());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_entry_retains_bytes_and_projects() {
        let raw = "@article{A,\n title = {A {NASA} result},\n author = {Doe, Jane},\n doi = {10.1/ABC}\n}";
        let parsed = parse(raw).unwrap();
        assert_eq!(parsed["A"].bibtex, raw);
        let projected = parsed["A"].project().unwrap();
        assert_eq!(projected.title, "A NASA result");
        assert_eq!(projected.identifiers.dois, ["10.1/abc"]);
    }

    #[test]
    fn rejects_non_standalone_content() {
        assert!(parse("% comment\n@article{A,title={A}}").is_err());
        assert!(parse("@string{x={x}}").is_err());
        assert!(parse("@preamble{\"x\"}").is_err());
        assert!(parse("@article{A,title={A}} trailing").is_err());
        assert!(parse("@article{A,title={A}}\n% outside").is_err());
    }

    #[test]
    fn raw_scanner_rejects_bad_boundaries_keys_and_duplicates() {
        assert!(parse("@misc{A,title={A}").is_err());
        assert!(parse("@misc{bad key,title={A}}").is_err());
        assert!(matches!(
            parse("@misc{A,title={One}}\n\n@misc{A,title={Two}}"),
            Err(Error::KeyConflict(key)) if key == "A"
        ));
    }

    #[test]
    fn raw_scanner_preserves_each_exact_entry_block() {
        let first = "@misc{Z,\n title = {Zed}\n}";
        let second = "@article{A,title={Alpha}}";
        let source = format!(" \n{first}\n\t\n{second}\n ");
        let parsed = parse(&source).unwrap();
        assert_eq!(parsed["Z"].bibtex, first);
        assert_eq!(parsed["A"].bibtex, second);
    }

    #[test]
    fn renaming_changes_only_key_token() {
        let raw = "@misc{Old,title={Old}}";
        assert_eq!(rename_entry(raw, "New").unwrap(), "@misc{New,title={Old}}");
        assert!(rename_entry("% outside\n@misc{Old,title={Old}}", "New").is_err());
    }

    #[test]
    fn inserting_a_field_follows_the_last_fields_layout() {
        assert_eq!(
            insert_field("@misc{A,\n  title = {T},\n  eprint = {1}\n}", "url", "u").unwrap(),
            "@misc{A,\n  title = {T},\n  eprint = {1},\n  url = {u}\n}"
        );
        // An existing trailing comma terminates the inserted field instead of
        // being duplicated, because the splice point sits before it.
        assert_eq!(
            insert_field("@misc{A,\n  title = {T},\n  eprint = {1},\n}", "url", "u").unwrap(),
            "@misc{A,\n  title = {T},\n  eprint = {1},\n  url = {u},\n}"
        );
        // Indentation comes from the last field's name, not its value, so a
        // wrapped value does not drag the new field out of alignment.
        assert_eq!(
            insert_field("@misc{A,\n  title = {Long\n    wrapped}\n}", "url", "u").unwrap(),
            "@misc{A,\n  title = {Long\n    wrapped},\n  url = {u}\n}"
        );
    }

    #[test]
    fn inserting_a_field_keeps_a_single_line_entry_on_one_line() {
        assert_eq!(
            insert_field("@misc{A,title={A}}", "url", "u").unwrap(),
            "@misc{A,title={A},url = {u}}"
        );
    }

    #[test]
    fn inserting_a_field_preserves_an_existing_value_case_insensitively() {
        let raw = "@misc{A,title={T},URL={keep}}";
        assert_eq!(insert_field(raw, "url", "derived").unwrap(), raw);
        // Insertion is a fixed point, so repeated exports stay byte-stable.
        let once = insert_field("@misc{A,title={T}}", "url", "u").unwrap();
        assert_eq!(insert_field(&once, "url", "u").unwrap(), once);
    }

    #[test]
    fn inserting_a_field_survives_a_trailing_inline_comment() {
        // `biblatex` accepts a comment between the last value and the closing
        // brace, so the field cannot simply be appended before that brace.
        let raw = "@misc{A,\n  title = {T},\n  year = {2025}  % note\n}";
        let extended = insert_field(raw, "url", "u").unwrap();
        assert_eq!(
            extended,
            "@misc{A,\n  title = {T},\n  year = {2025},\n  url = {u}  % note\n}"
        );
        assert!(parse(&extended).is_ok(), "{extended}");
    }

    #[test]
    fn field_insertion_rejects_unsafe_names_values_and_multiple_entries() {
        let raw = "@misc{A,title={T}}";
        assert!(insert_field(raw, "1bad", "u").is_err());
        assert!(insert_field(raw, "url field", "u").is_err());
        assert!(insert_field(raw, "url", "a{b}").is_err());
        assert!(insert_field(raw, "url", "a%b").is_err());
        assert!(insert_field(raw, "url", "a\\b").is_err());
        assert!(insert_field(raw, "url", "").is_err());
        assert!(insert_field(raw, "url", "a\nb").is_err());
        assert!(insert_field("@misc{A,title={T}}\n\n@misc{B,title={T}}", "url", "u").is_err());
        assert!(insert_field("% outside\n@misc{A,title={T}}", "url", "u").is_err());
    }

    /// The lenient scanner exists so a user-owned file survives a rewrite; the
    /// strict path still refuses everything it refused before.
    #[test]
    fn scanning_tolerates_the_non_entry_bytes_that_parsing_rejects() {
        let source =
            "% a note\n@string{j = {Journal}}\n\n@misc{A,\n  title = {T},\n}\n\n% trailing\n";
        let spans = scan_entries(source).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].key, "A");
        assert_eq!(
            &source[spans[0].span.clone()],
            "@misc{A,\n  title = {T},\n}"
        );
        assert_eq!(spans[0].field_names, ["title"]);

        assert!(parse(source).is_err());
        assert!(scan_raw_entries(source).is_err());
    }

    #[test]
    fn scanning_still_rejects_broken_entries_and_duplicate_keys() {
        assert!(scan_entries("@misc{A,title={T}").is_err());
        assert!(scan_entries("@misc{A,title={T}}\n@misc{A,title={U}}").is_err());
        assert!(scan_entries("@misc{bad key,title={T}}").is_err());
    }

    #[test]
    fn field_reads_values_case_insensitively() {
        let raw = "@misc{A,\n  title = {T},\n  x-bibi-inspire-id = {1229104},\n}";
        assert_eq!(
            field(raw, "x-bibi-inspire-id").unwrap().as_deref(),
            Some("1229104")
        );
        assert_eq!(
            field(raw, "X-BIBI-INSPIRE-ID").unwrap().as_deref(),
            Some("1229104")
        );
        assert_eq!(field(raw, "x-bibi-doi").unwrap(), None);
        assert!(field("@misc{A,title={T}}\n\n@misc{B,title={T}}", "title").is_err());
    }

    /// Removal has to answer for the comma whichever neighbour owns it.
    #[test]
    fn removal_takes_the_separating_comma_from_any_position() {
        let leading = "@misc{A,\n  x = {1},\n  title = {T},\n}";
        assert_eq!(
            remove_field(leading, "x").unwrap(),
            "@misc{A,\n  title = {T},\n}"
        );

        let middle = "@misc{A,\n  title = {T},\n  x = {1},\n  year = {2020},\n}";
        assert_eq!(
            remove_field(middle, "x").unwrap(),
            "@misc{A,\n  title = {T},\n  year = {2020},\n}"
        );

        let trailing = "@misc{A,\n  title = {T},\n  x = {1},\n}";
        assert_eq!(
            remove_field(trailing, "x").unwrap(),
            "@misc{A,\n  title = {T},\n}"
        );

        let no_trailing_comma = "@misc{A,\n  title = {T},\n  x = {1}\n}";
        assert_eq!(
            remove_field(no_trailing_comma, "x").unwrap(),
            "@misc{A,\n  title = {T}\n}"
        );
    }

    #[test]
    fn removing_an_absent_field_is_a_no_op_and_removal_is_idempotent() {
        let raw = "@misc{A,\n  title = {T},\n  x = {1},\n}";
        assert_eq!(remove_field(raw, "x-bibi-doi").unwrap(), raw);
        let once = remove_field(raw, "x").unwrap();
        assert_eq!(remove_field(&once, "x").unwrap(), once);
    }

    /// A cut that would leave an entry unparseable is reported, not written.
    #[test]
    fn removal_refuses_to_produce_an_invalid_entry() {
        assert!(remove_field("@misc{A,\n  title = {T},\n}", "title").is_err());
    }

    /// The governing property: stripping undoes insertion exactly, so a file
    /// this crate annotated returns to its original bytes.
    #[test]
    fn stripping_inverts_insertion_byte_for_byte() {
        for original in [
            "@misc{A,\n  title = {T},\n}",
            "@misc{A, title = {T}}",
            "@misc{A,\n  title = {T},\n  year = {2020}\n}",
        ] {
            let mut annotated = insert_field(original, "x-bibi-arxiv", "1207.7214").unwrap();
            annotated = insert_field(&annotated, "x-bibi-doi", "10.1/x").unwrap();
            assert_ne!(annotated, original);
            assert_eq!(
                strip_fields_with_prefix(&annotated, "x-bibi-").unwrap(),
                original
            );
        }
    }

    #[test]
    fn stripping_spans_a_whole_file_and_preserves_everything_else() {
        let source = "% keep me\n@misc{A,\n  title = {T},\n  x-bibi-doi = {10.1/x},\n}\n\n@misc{B,\n  title = {U},\n}\n";
        assert_eq!(
            strip_fields_with_prefix(source, "X-BIBI-").unwrap(),
            "% keep me\n@misc{A,\n  title = {T},\n}\n\n@misc{B,\n  title = {U},\n}\n"
        );
        // Nothing to strip leaves the source untouched, including its comments.
        assert_eq!(strip_fields_with_prefix(source, "z-").unwrap(), source);
    }
}
