use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn paperdb(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paperdb"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn git(cwd: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sample_manifest() -> &'static str {
    r#"schema = 1

[[papers]]
key = "Zed:2020"
title = "A  result with $Z\\to ee$"
authors = ["Zed, Zoe"]
year = 2020
arxiv_ids = ["2001.00001"]
source = "inspire"

[[papers]]
key = "Alpha:2019"
title = "Earlier paper"
collaborations = ["Example Collaboration"]
year = 2019
dois = ["10.1000/example"]
source = "inspire"

[papers.publication]
journal = "JHEP"
volume = "1"
pages = "12"
year = 2019
"#
}

#[test]
fn init_uses_git_root_and_refuses_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    assert_success(&git(directory.path(), &["init", "-q"]));
    let nested = directory.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    assert_success(&paperdb(&nested, &["init"]));
    assert!(directory.path().join("paperdb.toml").is_file());
    let duplicate = paperdb(&nested, &["init"]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already exists"));
}

#[test]
fn discovers_parent_manifest_lists_and_exports_clean_bibtex() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("paperdb.toml"), sample_manifest()).unwrap();
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();

    let list = paperdb(&nested, &["list"]);
    assert_success(&list);
    let list = String::from_utf8(list.stdout).unwrap();
    assert!(list.find("Alpha:2019").unwrap() < list.find("Zed:2020").unwrap());

    let export = paperdb(&nested, &["export", "--bibtex"]);
    assert_success(&export);
    assert!(export.stderr.is_empty());
    let bib = String::from_utf8(export.stdout).unwrap();
    assert!(bib.starts_with("@article{Alpha:2019,"));
    assert!(bib.contains("@misc{Zed:2020,"));
    assert!(bib.contains("$Z\\to ee$"));
}

#[test]
fn multi_remove_is_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("paperdb.toml");
    fs::write(&path, sample_manifest()).unwrap();
    let before = fs::read_to_string(&path).unwrap();
    let failed = paperdb(directory.path(), &["remove", "Alpha:2019", "missing"]);
    assert!(!failed.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_success(&paperdb(
        directory.path(),
        &["remove", "doi:10.1000/example"],
    ));
    assert!(!fs::read_to_string(path).unwrap().contains("Alpha:2019"));
}

#[test]
fn commit_is_scoped_and_leaves_unrelated_staging_intact() {
    let directory = tempfile::tempdir().unwrap();
    assert_success(&git(directory.path(), &["init", "-q"]));
    assert_success(&git(
        directory.path(),
        &["config", "user.name", "PaperDB Tests"],
    ));
    assert_success(&git(
        directory.path(),
        &["config", "user.email", "paperdb@example.invalid"],
    ));
    assert_success(&paperdb(directory.path(), &["init"]));
    assert_success(&paperdb(directory.path(), &["commit"]));
    let subject = git(directory.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "references: initialize paperdb"
    );

    fs::write(directory.path().join("unrelated.txt"), "keep staged\n").unwrap();
    assert_success(&git(directory.path(), &["add", "unrelated.txt"]));
    fs::write(directory.path().join("paperdb.toml"), sample_manifest()).unwrap();
    assert_success(&paperdb(directory.path(), &["commit"]));
    let subject = git(directory.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "references: add 2 papers"
    );

    let committed = git(
        directory.path(),
        &["show", "--pretty=", "--name-only", "HEAD"],
    );
    assert_eq!(
        String::from_utf8_lossy(&committed.stdout).trim(),
        "paperdb.toml"
    );
    let staged = git(directory.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(
        String::from_utf8_lossy(&staged.stdout).trim(),
        "unrelated.txt"
    );
    let unchanged = paperdb(directory.path(), &["commit"]);
    assert_success(&unchanged);
    assert!(String::from_utf8_lossy(&unchanged.stdout).contains("no Git changes"));
}

#[test]
fn no_subcommand_prints_help() {
    let directory = tempfile::tempdir().unwrap();
    let output = paperdb(directory.path(), &[]);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: paperdb"));
}
