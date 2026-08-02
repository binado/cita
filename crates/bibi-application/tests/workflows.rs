use bibi_application::{
    AddRequest, ImportRequest, Services, SyncRequest, add, import, remove, sync,
};
use bibi_core::{ProviderName, remote::testing::FakeProvider};
use bibi_manifest::BibliographyStore;
use bibi_provider::testing::{provider_record, providers};
use std::sync::Arc;

fn store() -> (tempfile::TempDir, BibliographyStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = BibliographyStore::new(directory.path().join("bibi.toml"));
    (directory, store)
}

fn services(answer: Vec<bibi_core::RecordState>) -> Services {
    Services::new(Arc::new(providers(Arc::new(
        FakeProvider::new(ProviderName::Inspire).answering(Ok(answer)),
    ))))
}

#[test]
fn import_is_strict_and_preserves_exact_bibtex() {
    let (_directory, store) = store();
    let source = "@misc{One, title={First}}\n\n@misc{Two, title={Second}}";
    let report = import(&store, source, &ImportRequest::default()).unwrap();
    assert_eq!(report.results.len(), 2);
    let loaded = store.load().unwrap().bibliography;
    assert_eq!(
        loaded.records()[0].state().bibtex().source(),
        "@misc{One, title={First}}"
    );

    let before = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        import(
            &store,
            "@misc{Three, title={Third}}\n@string{bad = {x}}",
            &ImportRequest::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), before);
}

#[tokio::test]
async fn add_and_sync_preserve_uuid_while_accepting_new_texkey() {
    let (_directory, store) = store();
    add(
        &services(vec![provider_record("42", "OldKey", "Old")]),
        &store,
        &["42".into()],
        &AddRequest::default(),
    )
    .await
    .unwrap();
    let original = store.load().unwrap().bibliography.records()[0].id();

    let report = sync(
        &services(vec![provider_record("42", "NewKey", "New")]),
        &store,
        &SyncRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.results[0].previous_texkey, "OldKey");
    assert_eq!(report.results[0].texkey, "NewKey");
    let loaded = store.load().unwrap();
    let record = &loaded.bibliography.records()[0];
    assert_eq!(record.id(), original);
    assert_eq!(record.texkey(), "NewKey");
}

#[test]
fn duplicate_removal_is_all_or_nothing() {
    let (_directory, store) = store();
    import(&store, "@misc{One,title={One}}", &ImportRequest::default()).unwrap();
    let before = std::fs::read_to_string(store.path()).unwrap();
    assert!(remove(&store, &["k:One".into(), "k:One".into()], false).is_err());
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), before);
}
