//! The compiled binary: exit status, exact stdout, and files on disk.
//!
//! Provider-backed commands run against a local listener through the test-only
//! base-URL override, so the suite never depends on the real network.

mod support;

use support::{TestServer, hits, record};

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Run `bibi` in a temporary project, with the global manifest redirected so a
/// test can never touch the machine's real one.
fn bibi(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .env("BIBI_GLOBAL_MANIFEST", directory.join("global.toml"))
        .env("BIBI_CACHE_ROOT", directory.join("cache/bibi"))
        .args(args)
        .output()
        .expect("running bibi")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("bibi exited normally")
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().to_path_buf();
    (directory, path)
}

const LIBRARY: &str = "@software{astropy:2022,\n  title = {Astropy},\n  author = {Price-Whelan, Adrian},\n  year = 2022\n}\n\n@unpublished{notes:2026,\n  title = {Lecture notes},\n  author = {Roe, Richard},\n  year = 2026\n}\n";

#[test]
fn init_creates_a_manifest_and_refuses_to_overwrite_one() {
    let (_directory, path) = project();
    let first = bibi(&path, &["init"]);
    assert_eq!(code(&first), 0);
    assert_eq!(stdout(&first), "", "init's result is the file, not stdout");
    assert!(path.join("bibi.toml").exists());
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        "schema = 1\n"
    );

    let second = bibi(&path, &["init"]);
    assert_eq!(code(&second), 1);
    assert!(stderr(&second).contains("already exists"));
}

#[test]
fn a_read_command_without_a_manifest_fails_and_creates_nothing() {
    let (_directory, path) = project();
    let output = bibi(&path, &["list"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no manifest"));
    assert!(!path.join("bibi.toml").exists());
}

#[test]
fn adding_a_file_writes_bibtex_to_stdout_and_a_summary_to_stderr() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    let output = bibi(&path, &["add", "-f", "library.bib"]);

    assert_eq!(code(&output), 0);
    // stdout is the entries themselves, in a form another tool can consume.
    assert_eq!(stdout(&output), LIBRARY.trim_end().to_owned() + "\n");
    assert!(stderr(&output).contains("added 2"));
    assert!(path.join("bibi.toml").exists());
}

#[test]
fn adding_the_same_file_twice_skips_and_still_exits_zero() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let output = bibi(&path, &["add", "-f", "library.bib"]);

    // The requested end state already holds, so a build script may proceed.
    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), LIBRARY.trim_end().to_owned() + "\n");
    assert!(stderr(&output).contains("skipped"));
}

#[test]
fn listing_formats_are_exactly_what_a_pipeline_expects() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let keys = bibi(&path, &["list", "--format", "keys"]);
    assert_eq!(stdout(&keys), "astropy:2022\nnotes:2026\n");

    let bibtex = bibi(&path, &["list", "--format", "bibtex"]);
    assert_eq!(stdout(&bibtex), LIBRARY.trim_end().to_owned() + "\n");

    let json = bibi(&path, &["list", "--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
    assert_eq!(parsed[0]["key"], "astropy:2022");
    assert!(parsed[0].get("bibtex").is_none());

    let filtered = bibi(&path, &["list", "--format", "keys", "--author", "roe"]);
    assert_eq!(stdout(&filtered), "notes:2026\n");
}

#[test]
fn show_emits_one_entry_under_its_local_key() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let output = bibi(&path, &["show", "notes:2026"]);
    assert_eq!(code(&output), 0);
    assert_eq!(
        stdout(&output),
        "@unpublished{notes:2026,\n  title = {Lecture notes},\n  author = {Roe, Richard},\n  year = 2026\n}\n"
    );

    let missing = bibi(&path, &["show", "nothing:here"]);
    assert_eq!(code(&missing), 1);
    assert!(stderr(&missing).contains("no record matches"));
}

#[test]
fn rename_rewrites_only_the_key_and_remove_emits_what_it_deleted() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let renamed = bibi(&path, &["rename", "notes:2026", "Roe:2026"]);
    assert_eq!(code(&renamed), 0);
    assert!(stdout(&renamed).starts_with("@unpublished{Roe:2026,"));
    assert!(stderr(&renamed).contains("\\cite{}"));

    let removed = bibi(&path, &["remove", "Roe:2026"]);
    assert_eq!(code(&removed), 0);
    assert!(stdout(&removed).contains("@unpublished{Roe:2026,"));
    assert_eq!(
        stdout(&bibi(&path, &["list", "--format", "keys"])),
        "astropy:2022\n"
    );
}

#[test]
fn removing_the_same_record_twice_is_a_successful_skip() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let removed = bibi(&path, &["remove", "notes:2026", "notes:2026"]);
    assert_eq!(code(&removed), 0);
    assert_eq!(
        stdout(&removed).matches("@unpublished{notes:2026,").count(),
        1
    );
    assert!(stderr(&removed).contains("already removed"));
    assert_eq!(
        stdout(&bibi(&path, &["list", "--format", "keys"])),
        "astropy:2022\n"
    );
}

#[test]
fn a_dry_run_changes_nothing_on_disk() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let before = std::fs::read_to_string(path.join("bibi.toml")).unwrap();

    let output = bibi(&path, &["remove", "astropy:2022", "--dry-run"]);
    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("@software{astropy:2022,"));
    assert!(stderr(&output).contains("dry run"));
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        before
    );
}

#[test]
fn an_explicit_path_targets_another_project_without_searching_upwards() {
    let (_directory, path) = project();
    let nested = path.join("chapters");
    std::fs::create_dir(&nested).unwrap();
    bibi(&path, &["init"]);
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    // Standing in a subdirectory targets the subdirectory, not the parent.
    let output = bibi(&nested, &["list"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no manifest"));

    // The parent's manifest is reachable by naming it.
    let explicit = bibi(&nested, &["list", "--format", "keys", "-p", "../bibi.toml"]);
    assert_eq!(code(&explicit), 0);
    assert_eq!(stdout(&explicit), "astropy:2022\nnotes:2026\n");
}

#[test]
fn the_global_manifest_is_an_ordinary_project_at_a_fixed_path() {
    let (_directory, path) = project();
    let output = bibi(&path, &["init", "-g"]);
    assert_eq!(code(&output), 0);
    assert!(path.join("global.toml").exists());
    assert!(!path.join("bibi.toml").exists(), "-g selects a target only");
}

/// Run `bibi` with INSPIRE pointed at a local listener.
fn bibi_against(directory: &Path, server: &TestServer, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .env("BIBI_GLOBAL_MANIFEST", directory.join("global.toml"))
        .env("BIBI_CACHE_ROOT", directory.join("cache/bibi"))
        .env("BIBI_INSPIRE_BASE_URL", &server.base_url)
        .args(args)
        .output()
        .expect("running bibi")
}

#[test]
fn adding_a_locator_adopts_the_providers_texkey_and_stores_its_bibtex() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(
            1124337,
            "Aad:2012tfa",
            "1207.7214",
            "Observation of a new particle",
        )]),
        "@article{Aad:2012tfa,\n  title = {Observation of a new particle}\n}\n".to_owned(),
    ]);

    let output = bibi_against(&path, &server, &["add", "1207.7214"]);
    assert_eq!(code(&output), 0);
    assert!(stdout(&output).starts_with("@article{Aad:2012tfa,"));

    // The record is provider-owned, with a handle and a token to refresh by.
    let json = bibi(&path, &["list", "--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(parsed[0]["key"], "Aad:2012tfa");
    assert_eq!(parsed[0]["provider"], "inspire");
    assert_eq!(parsed[0]["provider_id"], "1124337");
    assert_eq!(parsed[0]["revision"], "2026-01-01T00:00:00+00:00");
    assert_eq!(parsed[0]["arxiv"], "1207.7214");
    assert_eq!(parsed[0]["collaborations"][0], "ATLAS");
    // Two requests: one structured search, one BibTeX search.
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_locator_no_provider_holds_is_an_item_failure() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![hits(&[])]);
    let output = bibi_against(&path, &server, &["add", "2401.99999"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no provider holds a record"));
    assert!(!path.join("bibi.toml").exists());
}

#[test]
fn an_imported_entry_a_provider_holds_is_upgraded_but_keeps_its_key() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(
            1124337,
            "Aad:2012tfa",
            "1207.7214",
            "Observation of a new particle",
        )]),
        "@article{Aad:2012tfa,\n  title = {Provider formatting}\n}\n".to_owned(),
    ]);
    std::fs::write(
        path.join("colleague.bib"),
        "@article{TheirKey:2012,\n  title = {Their formatting},\n  eprint = {1207.7214}\n}\n",
    )
    .unwrap();

    let output = bibi_against(&path, &server, &["add", "-f", "colleague.bib"]);
    assert_eq!(code(&output), 0);
    // The colleague's citation key survives; the provider's bytes replace theirs.
    assert!(stdout(&output).contains("@article{TheirKey:2012,"));
    assert!(stdout(&output).contains("Provider formatting"));
    let manifest = std::fs::read_to_string(path.join("bibi.toml")).unwrap();
    assert!(manifest.contains("provider = \"inspire\""));
    assert!(manifest.contains("@article{Aad:2012tfa,"));
}

#[test]
fn structural_flag_conflicts_are_clap_usage_errors() {
    let (_directory, path) = project();
    for args in [
        vec!["list", "-p", "a.toml", "-g"],
        vec!["add", "-f", "x.bib", "--key", "K"],
        vec!["add", "--force-local", "1207.7214"],
        vec![
            "add",
            "-f",
            "x.bib",
            "--force-local",
            "--provider",
            "inspire",
        ],
    ] {
        let output = bibi(&path, &args);
        assert_eq!(code(&output), 2, "{args:?} should be a usage error");
    }
}

#[test]
fn export_writes_a_bibliography_and_check_verifies_it() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    // A bibliography is written only when asked for: adding wrote no .bib.
    assert!(!path.join("references.bib").exists());

    let exported = bibi(&path, &["export"]);
    assert_eq!(code(&exported), 0);
    assert_eq!(stdout(&exported), "", "the result is the file");
    assert!(stderr(&exported).contains("wrote 2 record(s)"));
    assert_eq!(
        std::fs::read_to_string(path.join("references.bib")).unwrap(),
        LIBRARY.trim_end().to_owned() + "\n"
    );

    let matched = bibi(&path, &["check"]);
    assert_eq!(code(&matched), 0);
    assert!(stderr(&matched).contains("matches the manifest"));

    // Drift is a nonzero exit, and check repairs nothing.
    std::fs::write(
        path.join("references.bib"),
        "@misc{edited,title={By hand}}\n",
    )
    .unwrap();
    let drifted = bibi(&path, &["check", "--diff"]);
    assert_eq!(code(&drifted), 1);
    assert!(stderr(&drifted).contains("drifted"));
    assert!(stderr(&drifted).contains("first difference at line 1"));
    assert_eq!(
        std::fs::read_to_string(path.join("references.bib")).unwrap(),
        "@misc{edited,title={By hand}}\n"
    );

    // A bibliography that is not there is missing, not drift.
    std::fs::remove_file(path.join("references.bib")).unwrap();
    let missing = bibi(&path, &["check"]);
    assert_eq!(code(&missing), 1);
    assert!(stderr(&missing).contains("no bibliography at"));
}

#[test]
fn export_refuses_to_write_over_the_manifest() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let before = std::fs::read_to_string(path.join("bibi.toml")).unwrap();

    let output = bibi(&path, &["export", "-o", "bibi.toml"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("choose another output path"));
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        before
    );
}

#[test]
fn export_filters_select_what_is_rendered() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let output = bibi(&path, &["export", "-o", "subset.bib", "--year", "2026"]);
    assert_eq!(code(&output), 0);
    let written = std::fs::read_to_string(path.join("subset.bib")).unwrap();
    assert!(written.contains("notes:2026"));
    assert!(!written.contains("astropy:2022"));

    // The same options make the check agree; without them it drifts.
    assert_eq!(
        code(&bibi(&path, &["check", "subset.bib", "--year", "2026"])),
        0
    );
    assert_eq!(code(&bibi(&path, &["check", "subset.bib"])), 1);
}

#[test]
fn export_and_check_honor_local() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);
    std::fs::write(path.join("mine.bib"), "@misc{Mine,title={Mine}}\n").unwrap();
    bibi(&path, &["add", "-f", "mine.bib", "--force-local"]);

    let listed = bibi(&path, &["list", "--format", "keys", "--local"]);
    assert_eq!(code(&listed), 0);
    assert_eq!(stdout(&listed), "Mine\n");

    let exported = bibi(&path, &["export", "-o", "local.bib", "--local"]);
    assert_eq!(code(&exported), 0);
    assert!(stderr(&exported).contains("wrote 1 record(s)"));
    let written = std::fs::read_to_string(path.join("local.bib")).unwrap();
    assert!(written.contains("@misc{Mine"));
    assert!(!written.contains("Aad:2012tfa"));

    assert_eq!(code(&bibi(&path, &["check", "local.bib", "--local"])), 0);
    assert_eq!(code(&bibi(&path, &["check", "local.bib"])), 1);
}

#[test]
fn a_sync_with_no_managed_records_reports_and_writes_nothing() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let before = std::fs::read_to_string(path.join("bibi.toml")).unwrap();

    // Both records are local, so nothing is refreshable and no request is made.
    let output = bibi(&path, &["sync"]);
    assert_eq!(code(&output), 0);
    assert!(stderr(&output).contains("2 unrefreshable"));
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        before
    );
}

#[test]
fn fetch_reports_a_url_without_downloading_anything() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);

    let output = bibi(&path, &["fetch", "Aad:2012tfa", "--url"]);
    assert_eq!(code(&output), 0);
    // One line, so `open $(bibi fetch <selector> --url)` works.
    assert_eq!(stdout(&output), "https://arxiv.org/pdf/1207.7214\n");
    // Nothing was cached: --url is a question, not a download.
    assert!(!path.join("cache").exists());
}

#[test]
fn fetch_url_answers_even_when_the_cache_root_is_unusable() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);

    // The filesystem root is not a cache root bibi may own, but --url is a
    // question about arXiv's public address, not about the cache.
    let output = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(&path)
        .env("BIBI_GLOBAL_MANIFEST", path.join("global.toml"))
        .env("BIBI_CACHE_ROOT", "/")
        .args(["fetch", "Aad:2012tfa", "--url"])
        .output()
        .expect("running bibi");
    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "https://arxiv.org/pdf/1207.7214\n");
}

#[test]
fn fetching_a_record_without_an_arxiv_id_explains_why_it_cannot() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let output = bibi(&path, &["fetch", "notes:2026", "--url"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no arXiv identifier"));
}

#[test]
fn cache_clean_previews_before_it_removes_and_needs_to_be_told_which() {
    let (_directory, path) = project();
    let cache = path.join("cache/bibi/documents/arxiv/1207.7214");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("paper.pdf"), "%PDF-1.7\n").unwrap();

    let preview = bibi(&path, &["cache", "clean", "--dry-run"]);
    assert_eq!(code(&preview), 0);
    assert!(stderr(&preview).contains("would remove 1 file"));
    assert!(cache.join("paper.pdf").exists());

    let removed = bibi(&path, &["cache", "clean", "--all"]);
    assert_eq!(code(&removed), 0);
    assert!(stderr(&removed).contains("removed 1 file"));
    assert!(!path.join("cache/bibi/documents").exists());
    // The cache root itself is shared with the platform, so it survives.
    assert!(path.join("cache/bibi").exists());

    // Neither flag is a usage error: eviction is never implicit.
    assert_eq!(code(&bibi(&path, &["cache", "clean"])), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn a_non_broken_stdout_error_exits_nonzero() {
    use std::{fs::OpenOptions, process::Stdio};

    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let sink = OpenOptions::new().write(true).open("/dev/full").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(&path)
        .env("BIBI_GLOBAL_MANIFEST", path.join("global.toml"))
        .env("BIBI_CACHE_ROOT", path.join("cache/bibi"))
        .args(["show", "notes:2026"])
        .stdout(Stdio::from(sink))
        .output()
        .expect("running bibi");

    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("writing stdout"));
}

#[cfg(unix)]
#[test]
fn a_closed_stdout_pipe_still_exits_zero() {
    use std::process::Stdio;

    let (_directory, path) = project();
    let large = format!("@misc{{Large,title={{{}}}}}\n", "x".repeat(1_000_000));
    std::fs::write(path.join("large.bib"), large).unwrap();
    bibi(&path, &["add", "-f", "large.bib", "--force-local"]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(&path)
        .env("BIBI_GLOBAL_MANIFEST", path.join("global.toml"))
        .env("BIBI_CACHE_ROOT", path.join("cache/bibi"))
        .args(["show", "Large"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running bibi");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("waiting for bibi");

    assert_eq!(code(&output), 0);
}
