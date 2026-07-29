//! The only ingestion verb.
//!
//! `add <locator>` resolves through providers; `add -f <file>` resolves each
//! entry and keeps the user's own BibTeX only where every applicable provider
//! reports absence. A single hand-written entry is a one-entry file, so there
//! is no second rule for when the local escape hatch applies.

use crate::{
    error::Error,
    reports::{BatchReport, ItemFailure, SkipReason, SkippedItem},
    services::Services,
};
use bibi_bibtex::{BibtexEntry, CitationKey, parse_file};
use bibi_core::{BibiId, ProviderName, QualifiedLocator, Record};
use bibi_manifest::{ManifestCandidate, ManifestStore};
use bibi_provider::{LocatorOutcome, ProviderRecord, Resolution};
use std::{io::Read, path::PathBuf};

/// Where `add -f` reads entries from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputSource {
    /// Standard input, for `-`.
    Stdin,
    /// A file the user named.
    Path(PathBuf),
}

/// Options for `add <locator>…`.
#[derive(Clone, Debug, Default)]
pub struct AddRequest {
    /// Override the citation key. Only valid for a single locator.
    pub key: Option<CitationKey>,
    /// Constrain every locator to one provider.
    pub provider: Option<ProviderName>,
    /// Replace a matching record instead of skipping it.
    pub overwrite: bool,
    /// Report what would happen without writing.
    pub dry_run: bool,
}

/// Options for `add -f <file>`.
#[derive(Clone, Debug)]
pub struct AddFileRequest {
    /// Where to read entries from.
    pub source: InputSource,
    /// Constrain every entry to one provider.
    pub provider: Option<ProviderName>,
    /// Replace matching records instead of skipping them.
    pub overwrite: bool,
    /// Store every entry locally without consulting any provider.
    pub force_local: bool,
    /// Report what would happen without writing.
    pub dry_run: bool,
}

/// Whether an item created a record or replaced one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddKind {
    /// A new record was stored.
    Added,
    /// An existing record's provider-owned data was replaced.
    Overwritten,
}

/// One record an add produced.
#[derive(Clone, Debug)]
pub struct AddedRecord {
    /// The local citation key it is stored under.
    pub key: CitationKey,
    /// Which provider owns it.
    pub provider: ProviderName,
    /// Its BibTeX, under the local key.
    pub bibtex: String,
    /// What happened to it.
    pub kind: AddKind,
}

/// What an add did.
#[derive(Debug)]
pub struct AddReport {
    /// Per-item outcomes, in input order.
    pub items: BatchReport<AddedRecord>,
    /// Whether the manifest was written.
    pub committed: bool,
}

/// Resolve locators and store what comes back.
pub async fn add_locators(
    services: &Services,
    store: &ManifestStore,
    locators: &[String],
    request: &AddRequest,
) -> Result<AddReport, Error> {
    if request.key.is_some() && locators.len() != 1 {
        return Err(Error::usage(
            "`--key` names one record, so it accepts exactly one locator",
        ));
    }
    let mut items = BatchReport::new();
    let mut parsed = Vec::new();
    for raw in locators {
        match raw.parse::<QualifiedLocator>() {
            Ok(locator) => parsed.push((raw.clone(), locator)),
            Err(error) => items
                .failures
                .push(ItemFailure::new(raw, error.to_string())),
        }
    }
    let (mut candidate, generation) = store.load_or_empty()?.into_candidate();
    let requests = parsed
        .iter()
        .map(|(_, locator)| locator.clone())
        .collect::<Vec<_>>();
    let outcomes = services
        .providers
        .resolve(&requests, request.provider.as_ref())
        .await?;

    for ((raw, _), outcome) in parsed.iter().zip(outcomes) {
        match outcome {
            LocatorOutcome::Found { record, .. } => {
                let key = request
                    .key
                    .clone()
                    .unwrap_or_else(|| record.payload.source_key().clone());
                let placement = plan(
                    &candidate,
                    &key,
                    &record,
                    request.overwrite,
                    request.key.is_some(),
                    // A single locator can be retried with `--key`, so a
                    // collision is a failure the user can act on.
                    CollisionPolicy::Fail,
                );
                apply(&mut candidate, raw, key, *record, placement, &mut items)?;
            }
            LocatorOutcome::NotFound => items.failures.push(ItemFailure::new(
                raw,
                "no provider holds a record for this locator",
            )),
            LocatorOutcome::Unrecognized => items.failures.push(ItemFailure::new(
                raw,
                "no installed provider recognizes this id; qualify it as `<provider>:<id>`",
            )),
            LocatorOutcome::Ambiguous { providers } => items.failures.push(ItemFailure::new(
                raw,
                format!(
                    "`{}` all recognize this id; name one with `--provider`",
                    providers
                        .iter()
                        .map(ProviderName::to_string)
                        .collect::<Vec<_>>()
                        .join("`, `")
                ),
            )),
            LocatorOutcome::Failed { provider, error } => items.failures.push(ItemFailure::new(
                raw,
                format!("{error}; retry, or choose another provider than `{provider}`"),
            )),
        }
    }

    commit(store, &generation, candidate, items, request.dry_run)
}

/// Resolve every entry in a file, keeping the user's BibTeX only where absent.
pub async fn add_file(
    services: &Services,
    store: &ManifestStore,
    request: &AddFileRequest,
) -> Result<AddReport, Error> {
    if request.force_local && request.provider.is_some() {
        return Err(Error::usage(
            "`--force-local` stores entries without consulting a provider, so it cannot be combined with `--provider`",
        ));
    }
    let source = read(&request.source)?;
    // Parsing everything first means a malformed file costs no requests.
    let entries = parse_file(&source)?;
    let (mut candidate, generation) = store.load_or_empty()?.into_candidate();
    let mut items = BatchReport::new();

    // One entry can offer two locators — a DOI and an arXiv id for one paper is
    // ordinary in an imported file — so the batch is flattened and each entry
    // remembers which slice of it is its own.
    let mut requests: Vec<QualifiedLocator> = Vec::new();
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for entry in &entries {
        let start = requests.len();
        if !request.force_local {
            let candidates = entry.payload.identifier_candidates();
            let locators = [
                candidates.doi.as_deref().map(|doi| format!("doi:{doi}")),
                candidates
                    .arxiv
                    .as_deref()
                    .map(|arxiv| format!("arxiv:{arxiv}")),
            ];
            for locator in locators.into_iter().flatten() {
                if let Ok(parsed) = locator.parse::<QualifiedLocator>() {
                    requests.push(parsed);
                }
            }
        }
        spans.push(start..requests.len());
    }
    let outcomes = if requests.is_empty() {
        Vec::new()
    } else {
        services
            .providers
            .resolve(&requests, request.provider.as_ref())
            .await?
    };

    for (entry, span) in entries.into_iter().zip(spans) {
        let name = entry.key.to_string();
        let resolved = resolve_entry(&outcomes[span]);
        let record = match resolved {
            EntryResolution::Found(record) => *record,
            EntryResolution::Failed(message) => {
                // A provider failure says nothing about whether the work
                // exists, so it must never quietly become a local record.
                items.failures.push(ItemFailure::new(name, message));
                continue;
            }
            EntryResolution::Absent => match ingest(services, entry.payload.clone()) {
                Ok(record) => record,
                Err(message) => {
                    items.failures.push(ItemFailure::new(name, message));
                    continue;
                }
            },
        };
        let placement = plan(
            &candidate,
            &entry.key,
            &record,
            request.overwrite,
            false,
            // A file has no per-entry `--key`, so failing the item would leave
            // the user no way forward short of editing a colleague's file.
            CollisionPolicy::Skip,
        );
        apply(
            &mut candidate,
            &name,
            entry.key.clone(),
            record,
            placement,
            &mut items,
        )?;
    }

    commit(store, &generation, candidate, items, request.dry_run)
}

/// What resolving one imported entry's identifiers produced.
enum EntryResolution {
    Found(Box<ProviderRecord>),
    Absent,
    Failed(String),
}

/// Reduce one entry's locator outcomes to a single decision.
///
/// An imported entry offers only DOI and arXiv locators, so the outcomes that
/// concern a bare provider id cannot arise here.
fn resolve_entry(outcomes: &[LocatorOutcome]) -> EntryResolution {
    let mut failure = None;
    let mut found: Option<Box<ProviderRecord>> = None;
    for outcome in outcomes {
        match outcome {
            LocatorOutcome::Found { record, .. } => {
                if let Some(first) = &found {
                    if !same_provider_record(first, record) {
                        return EntryResolution::Failed(format!(
                            "the entry's identifiers resolve to different provider records (`{}` and `{}`)",
                            provider_identity(first),
                            provider_identity(record)
                        ));
                    }
                } else {
                    found = Some(record.clone());
                }
            }
            LocatorOutcome::Failed { provider, error } => {
                failure.get_or_insert_with(|| {
                    format!("{error}; retry, or choose another provider than `{provider}`")
                });
            }
            LocatorOutcome::NotFound
            | LocatorOutcome::Ambiguous { .. }
            | LocatorOutcome::Unrecognized => {}
        }
    }
    match failure {
        Some(message) => EntryResolution::Failed(message),
        None => found
            .map(EntryResolution::Found)
            .unwrap_or(EntryResolution::Absent),
    }
}

fn same_provider_record(left: &ProviderRecord, right: &ProviderRecord) -> bool {
    match (left.provenance.identity(), right.provenance.identity()) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn provider_identity(record: &ProviderRecord) -> String {
    record
        .provenance
        .identity()
        .map(|(provider, id)| format!("{provider}:{id}"))
        .unwrap_or_else(|| format!("{}:<no stable id>", record.provenance.provider))
}

/// Hand an entry to the provider that ingests user-supplied BibTeX.
fn ingest(services: &Services, entry: BibtexEntry) -> Result<ProviderRecord, String> {
    let provider = services
        .providers
        .ingest_provider()
        .ok_or("no provider in this build can store a user-supplied entry")?;
    match provider.ingest(entry) {
        Ok(Resolution::Found(record)) => Ok(*record),
        Ok(_) => Err("the ingest provider refused this entry".to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// What to do with one resolved record.
enum Placement {
    Insert,
    Overwrite(BibiId),
    Skip(SkipReason),
    Fail(String),
}

/// What a citation-key collision means for this command.
#[derive(Clone, Copy, Eq, PartialEq)]
enum CollisionPolicy {
    Fail,
    Skip,
}

/// Decide where a resolved record goes.
///
/// bibi never appends a suffix to make a key unique, because the result would
/// be neither the provider's key nor the user's. Every collision therefore ends
/// in a skip, an error, or an explicit overwrite.
fn plan(
    candidate: &ManifestCandidate,
    key: &CitationKey,
    record: &ProviderRecord,
    overwrite: bool,
    explicit_key: bool,
    collisions: CollisionPolicy,
) -> Placement {
    let duplicates = candidate
        .duplicates_of(&record.identifiers, &record.provenance)
        .map(|existing| (existing.id, existing.key.clone()));
    let duplicates = duplicates.collect::<Vec<_>>();
    if duplicates.len() > 1 {
        return Placement::Fail(format!(
            "this work matches multiple records (`{}`); remove or repair the duplicates before adding it",
            duplicates
                .iter()
                .map(|(_, key)| key.to_string())
                .collect::<Vec<_>>()
                .join("`, `")
        ));
    }
    let duplicate = duplicates.into_iter().next();
    let holder = candidate
        .by_key(key)
        .map(|existing| (existing.id, existing.key.clone()));

    if !overwrite {
        return match (duplicate, holder) {
            (Some((_, existing)), _) => Placement::Skip(SkipReason::Duplicate { existing }),
            (None, Some((_, existing))) => match collisions {
                CollisionPolicy::Skip => Placement::Skip(SkipReason::KeyCollision { existing }),
                CollisionPolicy::Fail => Placement::Fail(format!(
                    "citation key `{existing}` already belongs to another record; choose one with `--key`"
                )),
            },
            (None, None) => Placement::Insert,
        };
    }

    match (duplicate, holder) {
        (Some((duplicate_id, _)), Some((holder_id, _))) if duplicate_id == holder_id => {
            Placement::Overwrite(duplicate_id)
        }
        // Two different records could be the target, which the design settles
        // the same way from either direction: bibi never guesses which to replace.
        (Some((_, matched)), Some((_, keyed))) => Placement::Fail(format!(
            "this work matches record `{matched}`, but key `{keyed}` belongs to a different record; remove one or name the target with `--key`"
        )),
        (Some((_, matched)), None) if explicit_key => Placement::Fail(format!(
            "this work matches record `{matched}`, whose key differs from the requested `{key}`; `--key` names an overwrite target and cannot rename one, so run `rename` separately"
        )),
        (Some((duplicate_id, _)), None) => Placement::Overwrite(duplicate_id),
        // A colliding key identifies the overwrite target: the same mechanism
        // as `add --overwrite --key <existing>`, reached from a file.
        (None, Some((holder_id, _))) => Placement::Overwrite(holder_id),
        (None, None) => Placement::Insert,
    }
}

/// Carry out a placement and record its outcome.
fn apply(
    candidate: &mut ManifestCandidate,
    item: &str,
    key: CitationKey,
    record: ProviderRecord,
    placement: Placement,
    items: &mut BatchReport<AddedRecord>,
) -> Result<(), Error> {
    match placement {
        Placement::Insert => {
            let provider = record.provenance.provider.clone();
            let stored = Record::new(BibiId::new(), key, record.into())?;
            let id = stored.id;
            candidate.insert(stored)?;
            items
                .successes
                .push(added(candidate, &id, provider, AddKind::Added)?);
        }
        Placement::Overwrite(id) => {
            let provider = record.provenance.provider.clone();
            candidate.replace(&id, record.into())?;
            items
                .successes
                .push(added(candidate, &id, provider, AddKind::Overwritten)?);
        }
        Placement::Skip(reason) => {
            // A skip still emits the stored record's BibTeX, so a bulk add's
            // stdout is the complete set of entries it was asked about.
            let existing = match &reason {
                SkipReason::Duplicate { existing } | SkipReason::KeyCollision { existing } => {
                    candidate
                        .by_key(existing)
                        .map(Record::rendered)
                        .transpose()?
                }
                SkipReason::AlreadyRemoved { .. } => None,
            };
            items.skipped.push(SkippedItem {
                item: item.to_owned(),
                reason,
                bibtex: existing,
            });
        }
        Placement::Fail(message) => items.failures.push(ItemFailure::new(item, message)),
    }
    Ok(())
}

fn added(
    candidate: &ManifestCandidate,
    id: &BibiId,
    provider: ProviderName,
    kind: AddKind,
) -> Result<AddedRecord, Error> {
    let stored = candidate
        .get(id)
        .expect("the record was just written into the candidate");
    Ok(AddedRecord {
        key: stored.key.clone(),
        provider,
        bibtex: stored.rendered()?,
        kind,
    })
}

/// Commit the successes, unless there are none or this is a dry run.
fn commit(
    store: &ManifestStore,
    generation: &bibi_manifest::Generation,
    candidate: ManifestCandidate,
    items: BatchReport<AddedRecord>,
    dry_run: bool,
) -> Result<AddReport, Error> {
    let committed = !dry_run && !items.successes.is_empty();
    if committed {
        store.commit(generation, candidate)?;
    }
    Ok(AddReport { items, committed })
}

fn read(source: &InputSource) -> Result<String, Error> {
    match source {
        InputSource::Stdin => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|error| Error::io("<stdin>", error))?;
            Ok(buffer)
        }
        InputSource::Path(path) => {
            std::fs::read_to_string(path).map_err(|error| Error::io(path, error))
        }
    }
}
