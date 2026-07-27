use crate::{
    DEFAULT_SHELF, Library, LibraryError, ShelfName, SourceSnapshot,
    library::{insert_reference, shelf_id, source_identities},
};
use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const INTERCHANGE_SCHEMA: u32 = 1;

/// Versioned, lossless logical-library interchange document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interchange {
    schema: u32,
    scope: Scope,
    references: Vec<ExportReference>,
    shelves: Vec<ExportShelf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Scope {
    Library,
    Shelf { name: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportReference {
    handle: String,
    source: SourceSnapshot,
    identities: Vec<ExportIdentity>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportIdentity {
    kind: String,
    value: String,
    canonical: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportShelf {
    name: String,
    entries: Vec<ExportMembership>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportMembership {
    key: String,
    reference: String,
}

impl Interchange {
    /// Parse JSON and reject an unsupported or inconsistent document.
    pub fn from_json(source: &str) -> Result<Self, LibraryError> {
        let value: Self = serde_json::from_str(source)
            .map_err(|error| LibraryError::InvalidInterchange(error.to_string()))?;
        value.validate()?;
        Ok(value)
    }

    /// Parse TOML and reject an unsupported or inconsistent document.
    pub fn from_toml(source: &str) -> Result<Self, LibraryError> {
        let value: Self = toml::from_str(source)
            .map_err(|error| LibraryError::InvalidInterchange(error.to_string()))?;
        value.validate()?;
        Ok(value)
    }

    /// Render deterministic pretty JSON with one trailing newline.
    pub fn to_json(&self) -> Result<String, LibraryError> {
        let mut output = serde_json::to_string_pretty(self)
            .map_err(|error| LibraryError::InvalidInterchange(error.to_string()))?;
        output.push('\n');
        Ok(output)
    }

    /// Render deterministic pretty TOML with one trailing newline.
    pub fn to_toml(&self) -> Result<String, LibraryError> {
        let mut output = toml::to_string_pretty(self)
            .map_err(|error| LibraryError::InvalidInterchange(error.to_string()))?;
        if !output.ends_with('\n') {
            output.push('\n');
        }
        Ok(output)
    }

    fn validate(&self) -> Result<(), LibraryError> {
        if self.schema != INTERCHANGE_SCHEMA {
            return Err(LibraryError::InvalidInterchange(format!(
                "unsupported schema {}; expected {INTERCHANGE_SCHEMA}",
                self.schema
            )));
        }
        let mut handles: BTreeMap<&str, &ExportReference> = BTreeMap::new();
        let mut identity_owners = BTreeMap::new();
        let mut provider_owners = BTreeMap::new();
        for reference in &self.references {
            if handles
                .insert(reference.handle.as_str(), reference)
                .is_some()
            {
                return Err(LibraryError::InvalidInterchange(format!(
                    "duplicate reference handle `{}`",
                    reference.handle
                )));
            }
            let expected = export_identities(&reference.source)?;
            if reference.identities != expected {
                return Err(LibraryError::InvalidInterchange(format!(
                    "identities for `{}` do not match its BibTeX",
                    reference.handle
                )));
            }
            for identity in &reference.identities {
                let identity_key = (identity.kind.as_str(), identity.value.as_str());
                if let Some(owner) = identity_owners.insert(identity_key, reference.handle.as_str())
                {
                    return Err(LibraryError::InvalidInterchange(format!(
                        "{} identity `{}` belongs to both `{owner}` and `{}`",
                        identity.kind, identity.value, reference.handle
                    )));
                }
            }
            if let Some(entry) = reference.source.inspire_entry()
                && let Some(owner) =
                    provider_owners.insert(entry.record_id, reference.handle.as_str())
            {
                return Err(LibraryError::InvalidInterchange(format!(
                    "INSPIRE record {} belongs to both `{owner}` and `{}`",
                    entry.record_id, reference.handle
                )));
            }
        }
        let mut names = BTreeSet::new();
        let mut used = BTreeSet::new();
        for shelf in &self.shelves {
            ShelfName::try_from(shelf.name.as_str())?;
            if !names.insert(shelf.name.to_ascii_lowercase()) {
                return Err(LibraryError::InvalidInterchange(format!(
                    "duplicate shelf `{}`",
                    shelf.name
                )));
            }
            let mut keys = BTreeSet::new();
            let mut references = BTreeSet::new();
            for entry in &shelf.entries {
                cita_bibliography::validate_key(&entry.key)?;
                if !keys.insert(&entry.key) {
                    return Err(LibraryError::InvalidInterchange(format!(
                        "duplicate key `{}` in shelf `{}`",
                        entry.key, shelf.name
                    )));
                }
                if !references.insert(&entry.reference) {
                    return Err(LibraryError::InvalidInterchange(format!(
                        "reference `{}` appears twice in shelf `{}`",
                        entry.reference, shelf.name
                    )));
                }
                if !handles.contains_key(entry.reference.as_str()) {
                    return Err(LibraryError::InvalidInterchange(format!(
                        "unknown reference handle `{}`",
                        entry.reference
                    )));
                }
                used.insert(entry.reference.as_str());
            }
        }
        match &self.scope {
            Scope::Library if !self.shelves.iter().any(|shelf| shelf.name == DEFAULT_SHELF) => {
                return Err(LibraryError::InvalidInterchange(
                    "library scope must contain the `main` shelf".into(),
                ));
            }
            Scope::Shelf { name } => {
                ShelfName::try_from(name.as_str())?;
                if name.eq_ignore_ascii_case(DEFAULT_SHELF) && name != DEFAULT_SHELF {
                    return Err(LibraryError::InvalidInterchange(format!(
                        "default shelf must be spelled `{DEFAULT_SHELF}`"
                    )));
                }
                if self.shelves.len() != 1 || self.shelves[0].name != *name {
                    return Err(LibraryError::InvalidInterchange(
                        "shelf scope must contain exactly its named shelf".into(),
                    ));
                }
            }
            Scope::Library => {}
        }
        if used.len() != handles.len() {
            return Err(LibraryError::InvalidInterchange(
                "every exported reference must belong to a shelf".into(),
            ));
        }
        Ok(())
    }
}

impl Library {
    /// Build a deterministic lossless document for one shelf.
    pub fn export_shelf(&self, shelf: &ShelfName) -> Result<Interchange, LibraryError> {
        self.export_interchange(Some(shelf))
    }

    /// Build a deterministic lossless document for the complete library.
    pub fn export_library(&self) -> Result<Interchange, LibraryError> {
        self.export_interchange(None)
    }

    fn export_interchange(
        &self,
        selected: Option<&ShelfName>,
    ) -> Result<Interchange, LibraryError> {
        let shelf_names = match selected {
            Some(name) => {
                self.validate_shelf(name)?;
                vec![name.clone()]
            }
            None => self.shelves()?.into_iter().collect(),
        };
        let mut handles: BTreeMap<i64, String> = BTreeMap::new();
        let mut references = Vec::new();
        let mut shelves = Vec::new();
        for shelf in shelf_names {
            let mut memberships = Vec::new();
            for entry in self.entries(&shelf)? {
                let handle = match handles.get(&entry.id) {
                    Some(handle) => handle.clone(),
                    None => {
                        let handle = format!("r{:06}", references.len() + 1);
                        references.push(ExportReference {
                            handle: handle.clone(),
                            identities: export_identities(&entry.source)?,
                            source: entry.source,
                        });
                        handles.insert(entry.id, handle.clone());
                        handle
                    }
                };
                memberships.push(ExportMembership {
                    key: entry.key,
                    reference: handle,
                });
            }
            shelves.push(ExportShelf {
                name: shelf.to_string(),
                entries: memberships,
            });
        }
        let value = Interchange {
            schema: INTERCHANGE_SCHEMA,
            scope: selected.map_or(Scope::Library, |name| Scope::Shelf {
                name: name.to_string(),
            }),
            references,
            shelves,
        };
        value.validate()?;
        Ok(value)
    }

    /// Replace all logical database content from a validated document.
    pub fn replace_from_interchange(&self, value: &Interchange) -> Result<(), LibraryError> {
        value.validate()?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM shelf_references", [])?;
        transaction.execute("DELETE FROM shelves", [])?;
        transaction.execute("DELETE FROM bibliography_references", [])?;

        let mut shelves = value.shelves.clone();
        if !shelves.iter().any(|shelf| shelf.name == DEFAULT_SHELF) {
            shelves.push(ExportShelf {
                name: DEFAULT_SHELF.into(),
                entries: Vec::new(),
            });
        }
        shelves.sort_by(|left, right| left.name.cmp(&right.name));
        for shelf in &shelves {
            transaction.execute("INSERT INTO shelves(name) VALUES (?1)", [&shelf.name])?;
        }

        let mut ids = BTreeMap::new();
        for reference in &value.references {
            let id = insert_reference(&transaction, &reference.source)?;
            ids.insert(reference.handle.as_str(), id);
        }
        for shelf in &value.shelves {
            let name = ShelfName::try_from(shelf.name.as_str())?;
            let shelf_id = shelf_id(&transaction, &name)?;
            for entry in &shelf.entries {
                transaction.execute(
                    "INSERT INTO shelf_references(shelf_id, reference_id, citation_key)
                     VALUES (?1, ?2, ?3)",
                    params![shelf_id, ids[entry.reference.as_str()], entry.key],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

fn export_identities(source: &SourceSnapshot) -> Result<Vec<ExportIdentity>, LibraryError> {
    let canonical = source.inspire_entry().map(|entry| &entry.identifiers);
    let mut identities = source_identities(source)?
        .into_iter()
        .map(|(kind, value)| ExportIdentity {
            canonical: canonical.is_some_and(|ids| match kind.as_str() {
                "arxiv" => ids.arxiv.as_deref() == Some(value.as_str()),
                "doi" => ids.doi.as_deref() == Some(value.as_str()),
                _ => false,
            }),
            kind,
            value,
        })
        .collect::<Vec<_>>();
    identities.sort();
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConflictPolicy, KeyRequest, PendingReference};
    use cita_bibliography::BibtexSnapshot;

    #[test]
    fn full_export_round_trips_sharing_and_bytes() {
        let first = tempfile::tempdir().unwrap();
        let first = Library::open_or_create(first.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        let other = ShelfName::try_from("paper").unwrap();
        first.create_shelf(&other).unwrap();
        let source = SourceSnapshot::Import(
            BibtexSnapshot::new("@article{Raw,\n title = {Exact},\n doi = {10.1/x}\n}".into())
                .unwrap(),
        );
        for (shelf, key) in [(&main, "MainKey"), (&other, "OtherKey")] {
            first
                .add_batch(
                    shelf,
                    vec![PendingReference {
                        key: KeyRequest::Exact(key.into()),
                        source: source.clone(),
                    }],
                    ConflictPolicy::Skip,
                )
                .unwrap();
        }
        let export = first.export_library().unwrap();
        assert_eq!(
            export,
            Interchange::from_toml(&export.to_toml().unwrap()).unwrap()
        );

        let second = tempfile::tempdir().unwrap();
        let second = Library::open_or_create(second.path().canonicalize().unwrap()).unwrap();
        second.replace_from_interchange(&export).unwrap();
        assert_eq!(
            first.export_library().unwrap().to_json().unwrap(),
            second.export_library().unwrap().to_json().unwrap()
        );
    }

    #[test]
    fn restoring_one_nondefault_shelf_also_creates_main() {
        let first = tempfile::tempdir().unwrap();
        let first = Library::open_or_create(first.path().canonicalize().unwrap()).unwrap();
        let paper = ShelfName::try_from("paper").unwrap();
        first.create_shelf(&paper).unwrap();
        first
            .add_batch(
                &paper,
                vec![PendingReference {
                    key: KeyRequest::Exact("Paper".into()),
                    source: SourceSnapshot::Import(
                        BibtexSnapshot::new("@misc{Raw,\n title={Paper}\n}".into()).unwrap(),
                    ),
                }],
                ConflictPolicy::Skip,
            )
            .unwrap();
        let export = first.export_shelf(&paper).unwrap();

        let second = tempfile::tempdir().unwrap();
        let second = Library::open_or_create(second.path().canonicalize().unwrap()).unwrap();
        second.replace_from_interchange(&export).unwrap();
        assert_eq!(
            second
                .shelves()
                .unwrap()
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
            ["main", "paper"]
        );
    }
}
