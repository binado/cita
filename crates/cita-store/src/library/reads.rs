use super::{
    LibraryError, ProjectedReference, ShelfEntry, ShelfName, SyncCandidate,
    models::{ContributorRow, IdentityRow, InspireRow, MembershipRow, ReferenceRow},
    schema::{
        bibliography_references, contributors, identities, inspire_records, shelf_references,
        shelves,
    },
};
use crate::{
    SourceSnapshot,
    source::{HepIdentifiers, InspireEntry},
};
use cita_bibliography::BibtexSnapshot;
use cita_core::{Identifiers, Locator, Reference, normalize_arxiv, normalize_doi};
use diesel::{OptionalExtension, QueryDsl, SelectableHelper, prelude::*, sqlite::SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};

const LOAD_CHUNK_SIZE: usize = 900;

#[derive(Clone, Debug)]
pub(super) struct StoredReference {
    pub(super) source: SourceSnapshot,
    pub(super) reference: Reference,
}

pub(super) fn shelves(
    connection: &mut SqliteConnection,
) -> Result<BTreeSet<ShelfName>, LibraryError> {
    shelves::table
        .order(shelves::name.asc())
        .select(shelves::name)
        .load::<String>(connection)?
        .into_iter()
        .map(ShelfName::try_from)
        .collect()
}

pub(crate) fn shelf_id(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
) -> Result<i64, LibraryError> {
    shelves::table
        .filter(shelves::name.eq(shelf.as_str()))
        .select(shelves::id)
        .first(connection)
        .optional()?
        .ok_or_else(|| LibraryError::UnknownShelf(shelf.to_string()))
}

pub(super) fn entries(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
) -> Result<Vec<ShelfEntry>, LibraryError> {
    let shelf_id = shelf_id(connection, shelf)?;
    entries_for_shelf(connection, shelf_id)
}

fn entries_for_shelf(
    connection: &mut SqliteConnection,
    shelf_id: i64,
) -> Result<Vec<ShelfEntry>, LibraryError> {
    let memberships = shelf_references::table
        .filter(shelf_references::shelf_id.eq(shelf_id))
        .order(shelf_references::citation_key.asc())
        .select(MembershipRow::as_select())
        .load(connection)?;
    let ids = memberships
        .iter()
        .map(|membership| membership.reference_id)
        .collect::<Vec<_>>();
    let mut stored = load_stored_references(connection, &ids)?;
    memberships
        .into_iter()
        .map(|membership| {
            debug_assert_eq!(membership.shelf_id, shelf_id);
            let value = stored
                .remove(&membership.reference_id)
                .ok_or_else(|| missing_reference(membership.reference_id))?;
            Ok(ShelfEntry {
                id: membership.reference_id,
                key: membership.citation_key,
                source: value.source,
                reference: value.reference,
            })
        })
        .collect()
}

pub(super) fn projected(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
) -> Result<Vec<ProjectedReference>, LibraryError> {
    Ok(entries(connection, shelf)?
        .into_iter()
        .map(|entry| ProjectedReference {
            key: entry.key,
            reference: entry.reference,
        })
        .collect())
}

pub(super) fn find(
    connection: &mut SqliteConnection,
    shelf: &ShelfName,
    selector: &str,
) -> Result<Option<ProjectedReference>, LibraryError> {
    let shelf_id = shelf_id(connection, shelf)?;
    find_in_connection(connection, shelf_id, selector)
        .map(|item| item.map(|(_, projected)| projected))
}

pub(super) fn find_in_connection(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    selector: &str,
) -> Result<Option<(i64, ProjectedReference)>, LibraryError> {
    let Some((id, key)) = resolve_membership(connection, shelf_id, selector)? else {
        return Ok(None);
    };
    Ok(Some((
        id,
        ProjectedReference {
            key,
            reference: load_reference(connection, id)?,
        },
    )))
}

fn resolve_membership(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    selector: &str,
) -> Result<Option<(i64, String)>, LibraryError> {
    let exact = shelf_references::table
        .filter(shelf_references::shelf_id.eq(shelf_id))
        .filter(shelf_references::citation_key.eq(selector))
        .select((
            shelf_references::reference_id,
            shelf_references::citation_key,
        ))
        .first(connection)
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }

    let locator = match selector.parse::<Locator>() {
        Ok(locator) => locator,
        Err(_) => return Ok(None),
    };
    match locator {
        Locator::Inspire(record_id) => {
            let Ok(record_id) = i64::try_from(record_id) else {
                return Ok(None);
            };
            Ok(shelf_references::table
                .inner_join(
                    inspire_records::table
                        .on(inspire_records::reference_id.eq(shelf_references::reference_id)),
                )
                .filter(shelf_references::shelf_id.eq(shelf_id))
                .filter(inspire_records::record_id.eq(record_id))
                .select((
                    shelf_references::reference_id,
                    shelf_references::citation_key,
                ))
                .first(connection)
                .optional()?)
        }
        Locator::Doi(value) => {
            identity_membership(connection, shelf_id, "doi", &normalize_doi(&value))
        }
        Locator::Arxiv(value) => {
            identity_membership(connection, shelf_id, "arxiv", &normalize_arxiv(&value))
        }
    }
}

fn identity_membership(
    connection: &mut SqliteConnection,
    shelf_id: i64,
    kind: &str,
    value: &str,
) -> Result<Option<(i64, String)>, LibraryError> {
    Ok(shelf_references::table
        .inner_join(
            identities::table.on(identities::reference_id.eq(shelf_references::reference_id)),
        )
        .filter(shelf_references::shelf_id.eq(shelf_id))
        .filter(identities::kind.eq(kind))
        .filter(identities::value.eq(value))
        .select((
            shelf_references::reference_id,
            shelf_references::citation_key,
        ))
        .first(connection)
        .optional()?)
}

pub(super) fn sync_candidates(
    connection: &mut SqliteConnection,
    shelf: Option<&ShelfName>,
) -> Result<Vec<SyncCandidate>, LibraryError> {
    let ids = match shelf {
        Some(shelf) => {
            let shelf_id = shelf_id(connection, shelf)?;
            shelf_references::table
                .filter(shelf_references::shelf_id.eq(shelf_id))
                .order(shelf_references::reference_id.asc())
                .select(shelf_references::reference_id)
                .load(connection)?
        }
        None => bibliography_references::table
            .order(bibliography_references::id.asc())
            .select(bibliography_references::id)
            .load(connection)?,
    };
    let mut stored = load_stored_references(connection, &ids)?;
    ids.into_iter()
        .map(|id| {
            let value = stored.remove(&id).ok_or_else(|| missing_reference(id))?;
            Ok(SyncCandidate {
                id,
                source: value.source,
            })
        })
        .collect()
}

pub(super) fn load_source(
    connection: &mut SqliteConnection,
    id: i64,
) -> Result<SourceSnapshot, LibraryError> {
    load_stored_reference(connection, id).map(|value| value.source)
}

fn load_reference(connection: &mut SqliteConnection, id: i64) -> Result<Reference, LibraryError> {
    load_stored_reference(connection, id).map(|value| value.reference)
}

fn load_stored_reference(
    connection: &mut SqliteConnection,
    id: i64,
) -> Result<StoredReference, LibraryError> {
    load_stored_references(connection, &[id])?
        .remove(&id)
        .ok_or_else(|| missing_reference(id))
}

pub(super) fn load_stored_references(
    connection: &mut SqliteConnection,
    ids: &[i64],
) -> Result<BTreeMap<i64, StoredReference>, LibraryError> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut base_rows = Vec::new();
    let mut contributor_rows = Vec::new();
    let mut identity_rows = Vec::new();
    let mut inspire_rows = Vec::new();
    for chunk in ids.chunks(LOAD_CHUNK_SIZE) {
        base_rows.extend(
            bibliography_references::table
                .filter(bibliography_references::id.eq_any(chunk))
                .select(ReferenceRow::as_select())
                .load(connection)?,
        );
        contributor_rows.extend(
            contributors::table
                .filter(contributors::reference_id.eq_any(chunk))
                .order((
                    contributors::reference_id.asc(),
                    contributors::kind.asc(),
                    contributors::position.asc(),
                ))
                .select(ContributorRow::as_select())
                .load(connection)?,
        );
        identity_rows.extend(
            identities::table
                .filter(identities::reference_id.eq_any(chunk))
                .order((
                    identities::reference_id.asc(),
                    identities::kind.asc(),
                    identities::value.asc(),
                ))
                .select(IdentityRow::as_select())
                .load(connection)?,
        );
        inspire_rows.extend(
            inspire_records::table
                .filter(inspire_records::reference_id.eq_any(chunk))
                .select(InspireRow::as_select())
                .load(connection)?,
        );
    }

    let mut contributors_by_id: BTreeMap<i64, Vec<ContributorRow>> = BTreeMap::new();
    for row in contributor_rows {
        contributors_by_id
            .entry(row.reference_id)
            .or_default()
            .push(row);
    }
    let mut identities_by_id: BTreeMap<i64, Vec<IdentityRow>> = BTreeMap::new();
    for row in identity_rows {
        identities_by_id
            .entry(row.reference_id)
            .or_default()
            .push(row);
    }
    let mut inspire_by_id = inspire_rows
        .into_iter()
        .map(|row| (row.reference_id, row))
        .collect::<BTreeMap<_, _>>();

    let mut stored = BTreeMap::new();
    for row in base_rows {
        let id = row.id;
        let contributor_rows = contributors_by_id.remove(&id).unwrap_or_default();
        let identity_rows = identities_by_id.remove(&id).unwrap_or_default();
        let value = assemble_reference(
            row,
            contributor_rows,
            identity_rows,
            inspire_by_id.remove(&id),
        )?;
        stored.insert(id, value);
    }
    Ok(stored)
}

fn assemble_reference(
    row: ReferenceRow,
    contributor_rows: Vec<ContributorRow>,
    identity_rows: Vec<IdentityRow>,
    inspire_row: Option<InspireRow>,
) -> Result<StoredReference, LibraryError> {
    let mut authors = Vec::new();
    let mut collaborations = Vec::new();
    for contributor in contributor_rows {
        if contributor.position < 0 {
            return Err(LibraryError::InvalidSource(format!(
                "stored contributor position {} is negative",
                contributor.position
            )));
        }
        match contributor.kind.as_str() {
            "author" => authors.push(contributor.name),
            "collaboration" => collaborations.push(contributor.name),
            kind => {
                return Err(LibraryError::InvalidSource(format!(
                    "unknown contributor kind `{kind}`"
                )));
            }
        }
    }

    let mut dois = Vec::new();
    let mut arxiv = Vec::new();
    let mut canonical = HepIdentifiers::default();
    for identity in identity_rows {
        match identity.kind.as_str() {
            "doi" => {
                if identity.canonical {
                    set_canonical(&mut canonical.doi, identity.value.clone(), "doi")?;
                }
                dois.push(identity.value);
            }
            "arxiv" => {
                if identity.canonical {
                    set_canonical(&mut canonical.arxiv, identity.value.clone(), "arxiv")?;
                }
                arxiv.push(identity.value);
            }
            kind => {
                return Err(LibraryError::InvalidSource(format!(
                    "unknown identity kind `{kind}`"
                )));
            }
        }
    }

    let source = match row.source_kind.as_str() {
        "import" => SourceSnapshot::Import(BibtexSnapshot::new(row.bibtex.clone())?),
        "inspire" => {
            let inspire = inspire_row.ok_or_else(|| {
                LibraryError::InvalidSource(format!(
                    "stored INSPIRE reference {} has no INSPIRE record",
                    row.id
                ))
            })?;
            SourceSnapshot::Inspire(InspireEntry {
                record_id: source_record_id(inspire.record_id)?,
                updated: inspire.updated,
                bibtex: row.bibtex.clone(),
                identifiers: canonical,
            })
        }
        kind => {
            return Err(LibraryError::InvalidSource(format!(
                "unknown source kind `{kind}`"
            )));
        }
    };

    Ok(StoredReference {
        source,
        reference: Reference {
            title: row.title,
            authors,
            collaborations,
            year: row.year,
            identifiers: Identifiers { dois, arxiv },
        },
    })
}

fn set_canonical(slot: &mut Option<String>, value: String, kind: &str) -> Result<(), LibraryError> {
    if slot.replace(value).is_some() {
        return Err(LibraryError::InvalidSource(format!(
            "stored reference has multiple canonical {kind} identities"
        )));
    }
    Ok(())
}

pub(super) fn source_record_id(record_id: i64) -> Result<u64, LibraryError> {
    u64::try_from(record_id)
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            LibraryError::InvalidSource(format!(
                "stored INSPIRE record id {record_id} is not positive"
            ))
        })
}

fn missing_reference(id: i64) -> LibraryError {
    LibraryError::InvalidSource(format!("stored reference {id} is missing"))
}
