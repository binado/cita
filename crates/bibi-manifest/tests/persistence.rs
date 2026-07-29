//! Manifest determinism, payload round trips, and the golden schema-1 file.

use bibi_bibtex::{BibtexEntry, CitationKey};
use bibi_core::{
    ArxivId, BibiId, Description, Doi, Identifiers, Provenance, ProviderId, ProviderName,
    ProviderOwned, Record, Revision,
};
use bibi_manifest::{ManifestCandidate, ManifestStore};

const POPULATED: &str = include_str!("golden/populated.toml");

fn managed(id: &str, key: &str, provider_id: &str, payload: &str, title: &str) -> Record {
    Record::new(
        id.parse::<BibiId>().unwrap(),
        CitationKey::new(key).unwrap(),
        ProviderOwned {
            provenance: Provenance::managed(
                ProviderName::new("inspire").unwrap(),
                ProviderId::new(provider_id).unwrap(),
                Some(Revision::new("2026-07-27T12:34:56+00:00").unwrap()),
            ),
            identifiers: Identifiers {
                doi: Some(Doi::new("10.1016/j.physletb.2012.08.020").unwrap()),
                arxiv: Some(ArxivId::new("1207.7214").unwrap()),
            },
            payload: BibtexEntry::parse_one(payload.to_owned()).unwrap(),
            description: Description {
                title: title.to_owned(),
                authors: vec!["Aad, G.".into(), "Abajyan, T.".into()],
                collaborations: vec!["ATLAS".into()],
                year: Some(2012),
            },
        },
    )
    .unwrap()
}

fn local(id: &str, key: &str, payload: &str, title: &str) -> Record {
    Record::new(
        id.parse::<BibiId>().unwrap(),
        CitationKey::new(key).unwrap(),
        ProviderOwned {
            provenance: Provenance::unmanaged(ProviderName::new("local").unwrap()),
            identifiers: Identifiers::default(),
            payload: BibtexEntry::parse_one(payload.to_owned()).unwrap(),
            description: Description {
                title: title.to_owned(),
                ..Description::default()
            },
        },
    )
    .unwrap()
}

fn populated() -> ManifestCandidate {
    let mut candidate = ManifestCandidate::empty();
    // Inserted out of order: the file is sorted by local key regardless.
    candidate
        .insert(local(
            "8f14e45f-ceea-467a-9f38-a2f2b1a1f1c9",
            "Zenodo:2024",
            "@software{upstream-key,\n  title = {A tool},\n  author = {Roe, Richard},\n  year = 2024\n}",
            "A tool",
        ))
        .unwrap();
    candidate
        .insert(managed(
            "d760f219-9098-4b49-9f62-10cbbcc22b11",
            "Aad:2012tfa",
            "1124337",
            "@article{Aad:2012tfa,\n  author = {Aad, Georges and others},\n  collaboration = {ATLAS},\n  title = {{Observation of a new particle}},\n  eprint = {1207.7214},\n  doi = {10.1016/j.physletb.2012.08.020},\n  year = {2012}\n}",
            "Observation of a new particle",
        ))
        .unwrap();
    candidate
}

#[test]
fn a_populated_manifest_matches_the_golden_file() {
    let rendered = populated().validate().unwrap().to_toml().unwrap();
    assert_eq!(rendered, POPULATED);
}

#[test]
fn serializing_parsing_and_serializing_again_is_byte_identical() {
    let directory = tempfile::tempdir().unwrap();
    let store = ManifestStore::new(directory.path().join("bibi.toml"));
    let (_, generation) = store.load_or_empty().unwrap().into_candidate();
    store.commit(&generation, populated()).unwrap();
    let first = std::fs::read_to_string(store.path()).unwrap();

    let (candidate, generation) = store.load().unwrap().into_candidate();
    store.commit(&generation, candidate).unwrap();
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), first);
    assert_eq!(first, POPULATED);
}

#[test]
fn payload_bytes_survive_the_toml_round_trip() {
    // The serializer is free to escape or requote a payload; what it may not do
    // is hand back different bytes. These are the shapes that break naive
    // quoting: newlines of both kinds, quotes, backslashes, and non-ASCII.
    let payloads = [
        "@misc{a,title={plain}}",
        "@misc{a,\n  title = {two lines}\n}",
        "@misc{a,\r\n  title = {carriage returns}\r\n}",
        "@misc{a,title={a \"quoted\" word}}",
        "@misc{a,title={a back\\slash and a tab\there}}",
        "@misc{a,title={triple \"\"\" quotes}}",
        "@misc{a,title={Schrödinger — “curly” ünïcode}}",
        "@misc{a,title={trailing spaces   }   }",
    ];
    let directory = tempfile::tempdir().unwrap();
    let store = ManifestStore::new(directory.path().join("bibi.toml"));
    for (index, payload) in payloads.iter().enumerate() {
        let mut candidate = ManifestCandidate::empty();
        candidate
            .insert(local(
                "8f14e45f-ceea-467a-9f38-a2f2b1a1f1c9",
                &format!("Key{index}"),
                payload,
                "T",
            ))
            .unwrap();
        let generation = store.load_or_empty().unwrap().into_candidate().1;
        store.commit(&generation, candidate).unwrap();
        let reloaded = store.load().unwrap().manifest;
        assert_eq!(
            reloaded.records()[0].payload.source(),
            *payload,
            "payload {index} did not survive"
        );
        std::fs::remove_file(store.path()).unwrap();
    }
}

#[test]
fn the_golden_manifest_reloads_into_the_same_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bibi.toml");
    std::fs::write(&path, POPULATED).unwrap();
    let manifest = ManifestStore::new(&path).load().unwrap().manifest;
    let keys = manifest
        .records()
        .iter()
        .map(|record| record.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(keys, ["Aad:2012tfa", "Zenodo:2024"]);
    let managed = &manifest.records()[0];
    assert_eq!(managed.provenance.provider.as_str(), "inspire");
    assert_eq!(
        managed.provenance.provider_id.as_ref().unwrap().as_str(),
        "1124337"
    );
    assert_eq!(managed.description.collaborations, ["ATLAS"]);
    // A local record carries neither a provider id nor a revision.
    let local = &manifest.records()[1];
    assert!(local.provenance.provider_id.is_none());
    assert!(local.provenance.revision.is_none());
    assert!(local.identifiers.doi.is_none());
}

#[test]
fn a_records_local_key_is_written_into_its_rendered_payload_only() {
    let manifest = populated().validate().unwrap();
    let record = &manifest.records()[1];
    // Stored bytes keep the upstream key; only the rendering adopts the local one.
    assert!(record.payload.source().contains("@software{upstream-key,"));
    assert!(
        record
            .rendered()
            .unwrap()
            .contains("@software{Zenodo:2024,")
    );
}
