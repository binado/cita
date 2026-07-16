use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cita(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
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

[papers."Zed:2020"]
title = "A  result with $Z\\to ee$"
authors = ["Zed, Zoe"]
year = 2020
source = "inspire"
arxiv_ids = ["2001.00001"]

[papers."Alpha:2019"]
title = "Earlier paper"
collaborations = ["Example Collaboration"]
year = 2019
source = "inspire"
dois = ["10.1000/example"]

[papers."Alpha:2019".publication]
journal = "JHEP"
volume = "1"
pages = "12"
year = 2019
"#
}

#[test]
fn init_uses_git_root_and_idempotently_creates_cache_layout() {
    let directory = tempfile::tempdir().unwrap();
    assert_success(&git(directory.path(), &["init", "-q"]));
    fs::write(directory.path().join(".gitignore"), ".DS_Store").unwrap();
    let nested = directory.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    assert_success(&cita(&nested, &["init"]));
    assert!(directory.path().join("cita.toml").is_file());
    assert!(directory.path().join(".cita/files").is_dir());
    let expected_ignore = ".DS_Store\n\n# Cita document cache\n/.cita/files/\n";
    assert_eq!(
        fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        expected_ignore
    );

    let duplicate = cita(&nested, &["init"]);
    assert_success(&duplicate);
    assert!(String::from_utf8_lossy(&duplicate.stdout).contains("Already initialized"));
    assert_eq!(
        fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        expected_ignore
    );
}

#[test]
fn init_refuses_an_invalid_existing_manifest_without_creating_layout() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), "not valid toml = [").unwrap();

    let initialized = cita(directory.path(), &["init"]);
    assert!(!initialized.status.success());
    assert!(!directory.path().join(".cita").exists());
    assert!(!directory.path().join(".gitignore").exists());
}

#[test]
fn discovers_parent_manifest_lists_and_exports_clean_bibtex() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), sample_manifest()).unwrap();
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();

    let list = cita(&nested, &["list"]);
    assert_success(&list);
    let list = String::from_utf8(list.stdout).unwrap();
    assert!(list.find("Alpha:2019").unwrap() < list.find("Zed:2020").unwrap());

    let export = cita(&nested, &["export", "--bibtex"]);
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
    let path = directory.path().join("cita.toml");
    fs::write(&path, sample_manifest()).unwrap();
    let before = fs::read_to_string(&path).unwrap();
    let failed = cita(directory.path(), &["remove", "Alpha:2019", "missing"]);
    assert!(!failed.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_success(&cita(directory.path(), &["remove", "doi:10.1000/example"]));
    assert!(!fs::read_to_string(path).unwrap().contains("Alpha:2019"));
}

#[test]
fn fetch_reuses_cached_pdf_and_reports_missing_arxiv_id() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), sample_manifest()).unwrap();
    let pdf = directory.path().join(".cita/files/arxiv/2001.00001.pdf");
    fs::create_dir_all(pdf.parent().unwrap()).unwrap();
    fs::write(&pdf, b"%PDF-cached").unwrap();
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();

    let fetched = cita(&nested, &["fetch", "Zed:2020"]);
    assert_success(&fetched);
    let stdout = String::from_utf8_lossy(&fetched.stdout);
    assert!(stdout.contains("Already fetched Zed:2020"), "{stdout}");
    assert!(stdout.contains(pdf.to_string_lossy().as_ref()), "{stdout}");
    assert_eq!(
        fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        "# Cita document cache\n/.cita/files/\n"
    );

    let missing = cita(&nested, &["fetch", "--force", "Alpha:2019"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("paper has no arXiv identifier"));
}

#[test]
fn open_no_download_errors_on_a_cache_miss_without_creating_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), sample_manifest()).unwrap();

    let opened = cita(directory.path(), &["open", "--no-download", "Zed:2020"]);

    assert!(!opened.status.success());
    assert!(
        String::from_utf8_lossy(&opened.stderr).contains("PDF is not cached"),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    assert!(!directory.path().join(".cita").exists());
    assert!(!directory.path().join(".gitignore").exists());
}

#[test]
fn commit_is_scoped_and_leaves_unrelated_staging_intact() {
    let directory = tempfile::tempdir().unwrap();
    assert_success(&git(directory.path(), &["init", "-q"]));
    assert_success(&git(
        directory.path(),
        &["config", "user.name", "Cita Tests"],
    ));
    assert_success(&git(
        directory.path(),
        &["config", "user.email", "cita@example.invalid"],
    ));
    assert_success(&cita(directory.path(), &["init"]));
    assert_success(&cita(directory.path(), &["commit"]));
    let subject = git(directory.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "references: initialize cita"
    );

    fs::write(directory.path().join("unrelated.txt"), "keep staged\n").unwrap();
    assert_success(&git(directory.path(), &["add", "unrelated.txt"]));
    fs::write(directory.path().join("cita.toml"), sample_manifest()).unwrap();
    assert_success(&cita(directory.path(), &["commit"]));
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
        "cita.toml"
    );
    let staged = git(directory.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(
        String::from_utf8_lossy(&staged.stdout).trim(),
        "unrelated.txt"
    );
    let unchanged = cita(directory.path(), &["commit"]);
    assert_success(&unchanged);
    assert!(String::from_utf8_lossy(&unchanged.stdout).contains("no Git changes"));
}

#[test]
fn commit_falls_back_gracefully_when_head_manifest_is_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    assert_success(&git(directory.path(), &["init", "-q"]));
    assert_success(&git(
        directory.path(),
        &["config", "user.name", "Cita Tests"],
    ));
    assert_success(&git(
        directory.path(),
        &["config", "user.email", "cita@example.invalid"],
    ));
    let old_format = "schema = 1\n\n[[papers]]\nkey = \"Old:2019\"\ntitle = \"Old format\"\nsource = \"inspire\"\n";
    fs::write(directory.path().join("cita.toml"), old_format).unwrap();
    assert_success(&git(directory.path(), &["add", "cita.toml"]));
    assert_success(&git(
        directory.path(),
        &["commit", "-q", "-m", "old-format manifest"],
    ));

    fs::write(directory.path().join("cita.toml"), sample_manifest()).unwrap();
    assert_success(&cita(directory.path(), &["commit"]));
    let subject = git(directory.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "references: update bibliography"
    );
}

#[test]
fn no_subcommand_prints_help() {
    let directory = tempfile::tempdir().unwrap();
    let output = cita(directory.path(), &[]);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: cita"));
}
