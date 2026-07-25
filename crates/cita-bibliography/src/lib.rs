//! Strict standalone BibTeX parsing and rendering support.
#![warn(missing_docs)]

use biblatex::{
    Bibliography, ChunksExt, DateValue, Entry as BibEntry, PermissiveType, RawBibliography,
};
use cita_core::{
    Identifiers, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One validated, standalone BibTeX entry with its source bytes preserved.
pub struct BibtexSnapshot {
    /// The complete raw BibTeX entry.
    pub bibtex: String,
}

impl BibtexSnapshot {
    /// Validate and construct a snapshot containing exactly one entry.
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
            .expect("snapshot validation requires one entry"))
    }
}

impl ReferenceSource for BibtexSnapshot {
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
pub fn parse(source: &str) -> Result<BTreeMap<String, BibtexSnapshot>, Error> {
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
        let snapshot = BibtexSnapshot {
            bibtex: source[raw_entry.entry_range].to_owned(),
        };
        let previous = entries.insert(key, snapshot);
        debug_assert!(previous.is_none(), "scanner rejects duplicate keys");
    }
    if semantic.len() != entries.len() {
        return Err(Error::UnsupportedContent(
            "macros requiring global context are not supported".into(),
        ));
    }
    Ok(entries)
}

/// Project one standalone entry into cita's source-neutral reference fields.
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
    /// Field names in source order, for case-insensitive presence checks.
    field_names: Vec<String>,
    /// From the start of the last field's name to just past its value with
    /// trailing whitespace removed; `None` when the entry declares no fields.
    last_field: Option<Range<usize>>,
}

/// Locate and validate complete raw entries without applying BibTeX semantics.
/// This is the sole authority for all raw source and span invariants.
fn scan_raw_entries(source: &str) -> Result<Vec<RawEntry>, Error> {
    let raw =
        RawBibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    if !raw.preamble.is_empty() || !raw.abbreviations.is_empty() {
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
        if source
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
        let field_names = item
            .v
            .fields
            .iter()
            .map(|pair| pair.key.v.to_owned())
            .collect::<Vec<_>>();
        let last_field = match item.v.fields.last() {
            None => None,
            Some(pair) => {
                let field_start = pair.key.span.start;
                // `abbr_field` eats trailing whitespace before returning, so the
                // value span runs past the field itself.
                let head = source.get(..pair.value.span.end).ok_or_else(|| {
                    Error::InvalidBibtex("field value has an invalid source range".into())
                })?;
                let field_end = head.trim_end().len();
                if !(start..end).contains(&field_start) || !(field_start..end).contains(&field_end)
                {
                    return Err(Error::InvalidBibtex(
                        "entry field has an invalid source range".into(),
                    ));
                }
                Some(field_start..field_end)
            }
        };
        entries.push(RawEntry {
            key,
            entry_range: start..end,
            key_range,
            field_names,
            last_field,
        });
        cursor = end;
    }
    if source
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

/// Validate a citation key against cita's safe key character set.
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
    BibtexSnapshot::new(renamed.clone())?;
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
    if entry
        .field_names
        .iter()
        .any(|field| field.eq_ignore_ascii_case(name))
    {
        return Ok(source.to_owned());
    }
    // The splice point is the end of the last field's value, before any trailing
    // whitespace or inline comment, so the existing comma placement is exact and
    // the new field can never land inside a `%` comment's line scope. A trailing
    // comment therefore ends up documenting the inserted field, as the test shows.
    let (insert, separator) = match &entry.last_field {
        Some(field) => (field.end, ","),
        None => (entry.entry_range.end - 1, ""),
    };
    let indent = entry
        .last_field
        .as_ref()
        .and_then(|field| line_indent(&source[entry.entry_range.start..field.start]));
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
    BibtexSnapshot::new(extended.clone())?;
    Ok(extended)
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
}
