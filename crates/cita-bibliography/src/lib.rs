//! Canonical, INSPIRE-managed BibTeX storage.

use biblatex::{
    Bibliography as ParsedBibliography, ChunksExt, DateValue, PermissiveType, RawBibliography,
};
use cita_core::{Locator, normalize_arxiv, normalize_doi};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    key: String,
    raw: String,
    title: String,
    authors: Vec<String>,
    year: Option<i32>,
    dois: Vec<String>,
    eprints: Vec<String>,
}

impl Entry {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn raw(&self) -> &str {
        &self.raw
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn authors(&self) -> &[String] {
        &self.authors
    }
    pub fn year(&self) -> Option<i32> {
        self.year
    }
    pub fn dois(&self) -> &[String] {
        &self.dois
    }
    pub fn eprints(&self) -> &[String] {
        &self.eprints
    }
    pub fn first_arxiv(&self) -> Option<&str> {
        self.eprints.first().map(String::as_str)
    }
}

#[derive(Debug)]
pub struct Bibliography {
    path: PathBuf,
    source: String,
    entries: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddOutcome {
    Added(String),
    Existing(String),
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("bibliography already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("could not read bibliography {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid bibliography {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("invalid BibTeX: {0}")]
    InvalidBibtex(String),
    #[error("unsupported bibliography content: {0}")]
    UnsupportedContent(String),
    #[error("unsafe INSPIRE texkey `{0}`; allowed characters are A-Z, a-z, 0-9, ., _, :, +, and -")]
    UnsafeKey(String),
    #[error("citation key conflict: `{0}` is already present")]
    KeyConflict(String),
    #[error("identifier conflict: {0}")]
    IdentifierConflict(String),
    #[error("paper `{0}` was not found")]
    PaperNotFound(String),
    #[error("could not write bibliography {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Bibliography {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            return Err(Error::AlreadyExists(path));
        }
        let bibliography = Self {
            path,
            source: String::new(),
            entries: BTreeMap::new(),
        };
        bibliography.save_entries(&bibliography.entries)?;
        Ok(bibliography)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let source = fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let entries = parse(&source).map_err(|error| Error::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        Ok(Self {
            path,
            source,
            entries,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }

    pub fn find(&self, selector: &str) -> Option<&Entry> {
        if let Some(entry) = self.entries.get(selector) {
            return Some(entry);
        }
        let locator = selector.parse::<Locator>().ok()?;
        self.entries.values().find(|entry| match &locator {
            Locator::Arxiv(id) => entry
                .eprints
                .iter()
                .any(|value| normalize_arxiv(value) == normalize_arxiv(id)),
            Locator::Doi(doi) => entry
                .dois
                .iter()
                .any(|value| normalize_doi(value) == normalize_doi(doi)),
            Locator::Inspire(_) => false,
        })
    }

    pub fn entry(&self, selector: &str) -> Result<&Entry, Error> {
        self.find(selector)
            .ok_or_else(|| Error::PaperNotFound(selector.to_owned()))
    }

    /// Validate all returned bodies, then add them as one atomic mutation.
    pub fn add_batch(&mut self, bodies: &[String]) -> Result<Vec<AddOutcome>, Error> {
        let mut candidate = self.entries.clone();
        let mut outcomes = Vec::new();
        for body in bodies {
            let parsed = parse(body)?;
            if parsed.len() != 1 {
                return Err(Error::InvalidBibtex(format!(
                    "an INSPIRE lookup returned {} entries instead of one",
                    parsed.len()
                )));
            }
            let (key, entry) = parsed.into_iter().next().expect("length checked");
            if candidate.contains_key(&key) {
                outcomes.push(AddOutcome::Existing(key));
            } else {
                candidate.insert(key.clone(), entry);
                outcomes.push(AddOutcome::Added(key));
            }
        }
        validate_identities(&candidate)?;
        if candidate != self.entries {
            self.save_entries(&candidate)?;
            self.source = render(&candidate);
            self.entries = candidate;
        }
        Ok(outcomes)
    }

    pub fn remove_batch(&mut self, selectors: &[String]) -> Result<Vec<Entry>, Error> {
        let keys = selectors
            .iter()
            .map(|selector| {
                self.find(selector)
                    .map(|entry| entry.key.clone())
                    .ok_or_else(|| Error::PaperNotFound(selector.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut candidate = self.entries.clone();
        let mut removed = Vec::new();
        for key in keys {
            if let Some(entry) = candidate.remove(&key)
                && !removed.iter().any(|old: &Entry| old.key == entry.key)
            {
                removed.push(entry);
            }
        }
        self.save_entries(&candidate)?;
        self.source = render(&candidate);
        self.entries = candidate;
        Ok(removed)
    }

    /// Replace the complete bibliography. Returns whether canonical bytes changed.
    pub fn replace_all(&mut self, bodies: &[String]) -> Result<bool, Error> {
        let source = bodies.join("\n\n");
        let candidate = parse(&source)?;
        let canonical = render(&candidate);
        let changed = canonical != self.source;
        if changed {
            self.save_entries(&candidate)?;
            self.source = canonical;
            self.entries = candidate;
        }
        Ok(changed)
    }

    pub fn canonical(&self) -> String {
        render(&self.entries)
    }

    fn save_entries(&self, entries: &BTreeMap<String, Entry>) -> Result<(), Error> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::Write {
            path: self.path.clone(),
            source,
        })?;
        temporary
            .write_all(render(entries).as_bytes())
            .map_err(|source| Error::Write {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| Error::Write {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .persist(&self.path)
            .map_err(|error| Error::Write {
                path: self.path.clone(),
                source: error.error,
            })?;
        Ok(())
    }
}

pub fn parse(source: &str) -> Result<BTreeMap<String, Entry>, Error> {
    let raw =
        RawBibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    if !raw.preamble.is_empty() || !raw.abbreviations.is_empty() {
        return Err(Error::UnsupportedContent(
            "@preamble and @string directives are not supported".into(),
        ));
    }
    let semantic = ParsedBibliography::parse(source)
        .map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    let mut entries = BTreeMap::new();
    let mut cursor = 0;
    for raw_entry in &raw.entries {
        if !source[cursor..raw_entry.span.start].trim().is_empty() {
            return Err(Error::UnsupportedContent(
                "only complete BibTeX entries and whitespace are allowed".into(),
            ));
        }
        let end = raw_entry
            .span
            .end
            .checked_add(1)
            .filter(|end| *end <= source.len() && source.as_bytes()[*end - 1] == b'}')
            .ok_or_else(|| Error::InvalidBibtex("entry has no closing brace".into()))?;
        let key = raw_entry.v.key.v.to_owned();
        validate_query_key(&key)?;
        let parsed = semantic
            .get(&key)
            .ok_or_else(|| Error::InvalidBibtex(format!("could not semantically parse `{key}`")))?;
        let title = parsed
            .title()
            .map(ChunksExt::format_verbatim)
            .map_err(|error| Error::InvalidBibtex(error.to_string()))?;
        if title.trim().is_empty() {
            return Err(Error::InvalidBibtex(format!("entry `{key}` has no title")));
        }
        let authors = parsed
            .author()
            .unwrap_or_default()
            .iter()
            .map(format_person)
            .collect();
        let dois = parsed
            .doi()
            .ok()
            .into_iter()
            .map(|value| normalize_doi(&value))
            .collect();
        let eprints = parsed
            .eprint()
            .ok()
            .into_iter()
            .map(|value| normalize_arxiv(&value))
            .collect();
        let entry = Entry {
            key: key.clone(),
            raw: source[raw_entry.span.start..end].to_owned(),
            title,
            authors,
            year: parsed.date().ok().and_then(|date| match date {
                PermissiveType::Typed(date) => Some(match date.value {
                    DateValue::At(value) | DateValue::After(value) => value.year,
                    DateValue::Before(value) => value.year,
                    DateValue::Between(value, _) => value.year,
                }),
                PermissiveType::Chunks(_) => None,
            }),
            dois,
            eprints,
        };
        if entries.insert(key.clone(), entry).is_some() {
            return Err(Error::KeyConflict(key));
        }
        cursor = end;
    }
    if !source[cursor..].trim().is_empty() {
        return Err(Error::UnsupportedContent(
            "comments, directives, and non-entry content are not supported".into(),
        ));
    }
    if semantic.len() != entries.len() {
        return Err(Error::UnsupportedContent(
            "unsupported BibTeX content".into(),
        ));
    }
    validate_identities(&entries)?;
    Ok(entries)
}

pub fn render(entries: &BTreeMap<String, Entry>) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut output = entries
        .values()
        .map(|entry| entry.raw.trim().to_owned())
        .collect::<Vec<_>>()
        .join("\n\n");
    output.push('\n');
    output
}

pub fn validate_query_key(key: &str) -> Result<(), Error> {
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
    validate_query_key(new_key)?;
    let raw =
        RawBibliography::parse(source).map_err(|error| Error::InvalidBibtex(error.to_string()))?;
    if raw.entries.len() != 1 || !raw.preamble.is_empty() || !raw.abbreviations.is_empty() {
        return Err(Error::InvalidBibtex(
            "expected exactly one entry to rename".into(),
        ));
    }
    let span = raw.entries[0].v.key.span.clone();
    let mut renamed = source.to_owned();
    renamed.replace_range(span, new_key);
    parse(&renamed)?;
    Ok(renamed)
}

fn validate_identities(entries: &BTreeMap<String, Entry>) -> Result<(), Error> {
    let mut dois: HashMap<&str, &str> = HashMap::new();
    let mut eprints: HashMap<&str, &str> = HashMap::new();
    for (key, entry) in entries {
        for doi in &entry.dois {
            if let Some(other) = dois.insert(doi, key)
                && other != key
            {
                return Err(Error::IdentifierConflict(format!(
                    "DOI {doi} is shared by `{other}` and `{key}`"
                )));
            }
        }
        for eprint in &entry.eprints {
            if let Some(other) = eprints.insert(eprint, key)
                && other != key
            {
                return Err(Error::IdentifierConflict(format!(
                    "arXiv id {eprint} is shared by `{other}` and `{key}`"
                )));
            }
        }
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

    fn entry(key: &str, title: &str, extra: &str) -> String {
        format!(
            "@article{{{key},\n  title = {{{title}}},\n  author = {{Doe, Jane and Roe, Richard}},\n  year = {{2024}},\n  {extra}\n}}"
        )
    }

    #[test]
    fn preserves_raw_entries_and_sorts_with_canonical_separators() {
        let zed = entry("Zed", "Last", "eprint = {2401.00002},");
        let alpha = entry("Alpha", "First", "eprint = {2401.00001},");
        let parsed = parse(&format!("{zed}\n\n{alpha}\n")).unwrap();
        assert_eq!(parsed["Zed"].raw(), zed);
        assert_eq!(render(&parsed), format!("{alpha}\n\n{zed}\n"));
    }

    #[test]
    fn projects_semantic_fields() {
        let parsed = parse(&entry(
            "A",
            "A {NASA} result",
            "doi = {10.1000/ABC},\n  eprint = {1207.7214v2},",
        ))
        .unwrap();
        let item = &parsed["A"];
        assert_eq!(item.title(), "A NASA result");
        assert_eq!(item.authors()[0], "Jane Doe");
        assert_eq!(item.year(), Some(2024));
        assert_eq!(item.dois(), ["10.1000/abc"]);
        assert_eq!(item.eprints(), ["1207.7214"]);
    }

    #[test]
    fn rejects_unsupported_content_and_identifier_conflicts() {
        assert!(matches!(
            parse("% comment\n@article{A,title={A}}"),
            Err(Error::UnsupportedContent(_))
        ));
        assert!(matches!(
            parse("@string{x={x}}"),
            Err(Error::UnsupportedContent(_))
        ));
        let a = entry("A", "A", "doi={10.1/X},");
        let b = entry("B", "B", "doi={10.1/x},");
        assert!(matches!(
            parse(&format!("{a}\n{b}")),
            Err(Error::IdentifierConflict(_))
        ));
    }

    #[test]
    fn batch_mutations_are_atomic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("references.bib");
        let mut bibliography = Bibliography::create(&path).unwrap();
        bibliography
            .add_batch(&[entry("A", "A", "doi={10.1/a},")])
            .unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert!(
            bibliography
                .add_batch(&[
                    entry("B", "B", "doi={10.1/b},"),
                    entry("C", "C", "doi={10.1/a},"),
                ])
                .is_err()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), before);
    }

    #[test]
    fn renames_only_the_key_token() {
        let original = entry("New", "New in title", "note={New},");
        let renamed = rename_entry(&original, "Old").unwrap();
        assert_eq!(renamed, original.replacen("{New,", "{Old,", 1));
    }
}
