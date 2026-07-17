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
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    pub key: String,
    pub reference: Reference,
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
            .map(|(key, snapshot)| (key, SourceSnapshot::Bibtex(snapshot)))
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
            .filter_map(|source| source.inspire().map(|snapshot| snapshot.record_id))
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
            validate_key(&item.key)?;
            match candidate.get(&item.key) {
                Some(existing) if existing == &item.source => {
                    outcomes.push(AddOutcome::Existing(item.key))
                }
                Some(_) => return Err(Error::KeyConflict { key: item.key }),
                None => {
                    candidate.insert(item.key.clone(), item.source);
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

    pub fn replace_inspire(&mut self, refreshed: Vec<InspireSnapshot>) -> Result<bool, Error> {
        let expected = self
            .references
            .iter()
            .filter_map(|(key, source)| {
                source
                    .inspire()
                    .map(|snapshot| (snapshot.record_id, key.clone()))
            })
            .collect::<HashMap<_, _>>();
        let mut returned = HashMap::with_capacity(refreshed.len());
        for snapshot in refreshed {
            let record_id = snapshot.record_id;
            if !expected.contains_key(&record_id) {
                return Err(Error::RefreshRecordSet {
                    message: format!("returned unexpected record {record_id}"),
                });
            }
            if returned.insert(record_id, snapshot).is_some() {
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
            let snapshot = returned
                .remove(&record_id)
                .expect("record-set reconciliation checked completeness");
            candidate.insert(key, SourceSnapshot::Inspire(Box::new(snapshot)));
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
        if let SourceSnapshot::Inspire(snapshot) = source {
            snapshot.validate().map_err(|error| Error::InvalidSource {
                key: key.clone(),
                message: error.to_string(),
            })?;
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
    use cita_inspire_client::Title;

    fn imported(key: &str, title: &str, extra: &str) -> PendingReference {
        PendingReference {
            key: key.into(),
            source: SourceSnapshot::Bibtex(
                BibtexSnapshot::new(format!("@misc{{{key},title={{{title}}},{extra}}}")).unwrap(),
            ),
        }
    }

    fn inspire(local_key: &str, provider_key: &str, record_id: u64) -> PendingReference {
        PendingReference {
            key: local_key.into(),
            source: SourceSnapshot::Inspire(Box::new(InspireSnapshot {
                record_id,
                updated: "2026-01-01".into(),
                texkeys: vec![provider_key.into()],
                bibtex: format!("@misc{{{provider_key},title={{Record {record_id}}}}}"),
                titles: vec![Title {
                    title: format!("Record {record_id}"),
                }],
                authors: Vec::new(),
                collaborations: Vec::new(),
                publication_info: Vec::new(),
                arxiv_eprints: Vec::new(),
                dois: Vec::new(),
                urls: Vec::new(),
                document_types: Vec::new(),
                preprint_date: None,
                earliest_date: None,
            })),
        }
    }

    fn updated(pending: PendingReference, timestamp: &str) -> InspireSnapshot {
        let SourceSnapshot::Inspire(snapshot) = pending.source else {
            unreachable!()
        };
        InspireSnapshot {
            updated: timestamp.into(),
            ..*snapshot
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
        assert!(text.contains("[references.Alpha]"), "{text}");
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
            updated(inspire("ignored", "ProviderB", 2), "2026-02-02"),
            updated(inspire("ignored", "ProviderA", 1), "2026-02-01"),
        ];
        assert!(manifest.replace_inspire(refreshed).unwrap());
        assert_eq!(
            manifest.references()["LocalA"].inspire().unwrap().record_id,
            1
        );
        assert_eq!(
            manifest.references()["LocalB"].inspire().unwrap().record_id,
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
        let one = updated(inspire("ignored", "ProviderA", 1), "new");
        let two = updated(inspire("ignored", "ProviderB", 2), "new");
        let unexpected = updated(inspire("ignored", "ProviderC", 3), "new");
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
}
