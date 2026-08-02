//! Schema determinism, exact payload round trips, and store semantics.

use bibi_core::{
    ArxivId, Bibliography, Bibtex, Description, Doi, Identifiers, ProviderId, ProviderName,
    RecordId, RecordState, Source,
};
use bibi_manifest::{BibliographyStore, Error};

const POPULATED: &str = include_str!("golden/populated.toml");

fn state(source: Source, texkey: &str, title: &str) -> RecordState {
    RecordState::new(
        source,
        Identifiers::new(
            (texkey == "Aad:2012tfa").then(|| Doi::new("10.1016/j.physletb.2012.08.020").unwrap()),
            (texkey == "Aad:2012tfa").then(|| ArxivId::new("1207.7214").unwrap()),
        ),
        Description::new(
            title,
            if texkey == "Aad:2012tfa" {
                vec!["Aad, G.".into(), "Abajyan, T.".into()]
            } else {
                Vec::new()
            },
            if texkey == "Aad:2012tfa" {
                vec!["ATLAS".into()]
            } else {
                Vec::new()
            },
            (texkey == "Aad:2012tfa").then_some(2012),
        ),
        Bibtex::new(format!("@misc{{{texkey}, title={{{title}}}}}"), texkey),
    )
    .unwrap()
}

fn populated() -> Bibliography {
    Bibliography::restore(vec![
        (
            "8f14e45f-ceea-467a-9f38-a2f2b1a1f1c9"
                .parse::<RecordId>()
                .unwrap(),
            state(Source::Local, "Zenodo:2024", "A tool"),
        ),
        (
            "d760f219-9098-4b49-9f62-10cbbcc22b11"
                .parse::<RecordId>()
                .unwrap(),
            state(
                Source::managed(ProviderName::Inspire, ProviderId::new("1124337").unwrap()),
                "Aad:2012tfa",
                "Observation of a new particle",
            ),
        ),
    ])
    .unwrap()
}

fn store() -> (tempfile::TempDir, BibliographyStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = BibliographyStore::new(directory.path().join("bibi.toml"));
    (directory, store)
}

#[test]
fn populated_output_is_canonical_and_round_trips_byte_identically() {
    let (_directory, store) = store();
    let generation = store.load_or_empty().unwrap().into_parts().1;
    store.commit(&generation, &populated()).unwrap();
    let first = std::fs::read_to_string(store.path()).unwrap();
    assert_eq!(first, POPULATED);

    let (bibliography, generation) = store.load().unwrap().into_parts();
    store.commit(&generation, &bibliography).unwrap();
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), first);
    assert_eq!(
        bibliography
            .records()
            .iter()
            .map(|record| record.texkey())
            .collect::<Vec<_>>(),
        ["Aad:2012tfa", "Zenodo:2024"]
    );
}

#[test]
fn bibtex_bytes_survive_toml_round_trips() {
    let payload = "@misc{Exact,\r\n title={Schrödinger — \\\"quoted\\\"}   \r\n}";
    let bibliography = Bibliography::restore(vec![(
        RecordId::new(),
        RecordState::new(
            Source::Local,
            Identifiers::default(),
            Description::new("Exact", Vec::new(), Vec::new(), None),
            Bibtex::new(payload, "Exact"),
        )
        .unwrap(),
    )])
    .unwrap();
    let (_directory, store) = store();
    let generation = store.load_or_empty().unwrap().into_parts().1;
    store.commit(&generation, &bibliography).unwrap();
    let reloaded = store.load().unwrap().bibliography;
    assert_eq!(reloaded.records()[0].state().bibtex().source(), payload);
}

#[test]
fn stale_generation_refuses_publication() {
    let (_directory, store) = store();
    store.create_empty().unwrap();
    let (original, stale) = store.load().unwrap().into_parts();
    let (current, generation) = store.load().unwrap().into_parts();
    store.commit(&generation, &current).unwrap();
    std::fs::write(store.path(), "schema = 1\n\n# user edit\n").unwrap();
    assert!(matches!(
        store.commit(&stale, &original),
        Err(Error::StaleBibliography { .. })
    ));
}

#[test]
fn source_shape_and_removed_fields_are_rejected() {
    let (_directory, store) = store();
    for source in [
        "schema = 1\n[[records]]\nid='d760f219-9098-4b49-9f62-10cbbcc22b11'\nsource='local'\nprovider_id='1'\ntitle='T'\nbibtex='@misc{T,title={T}}'\n",
        "schema = 1\n[[records]]\nid='d760f219-9098-4b49-9f62-10cbbcc22b11'\nsource='inspire'\ntitle='T'\nbibtex='@misc{T,title={T}}'\n",
        "schema = 1\n[[records]]\nid='d760f219-9098-4b49-9f62-10cbbcc22b11'\nsource='local'\nkey='old'\ntitle='T'\nbibtex='@misc{T,title={T}}'\n",
    ] {
        std::fs::write(store.path(), source).unwrap();
        assert!(store.load().is_err(), "accepted {source}");
    }
}
