//! The compiled binary: exit status, exact stdout, and files on disk.

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

#[test]
fn a_locator_no_installed_provider_holds_is_an_item_failure() {
    let (_directory, path) = project();
    // This build carries only the local provider, which resolves nothing.
    let output = bibi(&path, &["add", "1207.7214"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no provider holds a record"));
    assert!(!path.join("bibi.toml").exists());
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
