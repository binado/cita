use super::{
    AddOutcome, ConflictPolicy, LibraryError, ProjectedReference, ShelfName, SkippedReference,
    SyncUpdate,
    models::{InspireRow, NewContributor, NewIdentity, NewReference, NewShelf},
    reads,
    schema::{
        bibliography_references, contributors, identities, inspire_records, shelf_references,
        shelves,
    },
};
use crate::{KeyRequest, PendingReference, SourceSnapshot};
use cita_bibliography::validate_key;
use cita_core::{Reference, normalize_arxiv, normalize_doi};
use diesel::{
    Connection, OptionalExtension, QueryDsl, RunQueryDsl, dsl::exists, dsl::not, prelude::*,
    sqlite::SqliteConnection,
};
use std::collections::BTreeSet;

pub(super) fn create_shelf(
    connection: &mut SqliteConnection,
    requested: &ShelfName,
) -> Result<bool, LibraryError> {
    connection.immediate_transaction(|connection| {
        let existing = shelves::table
            .select(shelves::name)
            .load::<String>(connection)?
            .into_iter()
            .find(|name| name.eq_ignore_ascii_case(requested.as_str()));
        if let Some(existing) = existing {
            if existing != requested.as_str() {
                return Err(LibraryError::ShelfAlias {
                    requested: requested.to_string(),
                    existing,
                });
            }
            return Ok(false);
        }
        diesel::insert_into(shelves::table)
            .values(NewShelf {
                name: requested.as_str(),
            })
            .execute(connection)?;
        Ok(true)
    })
}

pub(super) fn add_batch(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
    pending: Vec<PendingReference>,
    policy: ConflictPolicy,
) -> Result<Vec<AddOutcome>, LibraryError> {
    for item in &pending {
        validate_pending(item)?;
    }
    connection.immediate_transaction(|connection| {
        let shelf_id = reads::shelf_id(connection, shelf)?;
        pending
            .into_iter()
            .map(|item| add_one(connection, shelf_id, item, policy))
            .collect()
    })
}

pub(super) fn add_batch_skipping_errors(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
    pending: Vec<PendingReference>,
    policy: ConflictPolicy,
) -> Result<(Vec<AddOutcome>, Vec<SkippedReference>), LibraryError> {
    connection.immediate_transaction(|connection| {
        let shelf_id = reads::shelf_id(connection, shelf)?;
        let mut outcomes = Vec::new();
        let mut skipped = Vec::new();
        for item in pending {
            let key = item.key.as_str().to_owned();
            if let Err(error) = validate_pending(&item) {
                skipped.push(SkippedReference {
                    key,
                    message: error.to_string(),
                });
                continue;
            }
            match connection.transaction(|connection| add_one(connection, shelf_id, item, policy)) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => skipped.push(SkippedReference {
                    key,
                    message: error.to_string(),
                }),
            }
        }
        Ok((outcomes, skipped))
    })
}

fn validate_pending(item: &PendingReference) -> Result<(), LibraryError> {
    validate_key(item.key.as_str())?;
    item.source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    Ok(())
}

pub(super) fn remove_batch(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
    selectors: &[String],
) -> Result<Vec<ProjectedReference>, LibraryError> {
    connection.immediate_transaction(|connection| {
        let shelf_id = reads::shelf_id(connection, shelf)?;
        let mut selected = Vec::new();
        for selector in selectors {
            let item = reads::find_in_connection(connection, shelf_id, selector)?
                .ok_or_else(|| LibraryError::ReferenceNotFound(selector.clone()))?;
            if !selected
                .iter()
                .any(|(id, _): &(i64, ProjectedReference)| id == &item.0)
            {
                selected.push(item);
            }
        }
        for (reference_id, _) in &selected {
            diesel::delete(
                shelf_references::table
                    .filter(shelf_references::shelf_id.eq(shelf_id))
                    .filter(shelf_references::reference_id.eq(reference_id)),
            )
            .execute(connection)?;
            delete_if_orphan(connection, *reference_id)?;
        }
        Ok(selected.into_iter().map(|(_, item)| item).collect())
    })
}

pub(super) fn apply_sync(
    connection: &mut SqliteConnection,
    updates: Vec<SyncUpdate>,
) -> Result<usize, LibraryError> {
    connection.immediate_transaction(|connection| {
        let mut changed = 0;
        for update in updates {
            let current = reads::load_source(connection, update.id)?;
            if current != update.expected {
                return Err(LibraryError::ConcurrentChange);
            }
            if current == update.replacement {
                continue;
            }
            ensure_source_identities_available(connection, &update.replacement, Some(update.id))?;
            replace_reference(connection, update.id, &update.replacement)?;
            changed += 1;
        }
        Ok(changed)
    })
}

pub(crate) fn source_identities(
    source: &SourceSnapshot,
) -> Result<Vec<(String, String)>, LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    let mut values = reference
        .identifiers
        .arxiv
        .into_iter()
        .map(|value| ("arxiv".into(), normalize_arxiv(&value)))
        .chain(
            reference
                .identifiers
                .dois
                .into_iter()
                .map(|value| ("doi".into(), normalize_doi(&value))),
        )
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    Ok(values)
}

fn matching_reference_ids(
    connection: &mut SqliteConnection,
    source: &SourceSnapshot,
    exclude: Option<i64>,
) -> Result<BTreeSet<i64>, LibraryError> {
    let mut matches = BTreeSet::new();
    for (kind, value) in source_identities(source)? {
        if let Some(id) = identities::table
            .filter(identities::kind.eq(kind))
            .filter(identities::value.eq(value))
            .select(identities::reference_id)
            .first(connection)
            .optional()?
            && Some(id) != exclude
        {
            matches.insert(id);
        }
    }
    if let Some(entry) = source.inspire_entry()
        && let Some(id) = inspire_records::table
            .filter(inspire_records::record_id.eq(sql_record_id(entry.record_id)?))
            .select(inspire_records::reference_id)
            .first(connection)
            .optional()?
        && Some(id) != exclude
    {
        matches.insert(id);
    }
    Ok(matches)
}

fn ensure_source_identities_available(
    connection: &mut SqliteConnection,
    source: &SourceSnapshot,
    exclude: Option<i64>,
) -> Result<(), LibraryError> {
    if matching_reference_ids(connection, source, exclude)?.is_empty() {
        Ok(())
    } else {
        Err(LibraryError::IdentityConflict)
    }
}

pub(crate) fn insert_reference(
    connection: &mut SqliteConnection,
    source: &SourceSnapshot,
) -> Result<i64, LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    let id = diesel::insert_into(bibliography_references::table)
        .values(NewReference {
            source_kind: source_kind(source),
            bibtex: source.raw_bibtex(),
            title: &reference.title,
            year: reference.year,
        })
        .returning(bibliography_references::id)
        .get_result(connection)?;
    insert_details(connection, id, source, &reference)?;
    Ok(id)
}

fn insert_details(
    connection: &mut SqliteConnection,
    id: i64,
    source: &SourceSnapshot,
    reference: &Reference,
) -> Result<(), LibraryError> {
    for (kind, values) in [
        ("author", &reference.authors),
        ("collaboration", &reference.collaborations),
    ] {
        for (position, name) in values.iter().enumerate() {
            diesel::insert_into(contributors::table)
                .values(NewContributor {
                    reference_id: id,
                    kind,
                    position: position as i64,
                    name,
                })
                .execute(connection)?;
        }
    }

    let canonical = source.inspire_entry().map(|entry| &entry.identifiers);
    for (kind, value) in source_identities(source)? {
        let is_canonical = canonical.is_some_and(|ids| match kind.as_str() {
            "arxiv" => ids.arxiv.as_deref() == Some(value.as_str()),
            "doi" => ids.doi.as_deref() == Some(value.as_str()),
            _ => false,
        });
        diesel::insert_into(identities::table)
            .values(NewIdentity {
                reference_id: id,
                kind: &kind,
                value: &value,
                canonical: is_canonical,
            })
            .execute(connection)?;
    }

    if let Some(entry) = source.inspire_entry() {
        diesel::insert_into(inspire_records::table)
            .values(InspireRow {
                reference_id: id,
                record_id: sql_record_id(entry.record_id)?,
                updated: entry.updated.clone(),
            })
            .execute(connection)?;
    }
    Ok(())
}

fn replace_reference(
    connection: &mut SqliteConnection,
    id: i64,
    source: &SourceSnapshot,
) -> Result<(), LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    diesel::delete(contributors::table.filter(contributors::reference_id.eq(id)))
        .execute(connection)?;
    diesel::delete(identities::table.filter(identities::reference_id.eq(id)))
        .execute(connection)?;
    diesel::delete(inspire_records::table.filter(inspire_records::reference_id.eq(id)))
        .execute(connection)?;
    diesel::update(bibliography_references::table.filter(bibliography_references::id.eq(id)))
        .set((
            bibliography_references::source_kind.eq(source_kind(source)),
            bibliography_references::bibtex.eq(source.raw_bibtex()),
            bibliography_references::title.eq(reference.title.as_str()),
            bibliography_references::year.eq(reference.year),
        ))
        .execute(connection)?;
    insert_details(connection, id, source, &reference)
}

fn add_one(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    item: PendingReference,
    policy: ConflictPolicy,
) -> Result<AddOutcome, LibraryError> {
    let matching = matching_reference_ids(connection, &item.source, None)?;
    if matching.len() > 1 {
        return Err(LibraryError::IdentityConflict);
    }
    let existing_global = matching.first().copied();
    let inserted = existing_global.is_none();
    let reference_id = match existing_global {
        Some(id) => id,
        None => insert_reference(connection, &item.source)?,
    };
    let existing_key = shelf_references::table
        .filter(shelf_references::shelf_id.eq(shelf_id))
        .filter(shelf_references::reference_id.eq(reference_id))
        .select(shelf_references::citation_key)
        .first::<String>(connection)
        .optional()?;
    let suggested = matches!(&item.key, KeyRequest::Suggested(_));
    let requested = item.key.into_string();
    let key_owner = shelf_references::table
        .filter(shelf_references::shelf_id.eq(shelf_id))
        .filter(shelf_references::citation_key.eq(&requested))
        .select(shelf_references::reference_id)
        .first::<i64>(connection)
        .optional()?;

    if let Some(existing_key) = existing_key {
        if suggested || existing_key == requested {
            promote_if_needed(connection, reference_id, &item.source)?;
            return Ok(AddOutcome::Existing(existing_key));
        }
        if matches!(policy, ConflictPolicy::Skip) {
            if inserted {
                delete_if_orphan(connection, reference_id)?;
            }
            return Ok(AddOutcome::Skipped {
                key: requested,
                conflicting: existing_key,
            });
        }
        let mut replaced = vec![existing_key];
        if let Some(owner) = key_owner
            && owner != reference_id
        {
            delete_membership(connection, shelf_id, owner)?;
            delete_if_orphan(connection, owner)?;
            replaced.push(requested.clone());
        }
        diesel::update(
            shelf_references::table
                .filter(shelf_references::shelf_id.eq(shelf_id))
                .filter(shelf_references::reference_id.eq(reference_id)),
        )
        .set(shelf_references::citation_key.eq(&requested))
        .execute(connection)?;
        promote_if_needed(connection, reference_id, &item.source)?;
        return Ok(AddOutcome::Overwritten {
            key: requested,
            replaced,
        });
    }

    if let Some(owner) = key_owner {
        if matches!(policy, ConflictPolicy::Skip) {
            if inserted {
                delete_if_orphan(connection, reference_id)?;
            }
            return Ok(AddOutcome::Skipped {
                key: requested.clone(),
                conflicting: requested,
            });
        }
        delete_membership(connection, shelf_id, owner)?;
        delete_if_orphan(connection, owner)?;
        insert_membership(connection, shelf_id, reference_id, &requested)?;
        promote_if_needed(connection, reference_id, &item.source)?;
        return Ok(AddOutcome::Overwritten {
            key: requested.clone(),
            replaced: vec![requested],
        });
    }

    insert_membership(connection, shelf_id, reference_id, &requested)?;
    promote_if_needed(connection, reference_id, &item.source)?;
    Ok(AddOutcome::Added(requested))
}

fn promote_if_needed(
    connection: &mut SqliteConnection,
    id: i64,
    incoming: &SourceSnapshot,
) -> Result<(), LibraryError> {
    if incoming.inspire_entry().is_some()
        && reads::load_source(connection, id)?
            .inspire_entry()
            .is_none()
    {
        ensure_source_identities_available(connection, incoming, Some(id))?;
        replace_reference(connection, id, incoming)?;
    }
    Ok(())
}

fn delete_membership(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    reference_id: i64,
) -> Result<(), LibraryError> {
    diesel::delete(
        shelf_references::table
            .filter(shelf_references::shelf_id.eq(shelf_id))
            .filter(shelf_references::reference_id.eq(reference_id)),
    )
    .execute(connection)?;
    Ok(())
}

fn delete_if_orphan(
    connection: &mut SqliteConnection,
    reference_id: i64,
) -> Result<(), LibraryError> {
    diesel::delete(
        bibliography_references::table
            .filter(bibliography_references::id.eq(reference_id))
            .filter(not(exists(
                shelf_references::table.filter(shelf_references::reference_id.eq(reference_id)),
            ))),
    )
    .execute(connection)?;
    Ok(())
}

pub(crate) fn clear_library(connection: &mut SqliteConnection) -> Result<(), LibraryError> {
    diesel::delete(shelf_references::table).execute(connection)?;
    diesel::delete(shelves::table).execute(connection)?;
    diesel::delete(bibliography_references::table).execute(connection)?;
    Ok(())
}

pub(crate) fn insert_shelf(
    connection: &mut SqliteConnection,
    name: &str,
) -> Result<(), LibraryError> {
    diesel::insert_into(shelves::table)
        .values(NewShelf { name })
        .execute(connection)?;
    Ok(())
}

pub(crate) fn insert_membership(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    reference_id: i64,
    citation_key: &str,
) -> Result<(), LibraryError> {
    diesel::insert_into(shelf_references::table)
        .values((
            shelf_references::shelf_id.eq(shelf_id),
            shelf_references::reference_id.eq(reference_id),
            shelf_references::citation_key.eq(citation_key),
        ))
        .execute(connection)?;
    Ok(())
}

fn source_kind(source: &SourceSnapshot) -> &'static str {
    if source.inspire_entry().is_some() {
        "inspire"
    } else {
        "import"
    }
}

fn sql_record_id(record_id: u64) -> Result<i64, LibraryError> {
    if record_id == 0 {
        return Err(LibraryError::InvalidSource(
            "INSPIRE record id is zero".into(),
        ));
    }
    i64::try_from(record_id).map_err(|_| {
        LibraryError::InvalidSource(format!(
            "INSPIRE record id {record_id} exceeds SQLite's integer range"
        ))
    })
}
