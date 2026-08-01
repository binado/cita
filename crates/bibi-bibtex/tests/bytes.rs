//! Byte-preservation, re-keying, and rendering fixtures.
//!
//! These assert the invariant the whole crate exists to hold: what bibi stores
//! and re-emits is byte-identical to what it was given, except for the citation
//! key token. The fixtures are files rather than Rust literals because the
//! interesting cases — trailing whitespace, CRLF, an inline comment before a
//! closing brace — are exactly the ones that a string escape quietly loses.

use bibi_bibtex::{BibtexEntry, CitationKey, parse_file, render};

const CORPUS: &str = include_str!("fixtures/corpus.bib");
const CRLF: &str = include_str!("fixtures/crlf.bib");
const RENDERED: &str = include_str!("golden/corpus.rendered.bib");

fn key(value: &str) -> CitationKey {
    CitationKey::new(value).unwrap()
}

#[test]
fn every_entry_span_is_a_verbatim_slice_of_the_file() {
    let entries = parse_file(CORPUS).unwrap();
    assert_eq!(entries.len(), 5, "fixture corpus size");
    // Walking the file entry by entry must account for every byte: each span
    // occurs verbatim and in order, and only whitespace lies between them.
    let mut cursor = 0;
    for entry in &entries {
        let source = entry.payload.source();
        assert_eq!(source.trim(), source, "span carries outer whitespace");
        assert!(source.starts_with('@'));
        assert!(source.ends_with('}') || source.ends_with(')'));
        let at = cursor
            + CORPUS[cursor..]
                .find(source)
                .unwrap_or_else(|| panic!("entry `{}` is not a verbatim slice", entry.key));
        assert!(
            CORPUS[cursor..at].trim().is_empty(),
            "non-whitespace precedes entry `{}`",
            entry.key
        );
        cursor = at + source.len();
    }
    assert!(CORPUS[cursor..].trim().is_empty(), "trailing content");
}

#[test]
fn parsing_one_entry_out_of_the_corpus_preserves_its_bytes() {
    for entry in parse_file(CORPUS).unwrap() {
        let source = entry.payload.source().to_owned();
        let reparsed = BibtexEntry::parse_one(source.clone()).unwrap();
        assert_eq!(reparsed.source(), source);
        assert_eq!(reparsed.source_key(), &entry.key);
    }
}

#[test]
fn crlf_line_endings_survive_parsing_and_rendering() {
    assert!(CRLF.contains("\r\n"), "fixture lost its CRLF endings");
    let entries = parse_file(CRLF).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0].payload;
    assert_eq!(
        entry.source(),
        "@misc{Windows:2025,\r\n  title = {An entry with CRLF line endings},\r\n  year = 2025\r\n}"
    );
    // Only the separator and terminator the renderer adds are LF.
    assert_eq!(
        render([(&key("Windows:2025"), entry)]).unwrap(),
        format!("{}\n", entry.source())
    );
}

#[test]
fn rekeying_changes_only_the_key_token() {
    let replacements = [
        "A",
        "Aad:2012tfa",
        "a-very-much-longer-replacement-key",
        "x.y_z:1+2-3",
    ];
    for entry in parse_file(CORPUS).unwrap() {
        let payload = &entry.payload;
        let original = payload.source();
        let old = entry.key.as_str();
        for replacement in replacements {
            let rekeyed = payload.rekey(&key(replacement)).unwrap();
            assert_eq!(
                rekeyed.len(),
                original.len() - old.len() + replacement.len(),
                "`{replacement}` changed the length by more than the key"
            );
            // Deleting each key from its own rendering leaves identical bytes.
            let reparsed = BibtexEntry::parse_one(rekeyed.clone()).unwrap();
            assert_eq!(reparsed.source_key().as_str(), replacement);
            assert_eq!(
                without_key(original, old),
                without_key(&rekeyed, replacement),
                "`{replacement}` disturbed bytes outside the key"
            );
            // Re-keying back is the identity, so the operation loses nothing.
            assert_eq!(reparsed.rekey(&entry.key).unwrap(), original);
        }
    }
}

/// Remove the first occurrence of an entry's key token from its source.
fn without_key(source: &str, key: &str) -> String {
    let at = source.find(key).expect("an entry contains its own key");
    format!("{}{}", &source[..at], &source[at + key.len()..])
}

#[test]
fn rendering_the_corpus_matches_the_golden_file() {
    let entries = parse_file(CORPUS).unwrap();
    let keys = entries
        .iter()
        .enumerate()
        .map(|(index, _)| key(&format!("Local:{index}")))
        .collect::<Vec<_>>();
    let rendered = render(
        keys.iter()
            .zip(entries.iter().map(|entry| &entry.payload))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(rendered, RENDERED);
    // The golden file is itself a valid input, and rendering it is a fixed point.
    let round_trip = parse_file(RENDERED).unwrap();
    let rendered_again = render(
        round_trip
            .iter()
            .map(|entry| (&entry.key, &entry.payload))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(rendered_again, RENDERED);
}

#[test]
fn rendering_ignores_everything_but_key_and_payload() {
    // An entry whose description would differ wildly renders identically as
    // long as its bytes and key are the same, which is invariant I7.
    let source = "@misc{k, title = {Whatever}, author = {Nobody}, year = 1900}";
    let entry = BibtexEntry::parse_one(source.to_owned()).unwrap();
    assert!(entry.local_metadata().is_ok());
    assert_eq!(
        render([(&key("Stable"), &entry)]).unwrap(),
        "@misc{Stable, title = {Whatever}, author = {Nobody}, year = 1900}\n"
    );
}
