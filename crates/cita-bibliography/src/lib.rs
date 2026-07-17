//! Strict standalone BibTeX parsing and rendering support.

use biblatex::{
    Bibliography, Chunk, ChunksExt, DateValue, Entry as BibEntry, EntryType, PermissiveType,
    RawBibliography,
};
use cita_core::{
    Identifiers, ProjectionError, Publication, Reference, ReferenceSource, normalize_arxiv,
    normalize_doi,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BibtexSnapshot {
    pub bibtex: String,
}

impl BibtexSnapshot {
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
pub enum Error {
    #[error("invalid BibTeX: {0}")]
    InvalidBibtex(String),
    #[error("unsupported bibliography content: {0}")]
    UnsupportedContent(String),
    #[error("unsafe citation key `{0}`; allowed characters are A-Z, a-z, 0-9, ., _, :, +, and -")]
    UnsafeKey(String),
    #[error("citation key conflict: `{0}` appears more than once")]
    KeyConflict(String),
    #[error("could not render BibTeX: {0}")]
    Render(String),
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
    let publication = {
        let journal = chunks(entry, "journal").or_else(|| chunks(entry, "journaltitle"));
        let volume = chunks(entry, "volume");
        let issue = chunks(entry, "number").or_else(|| chunks(entry, "issue"));
        let pages = chunks(entry, "pages");
        (journal.is_some() || volume.is_some() || issue.is_some() || pages.is_some()).then_some(
            Publication {
                journal,
                volume,
                issue,
                pages,
                year,
            },
        )
    };
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
        publication,
        url: chunks(entry, "url"),
        primary_category: chunks(entry, "primaryclass").or_else(|| chunks(entry, "eprintclass")),
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
        entries.push(RawEntry {
            key,
            entry_range: start..end,
            key_range,
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

/// Render a source that has no authoritative BibTeX through `biblatex::Entry`.
pub fn render_reference(key: &str, reference: &Reference) -> Result<String, Error> {
    validate_key(key)?;
    let kind = if reference
        .publication
        .as_ref()
        .and_then(|publication| publication.journal.as_ref())
        .is_some()
    {
        EntryType::Article
    } else {
        EntryType::Misc
    };
    let mut entry = BibEntry::new(key.to_owned(), kind);
    set_chunks(&mut entry, "title", &reference.title);
    if !reference.authors.is_empty() {
        set_chunks(&mut entry, "author", &reference.authors.join(" and "));
    }
    if !reference.collaborations.is_empty() {
        set_chunks(
            &mut entry,
            "collaboration",
            &reference.collaborations.join(" and "),
        );
    }
    let publication_year = reference
        .publication
        .as_ref()
        .and_then(|publication| publication.year)
        .or(reference.year);
    if let Some(year) = publication_year {
        set_chunks(&mut entry, "year", &year.to_string());
    }
    if let Some(publication) = &reference.publication {
        set_optional(&mut entry, "journal", publication.journal.as_deref());
        set_optional(&mut entry, "volume", publication.volume.as_deref());
        set_optional(&mut entry, "number", publication.issue.as_deref());
        set_optional(&mut entry, "pages", publication.pages.as_deref());
    }
    set_optional(
        &mut entry,
        "doi",
        reference.identifiers.dois.first().map(String::as_str),
    );
    if let Some(arxiv) = reference.identifiers.arxiv.first() {
        set_chunks(&mut entry, "eprint", arxiv);
        set_chunks(&mut entry, "archiveprefix", "arXiv");
    }
    set_optional(
        &mut entry,
        "primaryclass",
        reference.primary_category.as_deref(),
    );
    set_optional(&mut entry, "url", reference.url.as_deref());
    entry
        .to_bibtex_string()
        .map_err(|error| Error::Render(error.to_string()))
}

fn set_optional(entry: &mut BibEntry, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        set_chunks(entry, key, value);
    }
}

fn set_chunks(entry: &mut BibEntry, key: &str, value: &str) {
    entry.set(
        key,
        vec![biblatex::Spanned::zero(Chunk::Normal(value.to_owned()))],
    );
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
    fn generic_reference_uses_biblatex_serialization() {
        let reference = Reference {
            title: "Future adapter".into(),
            authors: vec!["Doe, Jane".into()],
            year: Some(2026),
            identifiers: Identifiers {
                arxiv: vec!["2601.00001".into()],
                ..Identifiers::default()
            },
            ..Reference::default()
        };
        let rendered = render_reference("Future:2026", &reference).unwrap();
        assert!(rendered.starts_with("@misc{Future:2026,"), "{rendered}");
        assert!(rendered.contains("title = {Future adapter}"), "{rendered}");
        BibtexSnapshot::new(rendered).unwrap();
    }
}
