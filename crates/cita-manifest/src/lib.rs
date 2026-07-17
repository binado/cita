//! Schema-2 source snapshot storage and generated-bibliography coordination.

use cita_bibliography::{BibtexSnapshot, parse as parse_bibtex, rename_entry, validate_key};
use cita_core::{Locator, Reference, ReferenceSource, normalize_arxiv, normalize_doi};
use cita_inspire_client::InspireSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

pub const SCHEMA: u32 = 2;
pub const MANIFEST_FILE: &str = "cita.toml";
pub const BIBLIOGRAPHY_FILE: &str = "references.bib";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceSnapshot {
    Inspire(Box<InspireSnapshot>),
    Bibtex(BibtexSnapshot),
}

impl SourceSnapshot {
    pub fn raw_bibtex(&self) -> &str {
        match self {
            Self::Inspire(snapshot) => &snapshot.bibtex,
            Self::Bibtex(snapshot) => &snapshot.bibtex,
        }
    }

    pub fn inspire(&self) -> Option<&InspireSnapshot> {
        match self {
            Self::Inspire(snapshot) => Some(snapshot),
            Self::Bibtex(_) => None,
        }
    }
}

impl ReferenceSource for SourceSnapshot {
    fn project(&self) -> Result<Reference, cita_core::ProjectionError> {
        match self {
            Self::Inspire(snapshot) => snapshot.project(),
            Self::Bibtex(snapshot) => snapshot.project(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredReference {
    pub source: SourceSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    pub key: String,
    pub reference: Reference,
    pub managed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingReference {
    pub key: String,
    pub source: SourceSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddOutcome {
    Added(String),
    Existing(String),
}

#[derive(Debug)]
pub struct Manifest {
    path: PathBuf,
    bibliography_path: PathBuf,
    references: BTreeMap<String, StoredReference>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestData {
    schema: u32,
    #[serde(default)]
    references: BTreeMap<String, StoredReference>,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("project already contains {0}")]
    AlreadyExists(PathBuf),
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid manifest {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error(
        "unsupported cita.toml schema {found}; this version supports schema 2 and provides no legacy migration"
    )]
    UnsupportedSchema { found: i64 },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("generated bibliography drift at {path}; run `cita generate`")]
    BibliographyDrift { path: PathBuf },
    #[error("citation key conflict: `{key}` has different source content")]
    KeyConflict { key: String },
    #[error("identifier conflict: {identity} is shared by `{first}` and `{second}`")]
    IdentityConflict {
        identity: String,
        first: String,
        second: String,
    },
    #[error("paper `{0}` was not found")]
    PaperNotFound(String),
    #[error("invalid source for `{key}`: {message}")]
    InvalidSource { key: String, message: String },
    #[error(transparent)]
    Bibtex(#[from] cita_bibliography::Error),
    #[error("could not serialize manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Manifest {
    pub fn create(directory: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = directory.as_ref();
        let path = directory.join(MANIFEST_FILE);
        let bibliography_path = directory.join(BIBLIOGRAPHY_FILE);
        if path.exists() || bibliography_path.exists() {
            return Err(Error::AlreadyExists(if path.exists() {
                path
            } else {
                bibliography_path
            }));
        }
        let manifest = Self {
            path,
            bibliography_path,
            references: BTreeMap::new(),
        };
        manifest.persist_candidate(&manifest.references)?;
        Ok(manifest)
    }

    /// Create schema 2 from an existing standalone bibliography in one mutation.
    pub fn import_existing(directory: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = directory.as_ref();
        let path = directory.join(MANIFEST_FILE);
        let bibliography_path = directory.join(BIBLIOGRAPHY_FILE);
        if path.exists() {
            return Err(Error::AlreadyExists(path));
        }
        let source = fs::read_to_string(&bibliography_path).map_err(|source| Error::Read {
            path: bibliography_path.clone(),
            source,
        })?;
        let references = parse_bibtex(&source)?
            .into_iter()
            .map(|(key, snapshot)| {
                (
                    key,
                    StoredReference {
                        source: SourceSnapshot::Bibtex(snapshot),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        validate_references(&references)?;
        let manifest = Self {
            path,
            bibliography_path,
            references,
        };
        manifest.persist_candidate(&manifest.references)?;
        Ok(manifest)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let source = fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let value: toml::Value = toml::from_str(&source).map_err(|error| Error::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let schema = value
            .get("schema")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| Error::Invalid {
                path: path.clone(),
                message: "missing integer schema".into(),
            })?;
        if schema != i64::from(SCHEMA) {
            return Err(Error::UnsupportedSchema { found: schema });
        }
        let data: ManifestData = toml::from_str(&source).map_err(|error| Error::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        validate_references(&data.references).map_err(|error| Error::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let bibliography_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(BIBLIOGRAPHY_FILE);
        Ok(Self {
            path,
            bibliography_path,
            references: data.references,
        })
    }

    pub fn load_verified(path: impl AsRef<Path>) -> Result<Self, Error> {
        let manifest = Self::load(path)?;
        manifest.verify_bibliography()?;
        Ok(manifest)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn bibliography_path(&self) -> &Path {
        &self.bibliography_path
    }
    pub fn references(&self) -> &BTreeMap<String, StoredReference> {
        &self.references
    }

    pub fn projected(&self) -> Result<Vec<ProjectedReference>, Error> {
        self.references
            .iter()
            .map(|(key, stored)| {
                Ok(ProjectedReference {
                    key: key.clone(),
                    reference: stored
                        .source
                        .project()
                        .map_err(|error| Error::InvalidSource {
                            key: key.clone(),
                            message: error.to_string(),
                        })?,
                    managed: matches!(stored.source, SourceSnapshot::Inspire(_)),
                })
            })
            .collect()
    }

    pub fn find(&self, selector: &str) -> Result<Option<ProjectedReference>, Error> {
        if let Some(stored) = self.references.get(selector) {
            return Ok(Some(projected(selector, stored)?));
        }
        let locator = match selector.parse::<Locator>() {
            Ok(locator) => locator,
            Err(_) => return Ok(None),
        };
        for (key, stored) in &self.references {
            let item = projected(key, stored)?;
            let matches = match &locator {
                Locator::Inspire(id) => item
                    .reference
                    .identifiers
                    .providers
                    .get("inspire")
                    .is_some_and(|ids| ids.iter().any(|value| value == &id.to_string())),
                Locator::Doi(id) => {
                    let id = normalize_doi(id);
                    item.reference.identifiers.dois.contains(&id)
                }
                Locator::Arxiv(id) => {
                    let id = normalize_arxiv(id);
                    item.reference.identifiers.arxiv.contains(&id)
                }
            };
            if matches {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    pub fn add_batch(&mut self, pending: Vec<PendingReference>) -> Result<Vec<AddOutcome>, Error> {
        let mut candidate = self.references.clone();
        let mut outcomes = Vec::with_capacity(pending.len());
        for item in pending {
            validate_key(&item.key)?;
            match candidate.get(&item.key) {
                Some(existing) if existing.source == item.source => {
                    outcomes.push(AddOutcome::Existing(item.key))
                }
                Some(_) => return Err(Error::KeyConflict { key: item.key }),
                None => {
                    candidate.insert(
                        item.key.clone(),
                        StoredReference {
                            source: item.source,
                        },
                    );
                    outcomes.push(AddOutcome::Added(item.key));
                }
            }
        }
        validate_references(&candidate)?;
        if candidate != self.references {
            self.persist_candidate(&candidate)?;
            self.references = candidate;
        }
        Ok(outcomes)
    }

    pub fn remove_batch(&mut self, selectors: &[String]) -> Result<Vec<ProjectedReference>, Error> {
        let mut keys = Vec::new();
        for selector in selectors {
            let item = self
                .find(selector)?
                .ok_or_else(|| Error::PaperNotFound(selector.clone()))?;
            if !keys.contains(&item.key) {
                keys.push(item.key);
            }
        }
        let mut candidate = self.references.clone();
        let mut removed = Vec::new();
        for key in keys {
            let stored = candidate.remove(&key).expect("selected key exists");
            removed.push(projected(&key, &stored)?);
        }
        self.persist_candidate(&candidate)?;
        self.references = candidate;
        Ok(removed)
    }

    pub fn replace_inspire(
        &mut self,
        refreshed: BTreeMap<String, InspireSnapshot>,
    ) -> Result<bool, Error> {
        let expected = self
            .references
            .iter()
            .filter_map(|(key, stored)| stored.source.inspire().map(|_| key.clone()))
            .collect::<Vec<_>>();
        if refreshed.len() != expected.len()
            || expected.iter().any(|key| !refreshed.contains_key(key))
        {
            return Err(Error::InvalidSource {
                key: "sync".into(),
                message: "refresh did not return every managed local key".into(),
            });
        }
        let mut candidate = self.references.clone();
        for (key, snapshot) in refreshed {
            candidate.insert(
                key,
                StoredReference {
                    source: SourceSnapshot::Inspire(Box::new(snapshot)),
                },
            );
        }
        validate_references(&candidate)?;
        let changed = candidate != self.references;
        if changed {
            self.persist_candidate(&candidate)?;
            self.references = candidate;
        }
        Ok(changed)
    }

    pub fn render_bibliography(&self) -> Result<String, Error> {
        render_bibliography(&self.references)
    }

    pub fn verify_bibliography(&self) -> Result<(), Error> {
        let expected = self.render_bibliography()?;
        let actual = fs::read(&self.bibliography_path).ok();
        if actual.as_deref() != Some(expected.as_bytes()) {
            return Err(Error::BibliographyDrift {
                path: self.bibliography_path.clone(),
            });
        }
        Ok(())
    }

    pub fn generate(&self) -> Result<(), Error> {
        atomic_write(
            &self.bibliography_path,
            self.render_bibliography()?.as_bytes(),
        )
    }

    fn persist_candidate(
        &self,
        candidate: &BTreeMap<String, StoredReference>,
    ) -> Result<(), Error> {
        validate_references(candidate)?;
        let bibliography = render_bibliography(candidate)?;
        let manifest = render_manifest(candidate)?;
        // The bibliography is prepared first. The manifest is the commit point.
        atomic_write(&self.bibliography_path, bibliography.as_bytes())?;
        atomic_write(&self.path, manifest.as_bytes())
    }
}

fn projected(key: &str, stored: &StoredReference) -> Result<ProjectedReference, Error> {
    Ok(ProjectedReference {
        key: key.into(),
        reference: stored
            .source
            .project()
            .map_err(|error| Error::InvalidSource {
                key: key.into(),
                message: error.to_string(),
            })?,
        managed: matches!(stored.source, SourceSnapshot::Inspire(_)),
    })
}

fn validate_references(references: &BTreeMap<String, StoredReference>) -> Result<(), Error> {
    let mut identities: HashMap<String, String> = HashMap::new();
    for (key, stored) in references {
        validate_key(key)?;
        if let SourceSnapshot::Inspire(snapshot) = &stored.source {
            snapshot.validate().map_err(|error| Error::InvalidSource {
                key: key.clone(),
                message: error.to_string(),
            })?;
        }
        let reference = stored
            .source
            .project()
            .map_err(|error| Error::InvalidSource {
                key: key.clone(),
                message: error.to_string(),
            })?;
        if reference.title.trim().is_empty() {
            return Err(Error::InvalidSource {
                key: key.clone(),
                message: "missing title".into(),
            });
        }
        for doi in &reference.identifiers.dois {
            record_identity(&mut identities, format!("doi:{}", normalize_doi(doi)), key)?;
        }
        for arxiv in &reference.identifiers.arxiv {
            record_identity(
                &mut identities,
                format!("arxiv:{}", normalize_arxiv(arxiv)),
                key,
            )?;
        }
        for (provider, values) in &reference.identifiers.providers {
            for value in values {
                record_identity(&mut identities, format!("{provider}:{value}"), key)?;
            }
        }
    }
    Ok(())
}

fn record_identity(
    index: &mut HashMap<String, String>,
    identity: String,
    key: &str,
) -> Result<(), Error> {
    if let Some(first) = index.insert(identity.clone(), key.into())
        && first != key
    {
        return Err(Error::IdentityConflict {
            identity,
            first,
            second: key.into(),
        });
    }
    Ok(())
}

fn render_bibliography(references: &BTreeMap<String, StoredReference>) -> Result<String, Error> {
    if references.is_empty() {
        return Ok(String::new());
    }
    let mut entries = Vec::with_capacity(references.len());
    for (key, stored) in references {
        entries.push(
            rename_entry(stored.source.raw_bibtex(), key)?
                .trim()
                .to_owned(),
        );
    }
    Ok(format!("{}\n", entries.join("\n\n")))
}

fn render_manifest(references: &BTreeMap<String, StoredReference>) -> Result<String, Error> {
    let mut output = toml::to_string_pretty(&ManifestData {
        schema: SCHEMA,
        references: references.clone(),
    })?;
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::Write {
        path: path.into(),
        source,
    })?;
    temporary.write_all(bytes).map_err(|source| Error::Write {
        path: path.into(),
        source,
    })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| Error::Write {
            path: path.into(),
            source,
        })?;
    temporary.persist(path).map_err(|error| Error::Write {
        path: path.into(),
        source: error.error,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imported(key: &str, title: &str, extra: &str) -> PendingReference {
        PendingReference {
            key: key.into(),
            source: SourceSnapshot::Bibtex(
                BibtexSnapshot::new(format!("@misc{{{key},title={{{title}}},{extra}}}")).unwrap(),
            ),
        }
    }

    #[test]
    fn schema_round_trips_and_generated_output_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![
                imported("Zed", "Last", ""),
                imported("Alpha", "First", ""),
            ])
            .unwrap();
        let text = fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(text.contains("schema = 2"));
        let loaded = Manifest::load_verified(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(
            loaded
                .render_bibliography()
                .unwrap()
                .starts_with("@misc{Alpha,")
        );
        fs::write(dir.path().join(BIBLIOGRAPHY_FILE), "edited").unwrap();
        assert!(matches!(
            Manifest::load_verified(dir.path().join(MANIFEST_FILE)),
            Err(Error::BibliographyDrift { .. })
        ));
        loaded.generate().unwrap();
        Manifest::load_verified(dir.path().join(MANIFEST_FILE)).unwrap();
    }

    #[test]
    fn cross_source_identity_conflicts_are_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![imported("A", "A", "doi={10.1/X}")])
            .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bib = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();
        assert!(
            manifest
                .add_batch(vec![imported("B", "B", "doi={10.1/x}")])
                .is_err()
        );
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bib
        );
    }

    #[test]
    fn legacy_schema_is_explicitly_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(MANIFEST_FILE), "schema = 1\n").unwrap();
        assert!(matches!(
            Manifest::load(dir.path().join(MANIFEST_FILE)),
            Err(Error::UnsupportedSchema { found: 1 })
        ));
    }
}
