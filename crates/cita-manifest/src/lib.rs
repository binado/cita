//! Schema-1 source snapshot storage and generated-bibliography coordination.
#![warn(missing_docs)]

mod library;

pub use library::{LIBRARY_FILE, Library, LibraryError, Shelf};

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

/// Manifest schema version supported by this crate.
pub const SCHEMA: u32 = 1;
/// Name of the authoritative project manifest.
pub const MANIFEST_FILE: &str = "cita.toml";
/// Name of the deterministic generated BibTeX artifact.
pub const BIBLIOGRAPHY_FILE: &str = "references.bib";

/// A stored reference tagged by the source that owns its refresh lifecycle.
/// Bibliographic content is always projected from the authoritative BibTeX;
/// INSPIRE records additionally carry canonical values selected from and
/// cross-checked against the authoritative BibTeX.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceSnapshot {
    /// A refreshable INSPIRE-managed snapshot.
    Inspire(InspireEntry),
    /// A source-preserving generic BibTeX import.
    Import(BibtexSnapshot),
}

/// An INSPIRE-managed reference: authoritative BibTeX plus refresh bookkeeping
/// and curated canonical HEP identifiers selected from and cross-checked
/// against that BibTeX.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireEntry {
    /// Stable INSPIRE record identifier used for refresh.
    pub record_id: u64,
    /// Provider update timestamp.
    pub updated: String,
    /// Complete authoritative standalone BibTeX entry.
    pub bibtex: String,
    /// Canonical identifiers selected from and cross-checked against the BibTeX.
    #[serde(default, skip_serializing_if = "HepIdentifiers::is_empty")]
    pub identifiers: HepIdentifiers,
}

/// Curated canonical, normalized identifiers cross-checked against INSPIRE BibTeX.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HepIdentifiers {
    /// Canonical normalized, versionless arXiv identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arxiv: Option<String>,
    /// Canonical normalized DOI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
}

impl HepIdentifiers {
    /// Normalize and construct a curated identifier set.
    pub fn new(arxiv: Option<String>, doi: Option<String>) -> Self {
        Self {
            arxiv: arxiv.as_deref().map(normalize_arxiv),
            doi: doi.as_deref().map(normalize_doi),
        }
    }

    /// Return whether neither curated identifier is present.
    pub fn is_empty(&self) -> bool {
        self.arxiv.is_none() && self.doi.is_none()
    }
}

impl InspireEntry {
    fn project(&self) -> Result<Reference, ProjectionError> {
        cita_inspire_client::project_inspire(
            &self.bibtex,
            self.identifiers.arxiv.as_deref(),
            self.identifiers.doi.as_deref(),
            self.record_id,
        )
    }
}

impl SourceSnapshot {
    /// Convert a durable provider record into a manifest snapshot.
    pub fn inspire(record: InspireRecord) -> Self {
        Self::Inspire(InspireEntry {
            record_id: record.record_id,
            updated: record.updated,
            bibtex: record.bibtex,
            identifiers: HepIdentifiers::new(record.arxiv, record.doi),
        })
    }

    /// Return the authoritative raw BibTeX entry.
    pub fn raw_bibtex(&self) -> &str {
        match self {
            Self::Inspire(entry) => &entry.bibtex,
            Self::Import(snapshot) => &snapshot.bibtex,
        }
    }

    /// Return INSPIRE bookkeeping for a managed snapshot.
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
/// A local citation key paired with its projected semantic reference.
pub struct ProjectedReference {
    /// Local citation key.
    pub key: String,
    /// Source-neutral projected reference.
    pub reference: Reference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Controls whether an add must use a key or may resolve an identity collision.
pub enum KeyRequest {
    /// Require this exact local citation key.
    Exact(String),
    /// Use this key unless its INSPIRE record ID already exists under another key.
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
/// A validated source snapshot waiting to be added under a requested key.
pub struct PendingReference {
    /// Exact or suggested local key request.
    pub key: KeyRequest,
    /// Authoritative source snapshot.
    pub source: SourceSnapshot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Controls how `add_batch` resolves a collision with an existing reference.
pub enum ConflictPolicy {
    /// Leave the existing reference in place and report the incoming one as skipped.
    #[default]
    Skip,
    /// Replace the colliding existing reference(s) with the incoming one.
    Overwrite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Result of adding one pending reference.
pub enum AddOutcome {
    /// A new reference was stored under this actual local key.
    Added(String),
    /// The same identity was already stored under this actual local key.
    Existing(String),
    /// An incoming reference collided with an existing one and was left out.
    Skipped {
        /// Local key the incoming reference requested.
        key: String,
        /// Existing local key it collides with (equal to `key` for a same-key clash).
        conflicting: String,
    },
    /// An incoming reference replaced a colliding existing reference.
    Overwritten {
        /// Local key the incoming reference now occupies.
        key: String,
        /// Every local key removed to make room for the incoming reference.
        replaced: Vec<String>,
    },
}

#[derive(Debug)]
/// Loaded schema-1 manifest and its coordinated bibliography artifact.
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
/// Error produced by manifest loading, validation, mutation, or persistence.
pub enum Error {
    /// Initialization found an existing managed artifact.
    #[error("project already contains {0}")]
    AlreadyExists(PathBuf),
    /// A managed file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A manifest failed syntax or semantic validation.
    #[error("invalid manifest {path}: {message}")]
    Invalid {
        /// Invalid manifest path.
        path: PathBuf,
        /// Validation diagnostic.
        message: String,
    },
    /// The manifest uses a schema this release cannot migrate or read.
    #[error(
        "unsupported cita.toml schema {found}; this version supports schema 1 and provides no legacy migration"
    )]
    UnsupportedSchema {
        /// Schema value found in the file.
        found: i64,
    },
    /// A managed file could not be written atomically.
    #[error("could not write {path}: {source}")]
    Write {
        /// File that could not be written.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Generated BibTeX differs from the manifest-derived bytes.
    #[error("generated bibliography drift at {path}; run `cita generate`")]
    BibliographyDrift {
        /// Drifted bibliography path.
        path: PathBuf,
    },
    /// An exact-key add tried to rename an already stored identity.
    #[error("cannot rename existing reference `{existing}` to `{requested}` during add")]
    CannotRename {
        /// Current local key.
        existing: String,
        /// Requested replacement key.
        requested: String,
    },
    /// Two local keys share a normalized DOI, arXiv, or provider identity.
    #[error("identifier conflict: {identity} is shared by `{first}` and `{second}`")]
    IdentityConflict {
        /// Conflicting normalized identity.
        identity: String,
        /// First local key.
        first: String,
        /// Second local key.
        second: String,
    },
    /// No reference matched a selector.
    #[error("reference `{0}` was not found")]
    ReferenceNotFound(String),
    /// A stored snapshot cannot be projected or violates source invariants.
    #[error("invalid source for `{key}`: {message}")]
    InvalidSource {
        /// Local key of the invalid source.
        key: String,
        /// Validation diagnostic.
        message: String,
    },
    /// Refreshed INSPIRE records do not explain the requested managed set.
    #[error("invalid INSPIRE refresh record set: {message}")]
    RefreshRecordSet {
        /// Record-set validation diagnostic.
        message: String,
    },
    /// BibTeX parsing, projection, or re-keying failed.
    #[error(transparent)]
    Bibtex(#[from] cita_bibliography::Error),
    /// Deterministic TOML serialization failed.
    #[error("could not serialize manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Manifest {
    /// Create an empty schema-1 manifest and bibliography in a directory.
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

    /// Create schema 1 from an existing standalone bibliography in one mutation.
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

    /// Load and semantically validate a manifest without checking generated output.
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

    /// Load a manifest and verify its generated bibliography byte-for-byte.
    pub fn load_verified(path: impl AsRef<Path>) -> Result<Self, Error> {
        let manifest = Self::load(path)?;
        manifest.verify_bibliography()?;
        Ok(manifest)
    }

    /// Return the authoritative manifest path.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Return the coordinated generated bibliography path.
    pub fn bibliography_path(&self) -> &Path {
        &self.bibliography_path
    }
    /// Return snapshots ordered by local citation key.
    pub fn references(&self) -> &BTreeMap<String, SourceSnapshot> {
        &self.references
    }

    /// Return stable IDs for all INSPIRE-managed references in key order.
    pub fn inspire_record_ids(&self) -> Vec<u64> {
        self.references
            .values()
            .filter_map(SourceSnapshot::inspire_record_id)
            .collect()
    }

    /// Project every snapshot into source-neutral reference fields.
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

    /// Find by exact local key, then provider ID, DOI, or arXiv ID.
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

    /// Validate and atomically add a complete batch of references.
    ///
    /// Collisions never abort the batch: under [`ConflictPolicy::Skip`] a
    /// colliding incoming entry is reported as [`AddOutcome::Skipped`] and left
    /// out, while under [`ConflictPolicy::Overwrite`] it replaces the colliding
    /// entry (rekeying when the collision is a different-key duplicate). Only
    /// malformed input still aborts, leaving the manifest untouched.
    pub fn add_batch(
        &mut self,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<Vec<AddOutcome>, Error> {
        let mut candidate = self.references.clone();
        let mut index = identity_index(&candidate)?;
        let mut outcomes = Vec::with_capacity(pending.len());
        for item in pending {
            validate_key(item.key.as_str())?;
            let forced_collision = if let Some(record_id) = item.source.inspire_record_id()
                && let Some(existing) = candidate.iter().find_map(|(key, source)| {
                    (source.inspire_record_id() == Some(record_id)).then(|| key.clone())
                }) {
                match &item.key {
                    KeyRequest::Suggested(_) => {
                        outcomes.push(AddOutcome::Existing(existing));
                        continue;
                    }
                    KeyRequest::Exact(requested) if requested == &existing => {
                        outcomes.push(AddOutcome::Existing(existing));
                        continue;
                    }
                    KeyRequest::Exact(requested) => match policy {
                        ConflictPolicy::Skip => {
                            return Err(Error::CannotRename {
                                existing,
                                requested: requested.clone(),
                            });
                        }
                        ConflictPolicy::Overwrite => Some(existing),
                    },
                }
            } else {
                None
            };

            let key = item.key.into_string();
            let source = item.source;
            if candidate
                .get(&key)
                .is_some_and(|existing| existing == &source)
            {
                outcomes.push(AddOutcome::Existing(key));
                continue;
            }
            let reference = projected(&key, &source)?.reference;
            let identities = reference_identities(&reference);
            let mut colliding = Vec::new();
            if candidate.contains_key(&key) {
                colliding.push(key.clone());
            }
            if let Some(existing) = forced_collision
                && existing != key
                && !colliding.contains(&existing)
            {
                colliding.push(existing);
            }
            for identity in &identities {
                if let Some(other) = index.get(identity)
                    && other != &key
                    && !colliding.iter().any(|existing| existing == other)
                {
                    colliding.push(other.clone());
                }
            }
            match (colliding.is_empty(), policy) {
                (true, _) => {
                    for identity in identities {
                        index.insert(identity, key.clone());
                    }
                    candidate.insert(key.clone(), source);
                    outcomes.push(AddOutcome::Added(key));
                }
                (false, ConflictPolicy::Skip) => {
                    let conflicting = colliding.into_iter().next().expect("collision present");
                    outcomes.push(AddOutcome::Skipped { key, conflicting });
                }
                (false, ConflictPolicy::Overwrite) => {
                    let replaced = colliding.clone();
                    for removed in &colliding {
                        candidate.remove(removed);
                        index.retain(|_, existing| existing != removed);
                    }
                    for identity in identities {
                        index.insert(identity, key.clone());
                    }
                    candidate.insert(key.clone(), source);
                    outcomes.push(AddOutcome::Overwritten { key, replaced });
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

    /// Resolve and atomically remove all supplied selectors.
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

    /// Replace the complete managed INSPIRE set while preserving local keys.
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

    /// Render deterministic BibTeX sorted and re-keyed by local key.
    pub fn render_bibliography(&self) -> Result<String, Error> {
        render_bibliography(&self.references)
    }

    /// Verify that the generated bibliography exactly matches the manifest.
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

    /// Atomically regenerate the bibliography from authoritative snapshots.
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
        if let Some(entry) = source.inspire_entry() {
            if entry.record_id == 0 {
                return Err(Error::InvalidSource {
                    key: key.clone(),
                    message: "INSPIRE record id is zero".into(),
                });
            }
            let bibtex_reference =
                project_bibtex(&entry.bibtex).map_err(|error| Error::InvalidSource {
                    key: key.clone(),
                    message: error.to_string(),
                })?;
            if let Some(arxiv) = &entry.identifiers.arxiv {
                let normalized = normalize_arxiv(arxiv);
                if !bibtex_reference.identifiers.arxiv.contains(&normalized) {
                    return Err(Error::InvalidSource {
                        key: key.clone(),
                        message: format!(
                            "curated arXiv id {normalized} is not present in stored BibTeX"
                        ),
                    });
                }
            }
            if let Some(doi) = &entry.identifiers.doi {
                let normalized = normalize_doi(doi);
                if !bibtex_reference.identifiers.dois.contains(&normalized) {
                    return Err(Error::InvalidSource {
                        key: key.clone(),
                        message: format!(
                            "curated DOI {normalized} is not present in stored BibTeX"
                        ),
                    });
                }
            }
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
        for identity in reference_identities(&reference) {
            record_identity(&mut identities, identity, key)?;
        }
    }
    Ok(())
}

/// Normalized identity strings (`doi:` / `arxiv:` / `<provider>:`) for a reference.
fn reference_identities(reference: &Reference) -> Vec<String> {
    let mut identities = Vec::new();
    for doi in &reference.identifiers.dois {
        identities.push(format!("doi:{}", normalize_doi(doi)));
    }
    for arxiv in &reference.identifiers.arxiv {
        identities.push(format!("arxiv:{}", normalize_arxiv(arxiv)));
    }
    for (provider, values) in &reference.identifiers.providers {
        for value in values {
            identities.push(format!("{provider}:{value}"));
        }
    }
    identities
}

/// Build an `identity -> local key` index over already-valid references.
fn identity_index(
    references: &BTreeMap<String, SourceSnapshot>,
) -> Result<HashMap<String, String>, Error> {
    let mut index = HashMap::new();
    for (key, source) in references {
        let reference = projected(key, source)?.reference;
        for identity in reference_identities(&reference) {
            index.insert(identity, key.clone());
        }
    }
    Ok(index)
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

    /// Add a batch with the default skip-on-collision policy.
    fn add(
        manifest: &mut Manifest,
        pending: Vec<PendingReference>,
    ) -> Result<Vec<AddOutcome>, Error> {
        manifest.add_batch(pending, ConflictPolicy::Skip)
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
        add(
            &mut manifest,
            vec![imported("Zed", "Last", ""), imported("Alpha", "First", "")],
        )
        .unwrap();
        let text = fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(text.contains("schema = 1"));
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
    fn cross_source_identity_conflicts_skip_by_default_and_rekey_on_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("A", "A", "doi={10.1/X}")]).unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bib = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        // A different key sharing the same normalized DOI is skipped, not fatal,
        // and leaves both managed files byte-for-byte unchanged.
        assert_eq!(
            add(&mut manifest, vec![imported("B", "B", "doi={10.1/x}")]).unwrap(),
            [AddOutcome::Skipped {
                key: "B".into(),
                conflicting: "A".into(),
            }]
        );
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bib
        );

        // Overwrite rekeys: the old key is removed and the incoming key stored.
        assert_eq!(
            manifest
                .add_batch(
                    vec![imported("B", "B", "doi={10.1/x}")],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [AddOutcome::Overwritten {
                key: "B".into(),
                replaced: vec!["A".into()],
            }]
        );
        assert!(manifest.references().contains_key("B"));
        assert!(!manifest.references().contains_key("A"));
    }

    #[test]
    fn suggested_inspire_additions_are_idempotent_by_record_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![inspire("Local", "Provider:Old", 42)]).unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        let mut changed = inspire("Provider:New", "Provider:New", 42);
        let SourceSnapshot::Inspire(entry) = &mut changed.source else {
            unreachable!()
        };
        entry.updated = "2026-07-17".into();
        entry.bibtex = "@misc{Provider:New,title={Changed provider title}}".into();

        assert_eq!(
            add(&mut manifest, vec![changed]).unwrap(),
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
        add(
            &mut manifest,
            vec![exact(inspire("Local", "Provider:Old", 42))],
        )
        .unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        assert_eq!(
            add(
                &mut manifest,
                vec![exact(inspire("Local", "Provider:New", 42))]
            )
            .unwrap(),
            [AddOutcome::Existing("Local".into())]
        );
        // The explicit-key rename of an already stored record stays a default error.
        assert!(matches!(
            add(&mut manifest, vec![exact(inspire("Renamed", "Provider:New", 42))]),
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

        // Under overwrite the same explicit-key request rekeys the record.
        assert_eq!(
            manifest
                .add_batch(
                    vec![exact(inspire("Renamed", "Provider:New", 42))],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [AddOutcome::Overwritten {
                key: "Renamed".into(),
                replaced: vec!["Local".into()],
            }]
        );
        assert!(manifest.references().contains_key("Renamed"));
        assert!(!manifest.references().contains_key("Local"));
    }

    #[test]
    fn occupied_keys_from_different_records_skip_then_overwrite_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![inspire("Local", "Provider:One", 1)]).unwrap();
        let before_manifest = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();
        let before_bibliography = fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();

        // Same local key, different record: skipped without touching the files.
        assert_eq!(
            add(&mut manifest, vec![inspire("Local", "Provider:Two", 2)]).unwrap(),
            [AddOutcome::Skipped {
                key: "Local".into(),
                conflicting: "Local".into(),
            }]
        );
        assert_eq!(
            fs::read(dir.path().join(MANIFEST_FILE)).unwrap(),
            before_manifest
        );
        assert_eq!(
            fs::read(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            before_bibliography
        );

        // Overwrite replaces the content in place under the same key.
        assert_eq!(
            manifest
                .add_batch(
                    vec![inspire("Local", "Provider:Two", 2)],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [AddOutcome::Overwritten {
                key: "Local".into(),
                replaced: vec!["Local".into()],
            }]
        );
        assert_eq!(
            manifest.references()["Local"]
                .inspire_entry()
                .unwrap()
                .record_id,
            2
        );
    }

    #[test]
    fn mixed_existing_new_and_skipped_batches_preserve_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![inspire("Local", "Provider:Old", 1)]).unwrap();

        assert_eq!(
            add(
                &mut manifest,
                vec![
                    inspire("Provider:New", "Provider:New", 1),
                    inspire("Second", "Provider:Two", 2),
                ],
            )
            .unwrap(),
            [
                AddOutcome::Existing("Local".into()),
                AddOutcome::Added("Second".into())
            ]
        );

        // A clean addition and a colliding one coexist: the new key is added and
        // the collision is skipped rather than aborting the whole batch.
        assert_eq!(
            add(
                &mut manifest,
                vec![
                    inspire("Third", "Provider:Three", 3),
                    inspire("Second", "Provider:Four", 4),
                ],
            )
            .unwrap(),
            [
                AddOutcome::Added("Third".into()),
                AddOutcome::Skipped {
                    key: "Second".into(),
                    conflicting: "Second".into(),
                },
            ]
        );
        assert!(manifest.references().contains_key("Third"));
        // The skipped colliding entry keeps its original record.
        assert_eq!(
            manifest.references()["Second"]
                .inspire_entry()
                .unwrap()
                .record_id,
            2
        );
    }

    #[test]
    fn intra_batch_duplicates_resolve_first_wins_then_later_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();

        // Two entries in one batch sharing a DOI: the first wins, the later one
        // skips against the running candidate.
        assert_eq!(
            add(
                &mut manifest,
                vec![
                    imported("First", "First", "doi={10.1/DUP}"),
                    imported("Second", "Second", "doi={10.1/dup}"),
                ],
            )
            .unwrap(),
            [
                AddOutcome::Added("First".into()),
                AddOutcome::Skipped {
                    key: "Second".into(),
                    conflicting: "First".into(),
                },
            ]
        );
        assert!(manifest.references().contains_key("First"));
        assert!(!manifest.references().contains_key("Second"));

        // Under overwrite the later entry rekeys the earlier one it collides with.
        let other = tempfile::tempdir().unwrap();
        let mut fresh = Manifest::create(other.path()).unwrap();
        assert_eq!(
            fresh
                .add_batch(
                    vec![
                        imported("First", "First", "doi={10.1/DUP}"),
                        imported("Second", "Second", "doi={10.1/dup}"),
                    ],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [
                AddOutcome::Added("First".into()),
                AddOutcome::Overwritten {
                    key: "Second".into(),
                    replaced: vec!["First".into()],
                },
            ]
        );
        assert!(fresh.references().contains_key("Second"));
        assert!(!fresh.references().contains_key("First"));
    }

    #[test]
    fn overwrite_reports_and_removes_every_identity_collision() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(
            &mut manifest,
            vec![
                imported("DoiOwner", "DOI owner", "doi={10.1/BRIDGE}"),
                imported("ArxivOwner", "arXiv owner", "eprint={2401.00042}"),
            ],
        )
        .unwrap();

        assert_eq!(
            manifest
                .add_batch(
                    vec![imported(
                        "Combined",
                        "Combined",
                        "doi={10.1/bridge},eprint={2401.00042}",
                    )],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [AddOutcome::Overwritten {
                key: "Combined".into(),
                replaced: vec!["DoiOwner".into(), "ArxivOwner".into()],
            }]
        );
        assert_eq!(
            manifest.references().keys().cloned().collect::<Vec<_>>(),
            ["Combined"]
        );
    }

    #[test]
    fn exact_inspire_overwrite_cleans_all_collisions_from_the_running_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(
            &mut manifest,
            vec![
                exact(inspire("Local", "Provider:Old", 42)),
                imported("Renamed", "Requested key occupant", "doi={10.1/FREED}"),
                imported("ArxivOwner", "arXiv owner", "eprint={2401.00042}"),
            ],
        )
        .unwrap();

        let mut refreshed = record("Provider:New", 42);
        refreshed.bibtex = "@misc{Provider:New,title={Refreshed},eprint={2401.00042}}".into();
        refreshed.arxiv = Some("2401.00042".into());
        let pending = PendingReference {
            key: KeyRequest::Exact("Renamed".into()),
            source: SourceSnapshot::inspire(refreshed),
        };

        assert_eq!(
            manifest
                .add_batch(
                    vec![
                        pending,
                        imported("Freed", "Freed identity", "doi={10.1/freed}"),
                    ],
                    ConflictPolicy::Overwrite,
                )
                .unwrap(),
            [
                AddOutcome::Overwritten {
                    key: "Renamed".into(),
                    replaced: vec!["Renamed".into(), "Local".into(), "ArxivOwner".into(),],
                },
                AddOutcome::Added("Freed".into()),
            ]
        );
        assert_eq!(
            manifest.references().keys().cloned().collect::<Vec<_>>(),
            ["Freed", "Renamed"]
        );
        assert_eq!(
            manifest.references()["Renamed"]
                .inspire_entry()
                .unwrap()
                .record_id,
            42
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
    fn non_current_schemas_are_explicitly_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(MANIFEST_FILE), "schema = 0\n").unwrap();
        assert!(matches!(
            Manifest::load(dir.path().join(MANIFEST_FILE)),
            Err(Error::UnsupportedSchema { found: 0 })
        ));
        fs::write(dir.path().join(MANIFEST_FILE), "schema = 2\n").unwrap();
        assert!(matches!(
            Manifest::load(dir.path().join(MANIFEST_FILE)),
            Err(Error::UnsupportedSchema { found: 2 })
        ));
    }

    #[test]
    fn nested_shape_and_unknown_fields_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("Alpha", "First", "")]).unwrap();
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
    fn curated_identifiers_override_bibtex_derived_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        let record = InspireRecord {
            record_id: 99,
            updated: "2026-01-01".into(),
            texkey: "Curated:2026".into(),
            bibtex: "@misc{Curated:2026,title={T},eprint={2401.00001},doi={10.1/BibtexCase}}"
                .into(),
            // Both curated values are un-normalized (version suffix / mixed
            // case) so the assertions only pass if the override branch
            // actually runs `normalize_arxiv`/`normalize_doi`, not merely
            // whether the record happens to have the same content as BibTeX.
            arxiv: Some("2401.00001v9".into()),
            doi: Some("10.1/BIBTEXCASE".into()),
        };
        add(
            &mut manifest,
            vec![PendingReference {
                key: KeyRequest::Suggested("Local".into()),
                source: SourceSnapshot::inspire(record),
            }],
        )
        .unwrap();
        let projected = manifest.projected().unwrap();
        let reference = &projected
            .iter()
            .find(|item| item.key == "Local")
            .unwrap()
            .reference;
        assert_eq!(reference.identifiers.arxiv, ["2401.00001"]);
        assert_eq!(reference.identifiers.dois, ["10.1/bibtexcase"]);
    }

    #[test]
    fn curated_identifier_partial_override_falls_through_for_the_other_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        let record = InspireRecord {
            record_id: 100,
            updated: "2026-01-01".into(),
            texkey: "Partial:2026".into(),
            bibtex: "@misc{Partial:2026,title={T},eprint={2401.00001},doi={10.1/frombib}}".into(),
            arxiv: Some("2401.00001v3".into()),
            doi: None,
        };
        add(
            &mut manifest,
            vec![PendingReference {
                key: KeyRequest::Suggested("Local".into()),
                source: SourceSnapshot::inspire(record),
            }],
        )
        .unwrap();
        let projected = manifest.projected().unwrap();
        let reference = &projected
            .iter()
            .find(|item| item.key == "Local")
            .unwrap()
            .reference;
        assert_eq!(reference.identifiers.arxiv, ["2401.00001"]);
        assert_eq!(reference.identifiers.dois, ["10.1/frombib"]);
    }

    #[test]
    fn zero_record_id_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        assert!(matches!(
            add(&mut manifest, vec![inspire("Local", "Key", 0)]),
            Err(Error::InvalidSource { .. })
        ));
    }

    #[test]
    fn source_snapshot_inspire_normalizes_curated_identifiers() {
        let record = InspireRecord {
            record_id: 1,
            updated: "2026-01-01".into(),
            texkey: "Key:2026".into(),
            bibtex: "@misc{Key:2026,title={T},eprint={2401.00001}}".into(),
            arxiv: Some("2401.00001v2".into()),
            doi: None,
        };
        let SourceSnapshot::Inspire(entry) = SourceSnapshot::inspire(record) else {
            unreachable!()
        };
        assert_eq!(entry.identifiers.arxiv.as_deref(), Some("2401.00001"));
    }

    #[test]
    fn curated_arxiv_id_absent_from_stored_bibtex_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        let mismatched = PendingReference {
            key: KeyRequest::Suggested("Local".into()),
            source: SourceSnapshot::Inspire(InspireEntry {
                record_id: 1,
                updated: "2026-01-01".into(),
                bibtex: "@misc{Key,title={T},eprint={2401.00001}}".into(),
                identifiers: HepIdentifiers {
                    arxiv: Some("2402.00002".into()),
                    doi: None,
                },
            }),
        };
        assert!(matches!(
            add(&mut manifest, vec![mismatched]),
            Err(Error::InvalidSource { .. })
        ));
    }

    #[test]
    fn refresh_reconciles_out_of_order_ids_without_changing_local_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(
            &mut manifest,
            vec![
                inspire("LocalA", "ProviderA", 1),
                inspire("LocalB", "ProviderB", 2),
                imported("Imported", "Imported", ""),
            ],
        )
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
        add(
            &mut manifest,
            vec![
                inspire("LocalA", "ProviderA", 1),
                inspire("LocalB", "ProviderB", 2),
            ],
        )
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
        add(
            &mut manifest,
            vec![
                inspire("Ten", "Provider:Ten", 10),
                inspire("Two", "Provider:Two", 2),
                inspire("Seven", "Provider:Seven", 7),
            ],
        )
        .unwrap();

        assert!(matches!(
            manifest.replace_inspire(Vec::new()),
            Err(Error::RefreshRecordSet { message }) if message == "did not return records [2, 7, 10]"
        ));
    }
}
