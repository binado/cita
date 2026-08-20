//! Schema-2 structured reference storage and generated-bibliography coordination.
#![warn(missing_docs)]

mod library;

pub use library::{LIBRARY_FILE, Library, LibraryError, Shelf};

use cita_bibliography::{
    BibtexSnapshot, parse as parse_bibtex, project_bibtex, rename_entry, validate_key,
};
use cita_core::{
    Locator, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use cita_inspire_client::InspireSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

/// Manifest schema version supported by this crate.
pub const SCHEMA: u32 = 2;
/// Name of the authoritative project manifest.
pub const MANIFEST_FILE: &str = "cita.toml";
/// Name of the deterministic generated BibTeX artifact.
pub const BIBLIOGRAPHY_FILE: &str = "references.bib";

/// A stored reference: authoritative structured fields, user-owned tags and
/// notes, and the opaque BibTeX blob a provider or an import supplied.
///
/// The structured fields are seeded exactly once, by [`Entry::from_inspire`] or
/// [`Entry::from_bibtex`], and are authoritative from then on. Nothing
/// re-derives them from `bibtex`, and nothing rewrites `bibtex` from them, so
/// the two lanes never need reconciling. A reference no provider knows about
/// simply carries no `bibtex` at all.
///
/// Provenance is the presence of a provider sub-table rather than a tag field:
/// an entry carrying [`InspireProvenance`] is refreshed by `cita sync`, and an
/// entry without one is never touched by a provider.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Lowercased BibTeX entry type, e.g. `article` or `book`.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Display title.
    pub title: String,
    /// Individual author names in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Collaboration names in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<String>,
    /// Publication year, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// Canonical DOI, normalized at ingest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    /// Canonical versionless arXiv identifier, normalized at ingest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arxiv: Option<String>,
    /// User-owned tags. A set, so they stay sorted and deduplicated.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    /// User-owned notes in the order they were written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The complete standalone BibTeX entry, when a source supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bibtex: Option<String>,
    /// INSPIRE refresh bookkeeping, present exactly for managed entries.
    ///
    /// TOML requires every value before any sub-table, so this must remain the
    /// last declared field: a field declared after it fails to serialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspire: Option<InspireProvenance>,
}

/// Refresh bookkeeping for an INSPIRE-managed entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireProvenance {
    /// Stable INSPIRE record identifier used for refresh.
    pub record_id: u64,
    /// Provider update timestamp.
    pub updated: String,
}

impl Entry {
    /// Seed a managed entry from a durable INSPIRE snapshot.
    ///
    /// One of the two projection sites in the codebase. Afterwards the stored
    /// fields are authoritative and the BibTeX is never parsed to infer them.
    pub fn from_inspire(record: InspireSnapshot) -> Result<Self, Error> {
        let reference = record.project()?;
        Ok(Self {
            bibtex: Some(record.bibtex),
            inspire: Some(InspireProvenance {
                record_id: record.record_id,
                updated: record.updated,
            }),
            ..Self::seed(reference)
        })
    }

    /// Seed an unmanaged entry from one imported standalone BibTeX entry.
    ///
    /// The other projection site. The snapshot's bytes are stored verbatim and
    /// never rewritten.
    pub fn from_bibtex(snapshot: BibtexSnapshot) -> Result<Self, Error> {
        let reference = project_bibtex(&snapshot.bibtex)?;
        Ok(Self {
            bibtex: Some(snapshot.bibtex),
            ..Self::seed(reference)
        })
    }

    /// The first provider-owned field that differs, or `None` when every one
    /// matches. `bibtex` is deliberately absent: it is immutable for managed
    /// and unmanaged entries alike, so its caller checks it once for both.
    pub fn provider_field_change(&self, other: &Self) -> Option<&'static str> {
        [
            ("type", self.entry_type != other.entry_type),
            ("title", self.title != other.title),
            ("authors", self.authors != other.authors),
            (
                "collaborations",
                self.collaborations != other.collaborations,
            ),
            ("year", self.year != other.year),
            ("doi", self.doi != other.doi),
            ("arxiv", self.arxiv != other.arxiv),
            ("inspire", self.inspire != other.inspire),
        ]
        .into_iter()
        .find_map(|(field, changed)| changed.then_some(field))
    }

    fn seed(reference: Reference) -> Self {
        Self {
            entry_type: reference.entry_type,
            title: reference.title,
            authors: reference.authors,
            collaborations: reference.collaborations,
            year: reference.year,
            doi: reference.identifiers.dois.into_iter().next(),
            arxiv: reference.identifiers.arxiv.into_iter().next(),
            tags: BTreeSet::new(),
            notes: Vec::new(),
            bibtex: None,
            inspire: None,
        }
    }

    fn inspire_record_id(&self) -> Option<u64> {
        self.inspire.as_ref().map(|inspire| inspire.record_id)
    }

    /// Whether two entries carry the same source-owned content.
    ///
    /// Tags and notes belong to the user, so re-ingesting identical provider
    /// content is an idempotent no-op rather than a content collision, and a
    /// repeated `add` or `import` can never discard what the user wrote.
    fn has_same_content(&self, other: &Self) -> bool {
        let compared = Self {
            tags: self.tags.clone(),
            notes: self.notes.clone(),
            ..other.clone()
        };
        self == &compared
    }

    /// Overwrite every provider-owned field from a refreshed snapshot.
    ///
    /// Destructuring is deliberate: a field added to `Entry` later stops
    /// compiling here until someone decides whether a refresh owns it, so user
    /// data cannot be silently dropped by a future schema addition.
    fn refresh_from_inspire(&mut self, record: InspireSnapshot) -> Result<(), Error> {
        let Self {
            entry_type,
            title,
            authors,
            collaborations,
            year,
            doi,
            arxiv,
            tags: _,
            notes: _,
            bibtex,
            inspire,
        } = Self::from_inspire(record)?;
        self.entry_type = entry_type;
        self.title = title;
        self.authors = authors;
        self.collaborations = collaborations;
        self.year = year;
        self.doi = doi;
        self.arxiv = arxiv;
        self.bibtex = bibtex;
        self.inspire = inspire;
        Ok(())
    }
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
/// A validated entry waiting to be added under a requested key.
pub struct PendingReference {
    /// Exact or suggested local key request.
    pub key: KeyRequest,
    /// The seeded entry to store.
    pub entry: Entry,
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
/// Loaded schema-2 manifest and its coordinated bibliography artifact.
pub struct Manifest {
    path: PathBuf,
    bibliography_path: PathBuf,
    references: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestData {
    schema: u32,
    #[serde(default)]
    references: BTreeMap<String, Entry>,
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
        "unsupported cita.toml schema {found}; this version supports schema {expected} and provides no legacy migration",
        expected = SCHEMA
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
    /// A stored entry violates the schema's semantic invariants.
    #[error("invalid entry for `{key}`: {message}")]
    InvalidEntry {
        /// Local key of the invalid entry.
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
    /// An authoritative source could not be projected at ingest.
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    /// BibTeX parsing, projection, or re-keying failed.
    #[error(transparent)]
    Bibtex(#[from] cita_bibliography::Error),
    /// Deterministic TOML serialization failed.
    #[error("could not serialize manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Manifest {
    /// Create an empty schema-2 manifest and bibliography in a directory.
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
            .map(|(key, snapshot)| Ok((key, Entry::from_bibtex(snapshot)?)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        validate_references(&references)?;
        let manifest = Self {
            path,
            bibliography_path,
            references,
        };
        manifest.persist_candidate(&manifest.references)?;
        Ok(manifest)
    }

    /// Parse and semantically validate manifest bytes without touching disk.
    ///
    /// `path` only names the source in diagnostics, so a candidate buffer can
    /// be validated in full before it goes anywhere near the managed files.
    pub fn parse(source: &str, path: &Path) -> Result<BTreeMap<String, Entry>, Error> {
        let invalid = |message: String| Error::Invalid {
            path: path.to_path_buf(),
            message,
        };
        let value: toml::Value =
            toml::from_str(source).map_err(|error| invalid(error.to_string()))?;
        let schema = value
            .get("schema")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| invalid("missing integer schema".into()))?;
        if schema != i64::from(SCHEMA) {
            return Err(Error::UnsupportedSchema { found: schema });
        }
        let data: ManifestData =
            toml::from_str(source).map_err(|error| invalid(error.to_string()))?;
        validate_references(&data.references).map_err(|error| invalid(error.to_string()))?;
        Ok(data.references)
    }

    /// Load and semantically validate a manifest without checking generated output.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let source = fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let references = Self::parse(&source, &path)?;
        let bibliography_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(BIBLIOGRAPHY_FILE);
        Ok(Self {
            path,
            bibliography_path,
            references,
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
    /// Return entries ordered by local citation key.
    pub fn references(&self) -> &BTreeMap<String, Entry> {
        &self.references
    }

    /// Return stable IDs for all INSPIRE-managed references in key order.
    pub fn inspire_record_ids(&self) -> Vec<u64> {
        self.references
            .values()
            .filter_map(Entry::inspire_record_id)
            .collect()
    }

    /// Find by exact local key, then provider ID, DOI, or arXiv ID.
    ///
    /// Every comparison reads stored fields, so a lookup costs no BibTeX parse.
    pub fn find(&self, selector: &str) -> Option<(&str, &Entry)> {
        if let Some((key, entry)) = self.references.get_key_value(selector) {
            return Some((key.as_str(), entry));
        }
        let locator = selector.parse::<Locator>().ok()?;
        self.references.iter().find_map(|(key, entry)| {
            let matches = match &locator {
                Locator::Inspire(id) => entry.inspire_record_id() == Some(*id),
                Locator::Doi(id) => {
                    entry.doi.as_deref().map(normalize_doi) == Some(normalize_doi(id))
                }
                Locator::Arxiv(id) => {
                    entry.arxiv.as_deref().map(normalize_arxiv) == Some(normalize_arxiv(id))
                }
            };
            matches.then_some((key.as_str(), entry))
        })
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
        let mut index = identity_index(&candidate);
        let mut outcomes = Vec::with_capacity(pending.len());
        for item in pending {
            validate_key(item.key.as_str())?;
            let forced_collision = if let Some(record_id) = item.entry.inspire_record_id()
                && let Some(existing) = candidate.iter().find_map(|(key, entry)| {
                    (entry.inspire_record_id() == Some(record_id)).then(|| key.clone())
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
            let entry = item.entry;
            if candidate
                .get(&key)
                .is_some_and(|existing| existing.has_same_content(&entry))
            {
                outcomes.push(AddOutcome::Existing(key));
                continue;
            }
            let identities = reference_identities(&entry);
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
                    candidate.insert(key.clone(), entry);
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
                    candidate.insert(key.clone(), entry);
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
    pub fn remove_batch(&mut self, selectors: &[String]) -> Result<Vec<(String, Entry)>, Error> {
        let mut keys = Vec::new();
        for selector in selectors {
            let (key, _) = self
                .find(selector)
                .ok_or_else(|| Error::ReferenceNotFound(selector.clone()))?;
            let key = key.to_owned();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let mut candidate = self.references.clone();
        let removed = keys
            .into_iter()
            .map(|key| {
                let entry = candidate.remove(&key).expect("selected key exists");
                (key, entry)
            })
            .collect();
        self.persist_candidate(&candidate)?;
        self.references = candidate;
        Ok(removed)
    }

    /// Validate and atomically persist a complete replacement reference set.
    ///
    /// This is the whole-manifest editing path. Every field-level policy about
    /// what an edit is allowed to change belongs to the caller; this only
    /// enforces the schema's own invariants.
    pub fn replace_all(&mut self, references: BTreeMap<String, Entry>) -> Result<bool, Error> {
        let changed = references != self.references;
        if changed {
            self.persist_candidate(&references)?;
            self.references = references;
        }
        Ok(changed)
    }

    /// Refresh the complete managed INSPIRE set in place.
    ///
    /// Every provider-owned field is replaced from the returned record while
    /// the local key and the user-owned tags and notes carry forward untouched.
    pub fn replace_inspire(&mut self, refreshed: Vec<InspireSnapshot>) -> Result<bool, Error> {
        let expected = self
            .references
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .inspire_record_id()
                    .map(|record_id| (record_id, key.clone()))
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
            candidate
                .get_mut(&key)
                .expect("managed key came from this map")
                .refresh_from_inspire(record)?;
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

    /// Render a derived bibliography with this manifest's exact layout.
    ///
    /// Entries are ordered by local key and re-keyed exactly as
    /// [`Manifest::render_bibliography`] does, joined by one blank line with a
    /// single trailing newline. `transform` receives the local key, its entry,
    /// and the re-keyed BibTeX, and owns any field-level policy. Entries
    /// without stored BibTeX are skipped, as they are for the generated
    /// bibliography. This renders a separate artifact and never touches
    /// `references.bib`.
    pub fn render_derived<E: From<Error>>(
        &self,
        transform: impl FnMut(&str, &Entry, String) -> Result<String, E>,
    ) -> Result<String, E> {
        render_entries(&self.references, transform)
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

    /// Atomically regenerate the bibliography from authoritative entries.
    pub fn generate(&self) -> Result<(), Error> {
        atomic_write(
            &self.bibliography_path,
            self.render_bibliography()?.as_bytes(),
        )
    }

    fn persist_candidate(&self, candidate: &BTreeMap<String, Entry>) -> Result<(), Error> {
        validate_references(candidate)?;
        let bibliography = render_bibliography(candidate)?;
        let manifest = render_manifest(candidate)?;
        // The bibliography is prepared first. The manifest is the commit point.
        atomic_write(&self.bibliography_path, bibliography.as_bytes())?;
        atomic_write(&self.path, manifest.as_bytes())
    }
}

/// Check the schema's semantic invariants against stored fields only.
///
/// Nothing here parses `bibtex`: under schema 2 the structured fields are the
/// authority, and the cross-check between an INSPIRE record's curated
/// identifiers and its BibTeX belongs at ingest, in `cita-inspire-client`.
fn validate_references(references: &BTreeMap<String, Entry>) -> Result<(), Error> {
    let mut identities: HashMap<String, String> = HashMap::new();
    for (key, entry) in references {
        validate_key(key)?;
        if entry.title.trim().is_empty() {
            return Err(Error::InvalidEntry {
                key: key.clone(),
                message: "missing title".into(),
            });
        }
        if entry.inspire_record_id() == Some(0) {
            return Err(Error::InvalidEntry {
                key: key.clone(),
                message: "INSPIRE record id is zero".into(),
            });
        }
        for identity in reference_identities(entry) {
            record_identity(&mut identities, identity, key)?;
        }
    }
    Ok(())
}

/// Normalized identity strings (`doi:` / `arxiv:` / `inspire:`) for one entry.
fn reference_identities(entry: &Entry) -> Vec<String> {
    let mut identities = Vec::new();
    if let Some(doi) = &entry.doi {
        identities.push(format!("doi:{}", normalize_doi(doi)));
    }
    if let Some(arxiv) = &entry.arxiv {
        identities.push(format!("arxiv:{}", normalize_arxiv(arxiv)));
    }
    if let Some(record_id) = entry.inspire_record_id() {
        identities.push(format!("inspire:{record_id}"));
    }
    identities
}

/// Build an `identity -> local key` index over already-valid references.
fn identity_index(references: &BTreeMap<String, Entry>) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for (key, entry) in references {
        for identity in reference_identities(entry) {
            index.insert(identity, key.clone());
        }
    }
    index
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

/// Render re-keyed entries in local-key order, applying `transform` to each.
///
/// Layout lives here so every rendered artifact shares one set of rules. An
/// entry that carries no BibTeX has nothing to render and is skipped, which is
/// how a provider-less reference stays out of `references.bib` without
/// disturbing the byte-determinism of the entries that remain.
fn render_entries<E: From<Error>>(
    references: &BTreeMap<String, Entry>,
    mut transform: impl FnMut(&str, &Entry, String) -> Result<String, E>,
) -> Result<String, E> {
    let mut entries = Vec::with_capacity(references.len());
    for (key, entry) in references {
        let Some(bibtex) = entry.bibtex.as_deref() else {
            continue;
        };
        let rekeyed = rename_entry(bibtex, key).map_err(Error::from)?;
        entries.push(transform(key, entry, rekeyed)?.trim().to_owned());
    }
    if entries.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{}\n", entries.join("\n\n")))
}

fn render_bibliography(references: &BTreeMap<String, Entry>) -> Result<String, Error> {
    render_entries(references, |_, _, entry| Ok(entry))
}

fn render_manifest(references: &BTreeMap<String, Entry>) -> Result<String, Error> {
    let mut output = toml::to_string_pretty(&ManifestData {
        schema: SCHEMA,
        references: references.clone(),
    })?;
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

/// Durably replace a file: write a sibling temporary, fsync it, then rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    // `parent` of a bare relative file name is `Some("")`, which is not a
    // usable directory, so an empty parent falls back alongside `None`.
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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
    use cita_core::Identifiers;

    fn snapshot(key: &str, title: &str, extra: &str) -> BibtexSnapshot {
        BibtexSnapshot::new(format!("@misc{{{key},title={{{title}}},{extra}}}")).unwrap()
    }

    fn imported(key: &str, title: &str, extra: &str) -> PendingReference {
        PendingReference {
            key: KeyRequest::Exact(key.into()),
            entry: Entry::from_bibtex(snapshot(key, title, extra)).unwrap(),
        }
    }

    fn record(provider_key: &str, record_id: u64) -> InspireSnapshot {
        InspireSnapshot {
            record_id,
            updated: "2026-01-01".into(),
            texkey: provider_key.into(),
            bibtex: format!("@misc{{{provider_key},title={{Record {record_id}}}}}"),
            reference: Reference {
                entry_type: "misc".into(),
                title: format!("Record {record_id}"),
                ..Reference::default()
            },
        }
    }

    fn inspire(local_key: &str, provider_key: &str, record_id: u64) -> PendingReference {
        PendingReference {
            key: KeyRequest::Suggested(local_key.into()),
            entry: Entry::from_inspire(record(provider_key, record_id)).unwrap(),
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

    fn updated(provider_key: &str, record_id: u64, timestamp: &str) -> InspireSnapshot {
        InspireSnapshot {
            updated: timestamp.into(),
            ..record(provider_key, record_id)
        }
    }

    fn record_id(manifest: &Manifest, key: &str) -> u64 {
        manifest.references()[key]
            .inspire
            .as_ref()
            .expect("managed entry")
            .record_id
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
        assert!(text.contains("schema = 2"));
        assert!(text.contains("[references.Alpha]"), "{text}");
        assert!(text.contains("type = \"misc\""), "{text}");
        // Provenance is the presence of a provider sub-table, never a tag.
        assert!(!text.contains("source = "), "{text}");
        assert!(!text.contains("inspire"), "{text}");
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
    fn structured_entries_render_readable_diffable_toml() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![inspire("Local", "Provider:Key", 42)]).unwrap();
        let mut references = manifest.references().clone();
        let entry = references.get_mut("Local").unwrap();
        entry.tags = ["higgs", "atlas"].map(String::from).into();
        entry.notes = vec!["Superseded by 1503.07589.".into()];
        manifest.replace_all(references).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap(),
            concat!(
                "schema = 2\n",
                "\n",
                "[references.Local]\n",
                "type = \"misc\"\n",
                "title = \"Record 42\"\n",
                // An array of more than one element is expanded one per line
                // with a trailing comma, so adding a tag is a one-line diff.
                "tags = [\n",
                "    \"atlas\",\n",
                "    \"higgs\",\n",
                "]\n",
                "notes = [\"Superseded by 1503.07589.\"]\n",
                "bibtex = \"@misc{Provider:Key,title={Record 42}}\"\n",
                "\n",
                "[references.Local.inspire]\n",
                "record_id = 42\n",
                "updated = \"2026-01-01\"\n",
            )
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            "@misc{Local,title={Record 42}}\n"
        );
    }

    #[test]
    fn provider_less_references_are_stored_but_never_rendered() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("Cited", "Cited work", "")]).unwrap();
        let mut references = manifest.references().clone();
        references.insert(
            "Rovelli2004".into(),
            Entry {
                entry_type: "book".into(),
                title: "Quantum Gravity".into(),
                authors: vec!["Rovelli, Carlo".into()],
                year: Some(2004),
                tags: ["qg".to_owned()].into(),
                ..Entry::default()
            },
        );
        assert!(manifest.replace_all(references).unwrap());

        let manifest = Manifest::load_verified(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(manifest.references().contains_key("Rovelli2004"));
        // A reference no provider knows about lives in the manifest only; the
        // generated bibliography holds exactly the entries that have BibTeX,
        // and still verifies without drift.
        assert_eq!(
            fs::read_to_string(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            "@misc{Cited,title={Cited work},}\n"
        );
        manifest.verify_bibliography().unwrap();
        let text = fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(text.contains("[references.Rovelli2004]"), "{text}");
        assert!(!text.contains("bibtex = \"\""), "{text}");
    }

    #[test]
    fn an_all_provider_less_manifest_generates_an_empty_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        manifest
            .replace_all(BTreeMap::from([(
                "Book".to_owned(),
                Entry {
                    entry_type: "book".into(),
                    title: "Quantum Gravity".into(),
                    ..Entry::default()
                },
            )]))
            .unwrap();
        assert_eq!(manifest.render_bibliography().unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap(),
            ""
        );
        manifest.verify_bibliography().unwrap();
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
        changed
            .entry
            .refresh_from_inspire(InspireSnapshot {
                updated: "2026-07-17".into(),
                bibtex: "@misc{Provider:New,title={Changed provider title}}".into(),
                ..record("Provider:New", 42)
            })
            .unwrap();

        assert_eq!(
            add(&mut manifest, vec![changed]).unwrap(),
            [AddOutcome::Existing("Local".into())]
        );
        let stored = &manifest.references()["Local"];
        assert_eq!(
            stored.bibtex.as_deref(),
            Some("@misc{Provider:Old,title={Record 42}}")
        );
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
    fn re_adding_identical_content_preserves_user_owned_tags_and_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("A", "Alpha", "")]).unwrap();
        let mut references = manifest.references().clone();
        let entry = references.get_mut("A").unwrap();
        entry.tags = ["reading-list".to_owned()].into();
        entry.notes = vec!["Ask about section 3.".into()];
        manifest.replace_all(references).unwrap();
        let tagged = fs::read(dir.path().join(MANIFEST_FILE)).unwrap();

        // Tags and notes are user-owned, so re-ingesting the same provider
        // content is an idempotent no-op rather than a content collision --
        // even under the overwrite policy, which would otherwise replace them.
        for policy in [ConflictPolicy::Skip, ConflictPolicy::Overwrite] {
            assert_eq!(
                manifest
                    .add_batch(vec![imported("A", "Alpha", "")], policy)
                    .unwrap(),
                [AddOutcome::Existing("A".into())]
            );
            assert_eq!(fs::read(dir.path().join(MANIFEST_FILE)).unwrap(), tagged);
        }
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
        assert_eq!(record_id(&manifest, "Local"), 2);
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
        assert_eq!(record_id(&manifest, "Second"), 2);
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

        let mut refreshed = InspireSnapshot {
            bibtex: "@misc{Provider:New,title={Refreshed},eprint={2401.00042}}".into(),
            ..record("Provider:New", 42)
        };
        refreshed.reference.identifiers.arxiv = vec!["2401.00042".into()];
        let pending = PendingReference {
            key: KeyRequest::Exact("Renamed".into()),
            entry: Entry::from_inspire(refreshed).unwrap(),
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
        assert_eq!(record_id(&manifest, "Renamed"), 42);
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
        // Schema 1 is a hard break with no migration: it stored opaque BibTeX
        // blobs whose structured fields this release will not re-derive.
        for schema in [0, 1, 3] {
            fs::write(
                dir.path().join(MANIFEST_FILE),
                format!("schema = {schema}\n"),
            )
            .unwrap();
            assert!(matches!(
                Manifest::load(dir.path().join(MANIFEST_FILE)),
                Err(Error::UnsupportedSchema { found }) if found == schema
            ));
        }
    }

    #[test]
    fn nested_shape_and_unknown_fields_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("Alpha", "First", "")]).unwrap();
        let path = dir.path().join(MANIFEST_FILE);
        let flat = fs::read_to_string(&path).unwrap();

        let nested = flat.replace("[references.Alpha]", "[references.Alpha.entry]");
        fs::write(&path, &nested).unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
        assert_eq!(fs::read_to_string(&path).unwrap(), nested);

        fs::write(&path, format!("{flat}unknown = true\n")).unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));

        // The retired schema-1 source tag is an unknown field, not a hint.
        fs::write(
            &path,
            flat.replace("type = ", "source = \"import\"\ntype = "),
        )
        .unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
    }

    #[test]
    fn ingest_seeds_structured_fields_from_the_projected_inspire_reference() {
        // `bibtex` deliberately disagrees with `reference` here: INSPIRE JSON
        // is authoritative outright, not merely a tie-breaker over BibTeX, so
        // the entry must reflect `reference` and never fall back to parsing
        // the stored BibTeX blob.
        let entry = Entry::from_inspire(InspireSnapshot {
            record_id: 99,
            updated: "2026-01-01".into(),
            texkey: "Curated:2026".into(),
            bibtex: "@misc{Curated:2026,title={Bibtex title}}".into(),
            reference: Reference {
                entry_type: "article".into(),
                title: "Curated title".into(),
                authors: vec!["Jane Doe".into()],
                collaborations: vec!["ATLAS".into()],
                year: Some(2024),
                identifiers: Identifiers {
                    dois: vec!["10.1/bibtexcase".into()],
                    arxiv: vec!["2401.00001".into()],
                    ..Identifiers::default()
                },
            },
        })
        .unwrap();
        assert_eq!(entry.entry_type, "article");
        assert_eq!(entry.title, "Curated title");
        assert_eq!(entry.authors, ["Jane Doe"]);
        assert_eq!(entry.collaborations, ["ATLAS"]);
        assert_eq!(entry.year, Some(2024));
        assert_eq!(entry.arxiv.as_deref(), Some("2401.00001"));
        assert_eq!(entry.doi.as_deref(), Some("10.1/bibtexcase"));
        assert_eq!(
            entry.bibtex.as_deref(),
            Some("@misc{Curated:2026,title={Bibtex title}}")
        );
    }

    #[test]
    fn ingest_seeds_structured_fields_from_the_authoritative_bibtex() {
        let entry = Entry::from_bibtex(
            BibtexSnapshot::new(
                "@Article{A,title={A {NASA} result},author={Doe, Jane and Roe, Ann},year={2024}}"
                    .into(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(entry.entry_type, "article");
        assert_eq!(entry.title, "A NASA result");
        assert_eq!(entry.authors, ["Jane Doe", "Ann Roe"]);
        assert_eq!(entry.year, Some(2024));
        // An import is unmanaged: no provider sub-table, so `cita sync` never
        // touches it.
        assert!(entry.inspire.is_none());
        assert!(entry.tags.is_empty() && entry.notes.is_empty());
    }

    #[test]
    fn zero_record_id_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        assert!(matches!(
            add(&mut manifest, vec![inspire("Local", "Key", 0)]),
            Err(Error::InvalidEntry { .. })
        ));
    }

    #[test]
    fn blank_titles_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        assert!(matches!(
            manifest.replace_all(BTreeMap::from([(
                "Blank".to_owned(),
                Entry {
                    entry_type: "book".into(),
                    title: "  ".into(),
                    ..Entry::default()
                },
            )])),
            Err(Error::InvalidEntry { .. })
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
        assert_eq!(record_id(&manifest, "LocalA"), 1);
        assert_eq!(record_id(&manifest, "LocalB"), 2);
        assert!(manifest.references().contains_key("Imported"));
        let bibliography = fs::read_to_string(dir.path().join(BIBLIOGRAPHY_FILE)).unwrap();
        assert!(bibliography.contains("@misc{LocalA,"), "{bibliography}");
        assert!(bibliography.contains("@misc{LocalB,"), "{bibliography}");
    }

    #[test]
    fn refresh_replaces_provider_fields_and_carries_user_data_forward() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![inspire("Local", "Provider:Old", 7)]).unwrap();
        let mut references = manifest.references().clone();
        let entry = references.get_mut("Local").unwrap();
        entry.tags = ["higgs".to_owned(), "reading-list".to_owned()].into();
        entry.notes = vec!["Check the systematics.".into(), "Second note.".into()];
        manifest.replace_all(references).unwrap();

        assert!(
            manifest
                .replace_inspire(vec![InspireSnapshot {
                    updated: "2026-08-19".into(),
                    bibtex: "@article{Provider:New,title={Published version},doi={10.1/NEW}}"
                        .into(),
                    reference: Reference {
                        entry_type: "article".into(),
                        title: "Published version".into(),
                        identifiers: Identifiers {
                            dois: vec!["10.1/new".into()],
                            ..Identifiers::default()
                        },
                        ..Reference::default()
                    },
                    ..record("Provider:New", 7)
                }])
                .unwrap()
        );

        let entry = &manifest.references()["Local"];
        // Provider-owned fields all move to the refreshed record...
        assert_eq!(entry.entry_type, "article");
        assert_eq!(entry.title, "Published version");
        assert_eq!(entry.doi.as_deref(), Some("10.1/new"));
        assert_eq!(entry.inspire.as_ref().unwrap().updated, "2026-08-19");
        assert_eq!(
            entry.bibtex.as_deref(),
            Some("@article{Provider:New,title={Published version},doi={10.1/NEW}}")
        );
        // ...and the user's own data is byte-identical afterwards.
        assert_eq!(
            entry.tags.iter().cloned().collect::<Vec<_>>(),
            ["higgs", "reading-list"]
        );
        assert_eq!(entry.notes, ["Check the systematics.", "Second note."]);
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

    #[test]
    fn selectors_match_stored_identity_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(
            &mut manifest,
            vec![
                imported("Local", "Imported", "doi={10.1/ABC},eprint={2401.00001}"),
                inspire("Managed", "Provider:Key", 42),
            ],
        )
        .unwrap();
        for (selector, expected) in [
            ("Local", "Local"),
            ("doi:10.1/abc", "Local"),
            ("2401.00001", "Local"),
            ("arxiv:2401.00001v3", "Local"),
            ("inspire:42", "Managed"),
        ] {
            assert_eq!(manifest.find(selector).map(|(key, _)| key), Some(expected));
        }
        assert!(manifest.find("inspire:99").is_none());
        assert!(manifest.find("Unknown").is_none());

        assert_eq!(
            manifest
                .remove_batch(&["doi:10.1/ABC".to_owned()])
                .unwrap()
                .into_iter()
                .map(|(key, entry)| (key, entry.title))
                .collect::<Vec<_>>(),
            [("Local".to_owned(), "Imported".to_owned())]
        );
    }

    #[test]
    fn render_derived_matches_the_generated_bibliography_for_an_identity_transform() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(
            &mut manifest,
            vec![
                imported("Zed", "Last", "eprint={2001.00001}"),
                imported("Alpha", "First", ""),
            ],
        )
        .unwrap();
        add(&mut manifest, vec![inspire("Rec", "Prov:2026", 7)]).unwrap();
        // The derived renderer owns layout for every artifact, so the identity
        // transform must reproduce references.bib byte for byte.
        assert_eq!(
            manifest
                .render_derived::<Error>(|_, _, entry| Ok(entry))
                .unwrap(),
            manifest.render_bibliography().unwrap()
        );
    }

    #[test]
    fn render_derived_propagates_a_failing_transform() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::create(dir.path()).unwrap();
        add(&mut manifest, vec![imported("A", "A", "")]).unwrap();
        assert!(matches!(
            manifest.render_derived::<Error>(|key, _, _| Err(Error::InvalidEntry {
                key: key.into(),
                message: "no".into()
            })),
            Err(Error::InvalidEntry { .. })
        ));
    }

    #[test]
    fn render_derived_of_an_empty_manifest_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::create(dir.path()).unwrap();
        assert_eq!(
            manifest
                .render_derived::<Error>(|_, _, entry| Ok(entry))
                .unwrap(),
            ""
        );
    }
}
