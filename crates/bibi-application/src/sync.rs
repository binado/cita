//! Conditional refresh.
//!
//! Sync fetches a narrowed structured record for everything it manages,
//! compares each provider's opaque revision token, and fetches BibTeX only for
//! what changed. A typical refresh therefore transfers no BibTeX at all, which
//! is what makes it cheap enough to run often.

use crate::{error::Error, reports::ItemFailure, services::Services};
use bibi_bibtex::CitationKey;
use bibi_core::{
    BibiId, Description, IdentifierChange, Identifiers, ProviderId, ProviderName, Record, Revision,
};
use bibi_manifest::{ManifestCandidate, ManifestStore};
use bibi_provider::{Provider, RefreshOptions, RefreshOutcome, RefreshTarget};
use std::collections::BTreeMap;

/// Options for `sync`.
#[derive(Clone, Debug, Default)]
pub struct SyncRequest {
    /// Refresh only records owned by this provider.
    pub provider: Option<Provider>,
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
    /// The provider answered [`RefreshOutcome::Missing`].
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
    let (manifest, generation) = store.load()?.into_parts();
    let mut candidate = manifest.to_candidate();
    let mut report = SyncReport::default();

    let mut groups: BTreeMap<ProviderName, Vec<Managed>> = BTreeMap::new();
    for record in manifest.records() {
        if request
            .provider
            .as_ref()
            .is_some_and(|name| record.provenance.provider.as_str() != name.as_str())
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
        let Some(owner) = services.providers.owner(&name) else {
            // A manifest may legitimately name a provider this build lacks.
            // Refusing to refresh anything on that account would make the file
            // unmaintainable by the build that can still maintain most of it.
            report.unavailable.insert(name, records.len());
            continue;
        };
        if !owner.is_remote() {
            report.unrefreshable += records.len();
            continue;
        }
        refresh_group(
            owner,
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
    owner: Provider,
    services: &Services,
    records: &[Managed],
    request: &SyncRequest,
    candidate: &mut ManifestCandidate,
    report: &mut SyncReport,
) {
    let targets = records
        .iter()
        .map(|record| RefreshTarget {
            bibi_id: record.id,
            provider_id: record.provider_id.clone(),
            stored_revision: record.revision.clone(),
            identifiers: record.identifiers.clone(),
        })
        .collect::<Vec<_>>();
    let items = match services
        .providers
        .refresh(
            owner,
            &targets,
            RefreshOptions {
                force: request.force,
            },
        )
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
    for item in items {
        let Some(record) = by_id.get(&item.bibi_id) else {
            continue;
        };
        match item.result {
            Err(error) => report
                .failures
                .push(ItemFailure::new(record.key.to_string(), error.to_string())),
            Ok(RefreshOutcome::Unchanged) => report.unchanged += 1,
            Ok(RefreshOutcome::Missing) => report.absences.push(SyncAbsence {
                key: record.key.clone(),
                reason: SyncAbsenceReason::ProviderGone,
            }),
            Ok(RefreshOutcome::PayloadMissing) => report.absences.push(SyncAbsence {
                key: record.key.clone(),
                reason: SyncAbsenceReason::PayloadAbsent,
            }),
            Ok(RefreshOutcome::Updated(owned)) => {
                let doi = IdentifierChange::classify(
                    record.identifiers.doi.as_ref(),
                    owned.identifiers.doi.as_ref(),
                );
                let arxiv = IdentifierChange::classify(
                    record.identifiers.arxiv.as_ref(),
                    owned.identifiers.arxiv.as_ref(),
                );
                let description_changed = owned.description != record.description;
                if let Err(error) = candidate.replace(&record.id, *owned) {
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
                if description_changed {
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
    }
}
