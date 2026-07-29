//! Semantic projection of an entry that is itself the original source.
//!
//! This module is the only place `biblatex` is used, and the only place BibTeX
//! is read as metadata rather than as structure. That is permitted precisely
//! where the BibTeX *is* the original — a user-supplied entry for a work no
//! provider holds — and nowhere else: a provider's BibTeX is a lossy rendering
//! of a structured record, and bibi maps the record instead.

use crate::{entry::IdentifierCandidates, error::Error};
use biblatex::{
    Bibliography, ChunksExt, DateValue, Entry as SemanticEntry, PermissiveType, Person,
};

/// Metadata projected from a local entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalMetadata {
    /// The entry's title. Required: a record without one cannot be listed.
    pub title: String,
    /// Authors in source order, formatted `Family, Suffix, Given`.
    pub authors: Vec<String>,
    /// The `collaboration` field, if it carries one.
    pub collaborations: Vec<String>,
    /// The year, when the entry states a parseable date.
    pub year: Option<i32>,
    /// The `doi` field, as written.
    pub doi: Option<String>,
    /// The `eprint` field, as written.
    pub arxiv: Option<String>,
}

/// Project one standalone entry, whose key the scanner has already read.
pub(crate) fn project(
    source: &str,
    key: &str,
    candidates: IdentifierCandidates,
) -> Result<LocalMetadata, Error> {
    let bibliography = Bibliography::parse(source).map_err(|error| Error::SemanticParse {
        message: error.to_string(),
    })?;
    let entry = bibliography.get(key).ok_or_else(|| Error::SemanticParse {
        message: format!("entry `{key}` did not survive semantic parsing"),
    })?;
    // A missing title is its own diagnostic rather than a parse failure: the
    // entry is well-formed, it simply cannot be listed. Reading the raw chunks
    // keeps the two apart, since the typed accessor reports absence as an error.
    let title = entry
        .get("title")
        .map(ChunksExt::format_verbatim)
        .unwrap_or_default();
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::MissingTitle {
            key: key.to_owned(),
        });
    }
    Ok(LocalMetadata {
        title: title.to_owned(),
        authors: entry
            .author()
            .unwrap_or_default()
            .iter()
            .map(format_person)
            .filter(|author| !author.is_empty())
            .collect(),
        collaborations: collaborations(entry),
        year: year(entry),
        doi: candidates.doi,
        arxiv: candidates.arxiv,
    })
}

/// Format one person as `Family, Suffix, Given`.
///
/// Family-name-first matches how structured providers render `full_name`, so a
/// listing mixing local and provider-owned records reads as one column rather
/// than two conventions.
fn format_person(person: &Person) -> String {
    let mut family = person.prefix.trim().to_owned();
    if !family.is_empty() && !person.name.trim().is_empty() {
        family.push(' ');
    }
    family.push_str(person.name.trim());
    let tail = [person.suffix.trim(), person.given_name.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if tail.is_empty() {
        return family;
    }
    if family.is_empty() {
        return tail.join(", ");
    }
    format!("{family}, {}", tail.join(", "))
}

/// Read the `collaboration` field as one value.
///
/// BibTeX has no list syntax that distinguishes "the ATLAS Collaboration" from
/// an author, which is exactly why provider-owned records take collaborations
/// from the structured record instead. A local entry's field is therefore kept
/// whole rather than split on a separator bibi would have to guess.
fn collaborations(entry: &SemanticEntry) -> Vec<String> {
    entry
        .get("collaboration")
        .map(ChunksExt::format_verbatim)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect()
}

fn year(entry: &SemanticEntry) -> Option<i32> {
    match entry.date().ok()? {
        PermissiveType::Typed(date) => Some(match date.value {
            DateValue::At(value) | DateValue::After(value) | DateValue::Before(value) => value.year,
            DateValue::Between(value, _) => value.year,
        }),
        PermissiveType::Chunks(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::BibtexEntry;

    fn metadata(source: &str) -> crate::LocalMetadata {
        BibtexEntry::parse_one(source.to_owned())
            .unwrap()
            .local_metadata()
            .unwrap()
    }

    #[test]
    fn projects_title_authors_year_and_identifiers() {
        let projected = metadata(
            "@article{A,\n  title = {A {NASA} result},\n  author = {Doe, Jane and van der Berg, Piet},\n  year = {2024},\n  doi = {10.1/ABC}\n}",
        );
        assert_eq!(projected.title, "A NASA result");
        assert_eq!(projected.authors, ["Doe, Jane", "van der Berg, Piet"]);
        assert_eq!(projected.year, Some(2024));
        assert_eq!(projected.doi.as_deref(), Some("10.1/ABC"));
        assert_eq!(projected.arxiv, None);
    }

    #[test]
    fn keeps_a_collaboration_field_whole() {
        let projected = metadata(
            "@article{A, title = {T}, author = {Aad, G.}, collaboration = {ATLAS and CMS}}",
        );
        assert_eq!(projected.collaborations, ["ATLAS and CMS"]);
        assert!(metadata("@misc{A, title = {T}}").collaborations.is_empty());
    }

    #[test]
    fn formats_a_suffix_between_family_and_given_names() {
        let projected = metadata("@misc{A, title = {T}, author = {King, Jr., Martin Luther}}");
        assert_eq!(projected.authors, ["King, Jr., Martin Luther"]);
    }

    #[test]
    fn requires_a_non_empty_title() {
        for source in ["@misc{A, author = {Doe, Jane}}", "@misc{A, title = { }}"] {
            let entry = BibtexEntry::parse_one(source.to_owned()).unwrap();
            assert!(
                matches!(
                    entry.local_metadata(),
                    Err(crate::Error::MissingTitle { .. })
                ),
                "{source}"
            );
        }
    }

    #[test]
    fn tolerates_an_unparseable_date() {
        let projected = metadata("@misc{A, title = {T}, year = {in press}}");
        assert_eq!(projected.year, None);
    }
}
