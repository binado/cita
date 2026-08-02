//! Exact byte preservation and deterministic rendering fixtures.

use bibi_bibtex::{parse_file, render};

const CORPUS: &str = include_str!("fixtures/corpus.bib");
const CRLF: &str = include_str!("fixtures/crlf.bib");

#[test]
fn every_entry_span_is_a_verbatim_slice_of_the_file() {
    let entries = parse_file(CORPUS).unwrap();
    assert_eq!(entries.len(), 5, "fixture corpus size");
    let mut cursor = 0;
    for entry in &entries {
        let source = entry.source();
        assert_eq!(source.trim(), source, "span carries outer whitespace");
        let at = cursor
            + CORPUS[cursor..]
                .find(source)
                .unwrap_or_else(|| panic!("entry `{}` is not a verbatim slice", entry.texkey()));
        assert!(CORPUS[cursor..at].trim().is_empty());
        cursor = at + source.len();
    }
    assert!(CORPUS[cursor..].trim().is_empty());
}

#[test]
fn parsing_one_entry_out_of_the_corpus_preserves_its_bytes_and_texkey() {
    for entry in parse_file(CORPUS).unwrap() {
        let reparsed = parse_file(entry.source()).unwrap().remove(0);
        assert_eq!(reparsed, entry);
    }
}

#[test]
fn crlf_line_endings_survive_parsing_and_rendering() {
    assert!(CRLF.contains("\r\n"), "fixture lost its CRLF endings");
    let entries = parse_file(CRLF).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        render(entries.iter().map(|entry| entry.source())),
        format!("{}\n", entries[0].source())
    );
}

#[test]
fn rendering_preserves_every_entry_and_is_a_fixed_point() {
    let entries = parse_file(CORPUS).unwrap();
    let rendered = render(entries.iter().map(|entry| entry.source()));
    let reparsed = parse_file(&rendered).unwrap();
    assert_eq!(reparsed, entries);
    assert_eq!(
        render(reparsed.iter().map(|entry| entry.source())),
        rendered
    );
}

#[test]
fn biblatex_valid_keys_need_not_fit_a_rewrite_safe_subset() {
    let entry = parse_file("@misc{a/b,title={T}}").unwrap().remove(0);
    assert_eq!(entry.texkey(), "a/b");
    assert_eq!(render([entry.source()]), "@misc{a/b,title={T}}\n");
}
