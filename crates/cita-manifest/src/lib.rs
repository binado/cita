//! Schema-3 source snapshot storage and generated-bibliography coordination.

use cita_bibliography::{
    BibtexSnapshot, parse as parse_bibtex, project_bibtex, rename_entry, validate_key,
};
use cita_core::{
    Locator, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use cita_inspire_client::InspireRecord;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

pub const SCHEMA: u32 = 3;
pub const MANIFEST_FILE: &str = "cita.toml";
pub const BIBLIOGRAPHY_FILE: &str = "references.bib";

/// A stored reference tagged by the source that owns its refresh lifecycle.
/// Bibliographic content is always projected from the authoritative BibTeX;
/// INSPIRE records additionally carry the canonical identity BibTeX cannot hold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceSnapshot {
    Inspire(InspireEntry),
    Import(BibtexSnapshot),
}

/// An INSPIRE-managed reference: authoritative BibTeX plus refresh bookkeeping
/// and the curated HEP identifiers that BibTeX rendering cannot express.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireEntry {
    pub record_id: u64,
    pub updated: String,
    pub bibtex: String,
    #[serde(default, skip_serializing_if = "HepIdentifiers::is_empty")]
    pub identifiers: HepIdentifiers,
}

/// Canonical, normalized identifiers stored alongside INSPIRE BibTeX.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HepIdentifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arxiv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
}

impl HepIdentifiers {
    pub fn is_empty(&self) -> bool {
        self.arxiv.is_none() && self.doi.is_none()
    }
}

impl InspireEntry {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let mut reference = project_bibtex(&self.bibtex)
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        if let Some(arxiv) = &self.identifiers.arxiv {
            reference.identifiers.arxiv = vec![normalize_arxiv(arxiv)];
        }
        if let Some(doi) = &self.identifiers.doi {
            reference.identifiers.dois = vec![normalize_doi(doi)];
        }
        reference
            .identifiers
            .providers
            .insert("inspire".to_owned(), vec![self.record_id.to_string()]);
        Ok(reference)
    }
}

impl SourceSnapshot {
    pub fn inspire(record: InspireRecord) -> Self {
        Self::Inspire(InspireEntry {
            record_id: record.record_id,
            updated: record.updated,
            bibtex: record.bibtex,
            identifiers: HepIdentifiers {
                arxiv: record.arxiv,
                doi: record.doi,
            },
        })
    }

    pub fn raw_bibtex(&self) -> &str {
        match self {
            Self::Inspire(entry) => &entry.bibtex,
            Self::Import(snapshot) => &snapshot.bibtex,
        }
    }

    pub fn inspire_entry(&self) -> Option<&InspireEntry> {
        match self {
            Self::Inspire(entry) => Some(entry),
            Self::Import(_) => None,
        }
    }

    fn inspire_record_id(&self) -> Option<u64> {
        self.inspire_entry().map(|entry| entry.record_id)
    }
}

impl ReferenceSource for SourceSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        match self {
            Self::Inspire(entry) => entry.project(),
            Self::Import(snapshot) => snapshot.project(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    pub key: String,
    pub reference: Reference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyRequest {
    Exact(String),
    Suggested(String),
}

impl KeyRequest {
    fn as_str(&self) -> &str {
        match self {
            Self::Exact(key) | Self::Suggested(key) => key,
        }
    }

    fn into_string(self) -> String {
        match self {
            Self::Exact(key) | Self::Suggested(key) => key,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingReference {
    pub key: KeyRequest,
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
    references: BTreeMap<String, SourceSnapshot>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestData {
    schema: u32,
    #[serde(default)]
    references: BTreeMap<String, SourceSnapshot>,
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
        "unsupported cita.toml schema {found}; this version supports schema 3 and provides no legacy migration"
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
    #[error("cannot rename existing reference `{existing}` to `{requested}` during add")]
    CannotRename { existing: String, requested: String },
    #[error("identifier conflict: {identity} is shared by `{first}` and `{second}`")]
    IdentityConflict {
        identity: String,
        first: String,
        second: String,
    },
    #[error("reference `{0}` was not found")]
    ReferenceNotFound(String),
    #[error("invalid source for `{key}`: {message}")]
    InvalidSource { key: String, message: String },
    #[error("invalid INSPIRE refresh record set: {message}")]
    RefreshRecordSet { message: String },
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
            .map(|(key, snapshot)| (key, SourceSnapshot::Import(snapshot)))
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
    pub fn references(&self) -> &BTreeMap<String, SourceSnapshot> {
        &self.references
    }

    pub fn inspire_record_ids(&self) -> Vec<u64> {
        self.references
            .values()
            .filter_map(SourceSnapshot::inspire_record_id)
            .collect()
    }

    pub fn projected(&self) -> Result<Vec<ProjectedReference>, Error> {
        self.references
            .iter()
            .map(|(key, source)| {
                Ok(ProjectedReference {
                    key: key.clone(),
                    reference: source.project().map_err(|error| Error::InvalidSource {
                        key: key.clone(),
                        message: error.to_string(),
                    })?,
                })
            })
            .collect()
    }

    pub fn find(&self, selector: &str) -> Result<Option<ProjectedReference>, Error> {
        if let Some(source) = self.references.get(selector) {
            return Ok(Some(projected(selector, source)?));
        }
        let locator = match selector.parse::<Locator>() {
            Ok(locator) => locator,
            Err(_) => return Ok(None),
        };
        for (key, source) in &self.references {
            let item = projected(key, source)?;
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
            validate_key(item.key.as_str())?;
            if let Some(record_id) = item.source.inspire_record_id()
                && let Some(existing) = candidate.iter().find_map(|(key, source)| {
                    (source.inspire_record_id() == Some(record_id)).then(|| key.clone())
                })
            {
                match item.key {
                    KeyRequest::Suggested(_) => outcomes.push(AddOutcome::Existing(existing)),
                    KeyRequest::Exact(requested) if requested == existing => {
                        outcomes.push(AddOutcome::Existing(existing));
                    }
                    KeyRequest::Exact(requested) => {
                        return Err(Error::CannotRename {
                            existing,
                            requested,
                        });
                    }
                }
                continue;
            }

            let key = item.key.into_string();
            match candidate.get(&key) {
                Some(existing) if existing == &item.source => {
                    outcomes.push(AddOutcome::Existing(key))
                }
                Some(_) => return Err(Error::KeyConflict { key }),
                None => {
                    candidate.insert(key.clone(), item.source);
                    outcomes.push(AddOutcome::Added(key));
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
                .ok_or_else(|| Error::ReferenceNotFound(selector.clone()))?;
            if !keys.contains(&item.key) {
                keys.push(item.key);
            }
        }
        let mut candidate = self.references.clone();
        let mut removed = Vec::new();
        for key in keys {
            let source = candidate.remove(&key).expect("selected key exists");
            removed.push(projected(&key, &source)?);
        }
        self.persist_candidate(&candidate)?;
        self.references = candidate;
        Ok(removed)
    }

    pub fn replace_inspire(&mut self, refreshed: Vec<InspireRecord>) -> Result<bool, Error> {
        let expected = self
            .references
            .iter()
            .filter_map(|(key, source)| {
                source
                    .inspire_entry()
                    .map(|entry| (entry.record_id, key.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut returned = BTreeMap::new();
        for record in refreshed {
            let record_id = record.record_id;
            if !expected.contains_key(&record_id) {
                return Err(Error::RefreshRecordSet {
                    message: format!("returned unexpected record {record_id}"),
                });
            }
            if returned.insert(record_id, record).is_some() {
                return Err(Error::RefreshRecordSet {
                    message: format!("returned duplicate record {record_id}"),
                });
            }
        }
        let missing = expected
            .keys()
            .filter(|record_id| !returned.contains_key(record_id))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(Error::RefreshRecordSet {
                message: format!("did not return records {missing:?}"),
            });
        }
        let mut candidate = self.references.clone();
        for (record_id, key) in expected {
            let record = returned
                .remove(&record_id)
                .expect("record-set reconciliation checked completeness");
            candidate.insert(key, SourceSnapshot::inspire(record));
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
        // A missing file is drift (repairable by `cita generate`); any other
        // read failure is an environment problem and must not masquerade as it.
        let actual = match fs::read(&self.bibliography_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Read {
                    path: self.bibliography_path.clone(),
                    source,
                });
            }
        };
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

    fn persist_candidate(&self, candidate: &BTreeMap<String, SourceSnapshot>) -> Result<(), Error> {
        validate_references(candidate)?;
        let bibliography = render_bibliography(candidate)?;
        let manifest = render_manifest(candidate)?;
        // The bibliography is prepared first. The manifest is the commit point.
        atomic_write(&self.bibliography_path, bibliography.as_bytes())?;
        atomic_write(&self.path, manifest.as_bytes())
    }
}

fn projected(key: &str, source: &SourceSnapshot) -> Result<ProjectedReference, Error> {
    Ok(ProjectedReference {
        key: key.into(),
        reference: source.project().map_err(|error| Error::InvalidSource {
            key: key.into(),
            message: error.to_string(),
        })?,
    })
}

fn validate_references(references: &BTreeMap<String, SourceSnapshot>) -> Result<(), Error> {
    let mut identities: HashMap<String, String> = HashMap::new();
    for (key, source) in references {
        validate_key(key)?;
        if let Some(entry) = source.inspire_entry()
            && entry.record_id == 0
        {
            return Err(Error::InvalidSource {
                key: key.clone(),
                message: "INSPIRE record id is zero".into(),
            });
        }
        let reference = source.project().map_err(|error| Error::InvalidSource {
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

fn render_bibliography(references: &BTreeMap<String, SourceSnapshot>) -> Result<String, Error> {
    if references.is_empty() {
        return Ok(String::new());
    }
    let mut entries = Vec::with_capacity(references.len());
    for (key, source) in references {
        entries.push(rename_entry(source.raw_bibtex(), key)?.trim().to_owned());
    }
    Ok(format!("{}\n", entries.join("\n\n")))
}

fn render_manifest(references: &BTreeMap<String, SourceSnapshot>) -> Result<String, Error> {
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
            key: KeyRequest::Exact(key.into()),
            source: SourceSnapshot::Import(
                BibtexSnapshot::new(format!("@misc{{{key},title={{{title}}},{extra}}}")).unwrap(),
            ),
        }
    }

    fn record(provider_key: &str, record_id: u64) -> InspireRecord {
        InspireRecord {
            record_id,
            updated: "2026-01-01".into(),
            texkey: provider_key.into(),
            bibtex: format!("@misc{{{provider_key},title={{Record {record_id}}}}}"),
            arxiv: None,
            doi: None,
        }
    }

    fn inspire(local_key: &str, provider_key: &str, record_id: u64) -> PendingReference {
        PendingReference {
            key: KeyRequest::Suggested(local_key.into()),
            source: SourceSnapshot::inspire(record(provider_key, record_id)),
        }
    }

    fn exact(mut pending: PendingReference) -> PendingReference {
        pending.key = KeyRequest::Exact(pending.key.into_string());
        pending
    }

    fn updated(provider_key: &str, record_id: u64, timestamp: &str) -> InspireRecord {
        InspireRecord {
            updated: timestamp.into(),
            ..record(provider_key, record_id)
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
        assert!(text.contains("schema = 3"));
        assert!(text.contains("[references.Alpha]"), "{text}");
        assert!(text.contains("source = \"import\""), "{text}");
        assert!(!text.contains("[references.Alpha.source]"), "{text}");
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
    fn suggested_inspire_additions_are_idempotent_by_record_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![inspire("Local", "Provider:Old", 42)])
            .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        let mut changed = inspire("Provider:New", "Provider:New", 42);
        let SourceSnapshot::Inspire(entry) = &mut changed.source else {
            unreachable!()
        };
        entry.updated = "2026-07-17".into();
        entry.bibtex = "@misc{Provider:New,title={Changed provider title}}".into();

        assert_eq!(
            manifest.add_batch(vec![changed]).unwrap(),
            [AddOutcome::Existing("Local".into())]
        );
        let stored = manifest.references()["Local"].inspire_entry().unwrap();
        assert_eq!(stored.bibtex, "@misc{Provider:Old,title={Record 42}}");
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bibliography
        );
    }

    #[test]
    fn exact_inspire_additions_cannot_rename_an_existing_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![exact(inspire("Local", "Provider:Old", 42))])
            .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        assert_eq!(
            manifest
                .add_batch(vec![exact(inspire("Local", "Provider:New", 42))])
                .unwrap(),
            [AddOutcome::Existing("Local".into())]
        );
        assert!(matches!(
            manifest.add_batch(vec![exact(inspire("Renamed", "Provider:New", 42))]),
            Err(Error::CannotRename { existing, requested })
                if existing == "Local" && requested == "Renamed"
        ));
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bibliography
        );
    }

    #[test]
    fn occupied_keys_from_different_records_remain_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![inspire("Local", "Provider:One", 1)])
            .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        assert!(matches!(
            manifest.add_batch(vec![inspire("Local", "Provider:Two", 2)]),
            Err(Error::KeyConflict { key }) if key == "Local"
        ));
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bibliography
        );
    }

    #[test]
    fn mixed_existing_and_new_batches_preserve_order_and_are_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![inspire("Local", "Provider:Old", 1)])
            .unwrap();

        assert_eq!(
            manifest
                .add_batch(vec![
                    inspire("Provider:New", "Provider:New", 1),
                    inspire("Second", "Provider:Two", 2),
                ])
                .unwrap(),
            [
                AddOutcome::Existing("Local".into()),
                AddOutcome::Added("Second".into())
            ]
        );

        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();
        assert!(
            manifest
                .add_batch(vec![
                    inspire("Third", "Provider:Three", 3),
                    inspire("Second", "Provider:Four", 4),
                ])
                .is_err()
        );
        assert!(!manifest.references().contains_key("Third"));
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bibliography
        );
    }

    #[test]
    fn read_failures_are_not_reported_as_drift() {
        let dir = tempfile::tempdir().unwrap();
        Manifest::create(dir.path()).unwrap();
        let bibliography = dir.path().join(BIBLIOGRAPHY_FILE);
        fs::remove_file(&bibliography).unwrap();
        assert!(matches!(
            Manifest::load_verified(dir.path().join(MANIFEST_FILE)),
            Err(Error::BibliographyDrift { .. })
        ));
        // A directory at the bibliography path makes fs::read fail with an
        // error other than NotFound, which must surface as a read failure.
        fs::create_dir(&bibliography).unwrap();
        assert!(matches!(
            Manifest::load_verified(dir.path().join(MANIFEST_FILE)),
            Err(Error::Read { .. })
        ));
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

    #[test]
    fn nested_schema_two_shape_and_unknown_fields_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![imported("Alpha", "First", "")])
            .unwrap();
        let path = dir.path().join(MANIFEST_FILE);
        let flat = fs::read_to_string(&path).unwrap();

        let nested = flat.replace("[references.Alpha]", "[references.Alpha.source]");
        fs::write(&path, &nested).unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
        assert_eq!(fs::read_to_string(&path).unwrap(), nested);

        fs::write(&path, format!("{flat}unknown = true\n")).unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
    }

    #[test]
    fn refresh_reconciles_out_of_order_ids_without_changing_local_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![
                inspire("LocalA", "ProviderA", 1),
                inspire("LocalB", "ProviderB", 2),
                imported("Imported", "Imported", ""),
            ])
            .unwrap();
        assert_eq!(manifest.inspire_record_ids(), [1, 2]);

        let refreshed = vec![
            updated("ProviderB", 2, "2026-02-02"),
            updated("ProviderA", 1, "2026-02-01"),
        ];
        assert!(manifest.replace_inspire(refreshed).unwrap());
        assert_eq!(
            manifest.references()["LocalA"]
                .inspire_entry()
                .unwrap()
                .record_id,
            1
        );
        assert_eq!(
            manifest.references()["LocalB"]
                .inspire_entry()
                .unwrap()
                .record_id,
            2
        );
        assert!(manifest.references().contains_key("Imported"));
        let bibliography = fs::read_to_string(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();
        assert!(bibliography.contains("@misc{LocalA,"), "{bibliography}");
        assert!(bibliography.contains("@misc{LocalB,"), "{bibliography}");
    }

    #[test]
    fn invalid_refresh_record_sets_leave_both_files_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![
                inspire("LocalA", "ProviderA", 1),
                inspire("LocalB", "ProviderB", 2),
            ])
            .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();
        let one = updated("ProviderA", 1, "new");
        let two = updated("ProviderB", 2, "new");
        let unexpected = updated("ProviderC", 3, "new");
        for returned in [
            vec![one.clone()],
            vec![one.clone(), one.clone(), two.clone()],
            vec![one.clone(), two.clone(), unexpected.clone()],
        ] {
            assert!(matches!(
                manifest.replace_inspire(returned),
                Err(Error::RefreshRecordSet { .. })
            ));
            assert_eq!(
                fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
                before_manifest
            );
            assert_eq!(
                fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
                before_bibliography
            );
        }
    }

    #[test]
    fn missing_refresh_record_ids_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .add_batch(vec![
                inspire("Ten", "Provider:Ten", 10),
                inspire("Two", "Provider:Two", 2),
                inspire("Seven", "Provider:Seven", 7),
            ])
            .unwrap();

        assert!(matches!(
            manifest.replace_inspire(Vec::new()),
            Err(Error::RefreshRecordSet { message }) if message == "did not return records [2, 7, 10]"
        ));
    }
}
