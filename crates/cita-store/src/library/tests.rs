use super::*;
use crate::{
    KeyRequest,
    library::schema::{bibliography_references, inspire_records},
    source::{HepIdentifiers, InspireEntry},
};
use cita_bibliography::BibtexSnapshot;
use diesel::{
    QueryableByName, RunQueryDsl,
    connection::{InstrumentationEvent, SimpleConnection},
    prelude::*,
    sql_query,
    sql_types::{BigInt, Text},
};

fn imported(key: &str, title: &str, extra: &str) -> PendingReference {
    PendingReference {
        key: KeyRequest::Exact(key.into()),
        source: SourceSnapshot::Import(
            BibtexSnapshot::new(format!(
                "@article{{Upstream,\n  title = {{{title}}}{extra}\n}}"
            ))
            .unwrap(),
        ),
    }
}

#[test]
fn one_reference_can_have_different_keys_in_two_shelves() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let other = ShelfName::try_from("paper").unwrap();
    library.create_shelf(&other).unwrap();
    let main = ShelfName::default_shelf();
    library
        .add_batch(
            &main,
            vec![imported("One", "Shared", ",\n  doi = {10.1/shared}")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    library
        .add_batch(
            &other,
            vec![imported("Two", "Different raw", ",\n  doi = {10.1/shared}")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    assert_eq!(library.entries(&main).unwrap()[0].key, "One");
    assert_eq!(library.entries(&other).unwrap()[0].key, "Two");
    assert_eq!(
        library.entries(&main).unwrap()[0].source,
        library.entries(&other).unwrap()[0].source
    );
}

#[test]
fn overwrite_is_local_and_removal_collects_orphans() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    library
        .add_batch(
            &main,
            vec![imported("K", "First", "")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    library
        .add_batch(
            &main,
            vec![imported("K", "Second", "")],
            ConflictPolicy::Overwrite,
        )
        .unwrap();
    assert_eq!(library.entries(&main).unwrap()[0].reference.title, "Second");
    library.remove_batch(&main, &["K".into()]).unwrap();
    let count = bibliography_references::table
        .count()
        .get_result::<i64>(&mut library.connection().unwrap())
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn lenient_batch_uses_savepoints_and_commits_valid_entries() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    let (outcomes, skipped) = library
        .add_batch_skipping_errors(
            &main,
            vec![imported("bad key", "Bad", ""), imported("Good", "Good", "")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    assert_eq!(outcomes, [AddOutcome::Added("Good".into())]);
    assert_eq!(skipped.len(), 1);
    assert_eq!(library.entries(&main).unwrap().len(), 1);
}

#[test]
fn identities_cannot_implicitly_merge_two_global_references() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    library
        .add_batch(
            &main,
            vec![
                imported("Doi", "DOI", ",\n  doi = {10.1/one}"),
                imported(
                    "Arxiv",
                    "arXiv",
                    ",\n  eprint = {2001.00001},\n  archivePrefix = {arXiv}",
                ),
            ],
            ConflictPolicy::Skip,
        )
        .unwrap();
    assert_eq!(
        library.find(&main, "doi:10.1/ONE").unwrap().unwrap().key,
        "Doi"
    );
    assert_eq!(
        library
            .find(&main, "arxiv:2001.00001v2")
            .unwrap()
            .unwrap()
            .key,
        "Arxiv"
    );
    let error = library
        .add_batch(
            &main,
            vec![imported(
                "Bridge",
                "Bridge",
                ",\n  doi = {10.1/one},\n  eprint = {2001.00001},\n  archivePrefix = {arXiv}",
            )],
            ConflictPolicy::Overwrite,
        )
        .unwrap_err();
    assert!(matches!(error, LibraryError::IdentityConflict));
    assert_eq!(library.entries(&main).unwrap().len(), 2);
}

#[test]
fn inspire_records_use_integer_ids_and_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    let source = SourceSnapshot::Inspire(InspireEntry {
        record_id: 42,
        updated: "2026-01-01".into(),
        bibtex: "@article{Provider,\n title={Managed},\n doi={10.1/managed}\n}".into(),
        identifiers: HepIdentifiers::new(None, Some("10.1/managed".into())),
    });
    library
        .add_batch(
            &main,
            vec![PendingReference {
                key: KeyRequest::Suggested("Managed".into()),
                source: source.clone(),
            }],
            ConflictPolicy::Skip,
        )
        .unwrap();

    let record_id = inspire_records::table
        .select(inspire_records::record_id)
        .first::<i64>(&mut library.connection().unwrap())
        .unwrap();
    let storage_class = sql_query("SELECT typeof(record_id) AS storage_class FROM inspire_records")
        .get_result::<StorageClass>(&mut library.connection().unwrap())
        .unwrap()
        .storage_class;
    assert_eq!(record_id, 42);
    assert_eq!(storage_class, "integer");
    assert_eq!(library.entries(&main).unwrap()[0].source, source);
    assert_eq!(
        library.find(&main, "inspire:42").unwrap().unwrap().key,
        "Managed"
    );
}

#[test]
fn inspire_record_ids_outside_sqlite_range_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let source = SourceSnapshot::Inspire(InspireEntry {
        record_id: u64::MAX,
        updated: "2026-01-01".into(),
        bibtex: "@article{Provider,\n title={Managed}\n}".into(),
        identifiers: HepIdentifiers::default(),
    });
    let error = library
        .add_batch(
            &ShelfName::default_shelf(),
            vec![PendingReference {
                key: KeyRequest::Suggested("Managed".into()),
                source,
            }],
            ConflictPolicy::Skip,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        LibraryError::InvalidSource(message) if message.contains("SQLite")
    ));
}

#[test]
fn stale_sync_results_are_rejected_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    library
        .add_batch(
            &main,
            vec![imported("Local", "Imported", ",\n  doi = {10.1/shared}")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    let candidate = library.sync_candidates(None).unwrap().pop().unwrap();
    let managed = SourceSnapshot::Inspire(InspireEntry {
        record_id: 42,
        updated: "2026-01-01".into(),
        bibtex: "@article{Provider,\n title={Managed},\n doi={10.1/shared}\n}".into(),
        identifiers: HepIdentifiers::new(None, Some("10.1/shared".into())),
    });
    library
        .add_batch(
            &main,
            vec![PendingReference {
                key: KeyRequest::Suggested("Local".into()),
                source: managed.clone(),
            }],
            ConflictPolicy::Skip,
        )
        .unwrap();
    assert_eq!(
        library.find(&main, "inspire:42").unwrap().unwrap().key,
        "Local"
    );
    let error = library
        .apply_sync(vec![SyncUpdate {
            id: candidate.id,
            expected: candidate.source,
            replacement: managed,
        }])
        .unwrap_err();
    assert!(matches!(error, LibraryError::ConcurrentChange));
    assert_eq!(
        library.entries(&main).unwrap()[0].reference.title,
        "Managed"
    );
}

#[test]
fn connections_enable_required_sqlite_guarantees() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let mut connection = library.connection().unwrap();
    let foreign_keys = sql_query("PRAGMA foreign_keys")
        .get_result::<ForeignKeys>(&mut connection)
        .unwrap()
        .foreign_keys;
    let journal = sql_query("PRAGMA journal_mode")
        .get_result::<JournalMode>(&mut connection)
        .unwrap()
        .journal_mode;
    let synchronous = sql_query("PRAGMA synchronous")
        .get_result::<Synchronous>(&mut connection)
        .unwrap()
        .synchronous;
    let busy_timeout = sql_query("PRAGMA busy_timeout")
        .get_result::<BusyTimeout>(&mut connection)
        .unwrap()
        .timeout;
    assert_eq!(foreign_keys, 1);
    assert_eq!(journal, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(busy_timeout, 5000);
}

#[test]
fn shelf_case_aliases_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    library
        .create_shelf(&ShelfName::try_from("Paper").unwrap())
        .unwrap();
    let error = library
        .create_shelf(&ShelfName::try_from("paper").unwrap())
        .unwrap_err();
    assert!(matches!(error, LibraryError::ShelfAlias { .. }));
}

#[test]
fn existing_schema_one_library_loads_without_migration() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let library = Library::open_or_create(&root).unwrap();
    library
        .add_batch(
            &ShelfName::default_shelf(),
            vec![imported("Existing", "Existing", "")],
            ConflictPolicy::Skip,
        )
        .unwrap();
    drop(library);

    let loaded = Library::load(root).unwrap();
    assert_eq!(
        loaded.entries(&ShelfName::default_shelf()).unwrap()[0].key,
        "Existing"
    );
}

#[test]
fn unsupported_schema_is_rejected_without_reinitializing() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let library = Library::open_or_create(&root).unwrap();
    library
        .connection()
        .unwrap()
        .batch_execute("PRAGMA user_version = 2;")
        .unwrap();
    let error = Library::load(root).unwrap_err();
    assert!(matches!(
        error,
        LibraryError::UnsupportedSchema { found: 2 }
    ));
}

#[test]
fn exact_key_wins_before_locator_resolution() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    library
        .add_batch(
            &main,
            vec![
                imported("2001.00001", "Exact key", ""),
                imported(
                    "ByIdentity",
                    "Identity",
                    ",\n  eprint = {2001.00001},\n  archivePrefix = {arXiv}",
                ),
            ],
            ConflictPolicy::Skip,
        )
        .unwrap();
    let selected = library.find(&main, "2001.00001").unwrap().unwrap();
    assert_eq!(selected.key, "2001.00001");
    assert_eq!(selected.reference.title, "Exact key");
}

#[test]
fn hydration_queries_are_bounded_by_id_chunks() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let root = tempfile::tempdir().unwrap();
    let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
    let main = ShelfName::default_shelf();
    let pending = (0..901)
        .map(|index| imported(&format!("K{index:04}"), &format!("Title {index}"), ""))
        .collect();
    library
        .add_batch(&main, pending, ConflictPolicy::Skip)
        .unwrap();

    let queries = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&queries);
    let mut connection = library.connection().unwrap();
    connection.set_instrumentation(move |event: InstrumentationEvent<'_>| {
        if matches!(event, InstrumentationEvent::StartQuery { .. }) {
            observed.fetch_add(1, Ordering::Relaxed);
        }
    });
    let entries = reads::entries(&mut connection, &main).unwrap();

    assert_eq!(entries.len(), 901);
    assert_eq!(queries.load(Ordering::Relaxed), 10);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_library_paths_round_trip_through_sqlite_url() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let root = tempfile::tempdir_in("/private/tmp").unwrap();
    let path = root.path().join(OsString::from_vec(b"cita-\xff".to_vec()));
    let library = Library::open_or_create(&path).unwrap();
    assert!(library.path().is_file());
    assert_eq!(Library::load(path).unwrap().root(), library.root());
}

#[cfg(unix)]
#[test]
fn non_utf8_library_paths_are_percent_encoded() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    let path = PathBuf::from("/tmp").join(OsString::from_vec(b"cita-\xff".to_vec()));
    let url = connection::database_url(&path).unwrap();
    assert!(url.as_str().ends_with("cita-%FF"));
}

#[derive(QueryableByName)]
struct StorageClass {
    #[diesel(sql_type = Text)]
    storage_class: String,
}

#[derive(QueryableByName)]
struct ForeignKeys {
    #[diesel(sql_type = BigInt)]
    foreign_keys: i64,
}

#[derive(QueryableByName)]
struct JournalMode {
    #[diesel(sql_type = Text)]
    journal_mode: String,
}

#[derive(QueryableByName)]
struct Synchronous {
    #[diesel(sql_type = BigInt)]
    synchronous: i64,
}

#[derive(QueryableByName)]
struct BusyTimeout {
    #[diesel(sql_type = BigInt, column_name = "timeout")]
    timeout: i64,
}
