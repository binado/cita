//! Conditional refresh.
//!
//! Sync fetches a narrowed structured record for everything it manages,
//! compares each provider's opaque revision token, and fetches BibTeX only for
//! what changed. A typical refresh therefore transfers no BibTeX at all, which
//! is what makes it cheap enough to run often.

use crate::{error::Error, reports::ItemFailure, services::Services};
use bibi_bibtex::CitationKey;
use bibi_core::{
    BibiId, Description, IdentifierChange, Identifiers, Provenance, ProviderId, ProviderName,
    ProviderOwned, Record, Revision,
};
use bibi_manifest::{ManifestCandidate, ManifestStore};
use bibi_provider::{PayloadRequest, Provider, ProviderMetadata, RefreshRequest, RefreshState};
use std::{collections::BTreeMap, sync::Arc};

/// Options for `sync`.
#[derive(Clone, Debug, Default)]
pub struct SyncRequest {
    /// Refresh only records owned by this provider.
    pub provider: Option<ProviderName>,
    /// Refetch every refreshable record, whatever its revision says.
    pub force: bool,
    /// Do the work and report it, but write nothing.
    pub dry_run: bool,
}

/// One record a sync updated.
#[derive(Clone, Debug)]
pub struct RefreshedRecord {
    /// Its local key, which a refresh never changes.
    pub key: CitationKey,
    /// Its new BibTeX, under that key.
    pub bibtex: String,
}

/// An identifier a record gained.
///
/// Reported individually, unlike description changes: a preprint gaining a DOI
/// may create a duplicate relationship with an existing record, and that should
/// not pass silently.
#[derive(Clone, Debug)]
pub struct IdentifierAddition {
    /// The record that gained it.
    pub key: CitationKey,
    /// Which identifier.
    pub kind: &'static str,
    /// Its new value.
    pub value: String,
}

/// A record sync left alone with a warning (not a failure).
#[derive(Clone, Debug)]
pub struct SyncAbsence {
    /// The record that was left unchanged.
    pub key: CitationKey,
    /// Why nothing was written.
    pub reason: SyncAbsenceReason,
}

/// Why a sync left a record unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncAbsenceReason {
    /// The provider answered [`RefreshState::Missing`].
    ProviderGone,
    /// Metadata indicated a change, but no BibTeX payload arrived.
    PayloadAbsent,
}

impl std::fmt::Display for SyncAbsenceReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderGone => write!(
                formatter,
                "its provider no longer holds this record; it was left unchanged"
            ),
            Self::PayloadAbsent => write!(
                formatter,
                "provider metadata changed but no BibTeX arrived; it was left unchanged"
            ),
        }
    }
}

/// What a sync did.
#[derive(Debug, Default)]
pub struct SyncReport {
    /// How many records were considered.
    pub examined: usize,
    /// How many were already current.
    pub unchanged: usize,
    /// The records that were updated.
    pub refreshed: Vec<RefreshedRecord>,
    /// How many updates changed a title, author list, or year.
    pub description_changes: usize,
    /// Identifiers that were added.
    pub identifier_additions: Vec<IdentifierAddition>,
    /// Records left unchanged with a warning.
    pub absences: Vec<SyncAbsence>,
    /// Records no provider will ever refresh.
    pub unrefreshable: usize,
    /// Records owned by providers this build does not carry.
    pub unavailable: BTreeMap<ProviderName, usize>,
    /// Records that could not be refreshed.
    pub failures: Vec<ItemFailure>,
    /// Whether the manifest was written.
    pub committed: bool,
}

impl SyncReport {
    /// True when any record failed, which is what makes the exit nonzero.
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }
}

/// Refresh managed records, conditionally unless forced.
pub async fn sync(
    services: &Services,
    store: &ManifestStore,
    request: &SyncRequest,
) -> Result<SyncReport, Error> {
    // Naming a provider this build does not carry is a usage error, raised
    // before any I/O — unlike a plain sync, where such records are skipped.
    if let Some(name) = &request.provider {
        services.providers.require(name)?;
    }
    let (manifest, generation) = store.load()?.into_parts();
    let mut candidate = manifest.to_candidate();
    let mut report = SyncReport::default();

    let mut groups: BTreeMap<ProviderName, Vec<Managed>> = BTreeMap::new();
    for record in manifest.records() {
        if request
            .provider
            .as_ref()
            .is_some_and(|name| record.provenance.provider != *name)
        {
            continue;
        }
        report.examined += 1;
        match &record.provenance.provider_id {
            // A record with no handle cannot be asked about: there is nothing
            // to send. This is a property of the record, not a test for any
            // particular provider's name.
            None => report.unrefreshable += 1,
            Some(provider_id) => groups
                .entry(record.provenance.provider.clone())
                .or_default()
                .push(Managed {
                    id: record.id,
                    key: record.key.clone(),
                    provider_id: provider_id.clone(),
                    revision: record.provenance.revision.clone(),
                    identifiers: record.identifiers.clone(),
                    description: record.description.clone(),
                }),
        }
    }

    for (name, records) in groups {
        let Some(provider) = services.providers.get(&name) else {
            // A manifest may legitimately name a provider this build lacks.
            // Refusing to refresh anything on that account would make the file
            // unmaintainable by the build that can still maintain most of it.
            report.unavailable.insert(name, records.len());
            continue;
        };
        refresh_group(
            provider,
            services,
            &records,
            request,
            &mut candidate,
            &mut report,
        )
        .await;
    }

    report.committed = !request.dry_run && !report.refreshed.is_empty();
    if report.committed {
        store.commit(&generation, candidate)?;
    }
    Ok(report)
}

/// One record as sync sees it before asking its provider.
struct Managed {
    id: BibiId,
    key: CitationKey,
    provider_id: ProviderId,
    revision: Option<Revision>,
    identifiers: Identifiers,
    description: Description,
}

/// Refresh every record owned by one provider.
async fn refresh_group(
    provider: &Arc<dyn Provider>,
    services: &Services,
    records: &[Managed],
    request: &SyncRequest,
    candidate: &mut ManifestCandidate,
    report: &mut SyncReport,
) {
    let requests = records
        .iter()
        .map(|record| RefreshRequest {
            bibi_id: record.id,
            provider_id: record.provider_id.clone(),
            stored_revision: record.revision.clone(),
        })
        .collect::<Vec<_>>();
    let items = match services
        .providers
        .refresh_metadata(provider, &requests)
        .await
    {
        Ok(items) => items,
        Err(error) => {
            for record in records {
                report
                    .failures
                    .push(ItemFailure::new(record.key.to_string(), error.to_string()));
            }
            return;
        }
    };

    let by_id = records
        .iter()
        .map(|record| (record.id, record))
        .collect::<BTreeMap<_, _>>();
    let mut changed: Vec<(&Managed, Box<ProviderMetadata>)> = Vec::new();
    for item in items {
        let Some(record) = by_id.get(&item.bibi_id) else {
            continue;
        };
        match item.result {
            Err(error) => report
                .failures
                .push(ItemFailure::new(record.key.to_string(), error.to_string())),
            Ok(RefreshState::Unrefreshable) => report.unrefreshable += 1,
            // Absence is a no-op with a warning. bibi does not re-resolve by
            // identifier on its own: that would be a silent rebind inside a
            // bulk operation, discovered late and corrupting a deliverable.
            Ok(RefreshState::Missing) => report.absences.push(SyncAbsence {
                key: record.key.clone(),
                reason: SyncAbsenceReason::ProviderGone,
            }),
            Ok(RefreshState::Metadata(metadata)) => {
                if metadata.provider_id != record.provider_id {
                    report.failures.push(ItemFailure::new(
                        record.key.to_string(),
                        format!(
                            "provider now reports id `{}` instead of `{}`; migrate it explicitly with `add --overwrite --provider`",
                            metadata.provider_id, record.provider_id
                        ),
                    ));
                    continue;
                }
                let current = !request.force
                    && record.revision.is_some()
                    && metadata.revision.is_some()
                    && record.revision == metadata.revision;
                if current {
                    report.unchanged += 1;
                } else {
                    changed.push((record, metadata));
                }
            }
        }
    }

    if changed.is_empty() {
        return;
    }
    let requests = changed
        .iter()
        .map(|(record, metadata)| PayloadRequest {
            provider_id: record.provider_id.clone(),
            join_tokens: metadata.join_tokens.clone(),
        })
        .collect::<Vec<_>>();
    let payloads = match services.providers.fetch_payloads(provider, &requests).await {
        Ok(items) => items
            .into_iter()
            .map(|item| (item.provider_id, item.payload))
            .collect::<BTreeMap<_, _>>(),
        Err(error) => {
            // An ambiguous join gave no trustworthy pairing for any record it
            // covered, so none is written. Other providers still commit.
            for (record, _) in &changed {
                report
                    .failures
                    .push(ItemFailure::new(record.key.to_string(), error.to_string()));
            }
            return;
        }
    };

    for (record, metadata) in changed {
        let Some(Some(payload)) = payloads.get(&record.provider_id).cloned() else {
            // Metadata arrived and the payload did not. Nothing about this
            // record is written — above all not the revision, since an advanced
            // revision beside an old payload would desync the two permanently
            // *and* suppress the repair on the next sync.
            report.absences.push(SyncAbsence {
                key: record.key.clone(),
                reason: SyncAbsenceReason::PayloadAbsent,
            });
            continue;
        };
        let doi = IdentifierChange::classify(
            record.identifiers.doi.as_ref(),
            metadata.identifiers.doi.as_ref(),
        );
        let arxiv = IdentifierChange::classify(
            record.identifiers.arxiv.as_ref(),
            metadata.identifiers.arxiv.as_ref(),
        );
        if let Some(message) = replacement(&doi, "DOI").or_else(|| replacement(&arxiv, "arXiv id"))
        {
            report
                .failures
                .push(ItemFailure::new(record.key.to_string(), message));
            continue;
        }

        let owned = ProviderOwned {
            provenance: Provenance::managed(
                provider.name().clone(),
                metadata.provider_id.clone(),
                metadata.revision.clone(),
            ),
            identifiers: Identifiers {
                doi: metadata
                    .identifiers
                    .doi
                    .clone()
                    .or(record.identifiers.doi.clone()),
                arxiv: metadata
                    .identifiers
                    .arxiv
                    .clone()
                    .or(record.identifiers.arxiv.clone()),
            },
            payload,
            description: metadata.description.clone(),
        };
        if let Err(error) = candidate.replace(&record.id, owned) {
            report
                .failures
                .push(ItemFailure::new(record.key.to_string(), error.to_string()));
            continue;
        }

        if let IdentifierChange::Added(value) = &doi {
            report.identifier_additions.push(IdentifierAddition {
                key: record.key.clone(),
                kind: "DOI",
                value: value.to_string(),
            });
        }
        if let IdentifierChange::Added(value) = &arxiv {
            report.identifier_additions.push(IdentifierAddition {
                key: record.key.clone(),
                kind: "arXiv id",
                value: value.to_string(),
            });
        }
        if metadata.description != record.description {
            // Description changes are routine — a corrected title, a grown
            // author list — so they are reported in aggregate.
            report.description_changes += 1;
        }
        match candidate.get(&record.id).map(Record::rendered).transpose() {
            Ok(Some(bibtex)) => report.refreshed.push(RefreshedRecord {
                key: record.key.clone(),
                bibtex,
            }),
            Ok(None) | Err(_) => report.failures.push(ItemFailure::new(
                record.key.to_string(),
                "the refreshed record could not be rendered under its local key",
            )),
        }
    }
}

/// A replaced identifier fails its record.
///
/// These values do not change once set, so a replacement means the stored
/// value, the new value, or the provider is wrong — and none of the three
/// should be committed by a routine refresh.
fn replacement<T: std::fmt::Display>(
    change: &IdentifierChange<T>,
    kind: &'static str,
) -> Option<String> {
    match change {
        IdentifierChange::Replaced { old, new } => Some(format!(
            "provider reports {kind} `{new}` where `{old}` is stored; identifiers do not change once set, so repair this with `add --overwrite`"
        )),
        _ => None,
    }
}
