use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    thread,
};

fn cita(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn cita_with_server(cwd: &Path, args: &[&str], base: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env("CITA_INSPIRE_BASE_URL", base)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn failure(output: Output) -> String {
    assert!(!output.status.success(), "command unexpectedly succeeded");
    String::from_utf8(output.stderr).unwrap()
}

fn entry(key: &str, title: &str, author: &str, year: Option<i32>, extra: &str) -> String {
    let year = year
        .map(|value| format!("  year = {{{value}}},\n"))
        .unwrap_or_default();
    format!(
        "@article{{{key},\n  title = {{{title}}},\n  author = {{{author}}},\n{year}  {extra}\n}}"
    )
}

fn write_bibliography(directory: &Path, entries: &[String]) {
    fs::write(
        directory.join("references.bib"),
        format!("{}\n", entries.join("\n\n")),
    )
    .unwrap();
}

fn server(responses: Vec<(&'static str, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|(status, body)| {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0; 32768];
                let length = stream.read(&mut bytes).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                String::from_utf8_lossy(&bytes[..length])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    });
    (format!("http://{address}/"), handle)
}

fn git(directory: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap()
}

fn git_ok(directory: &Path, args: &[&str]) -> String {
    success(git(directory, args))
}

fn init_git(directory: &Path) {
    git_ok(directory, &["init", "-q"]);
    git_ok(directory, &["config", "user.email", "cita@example.test"]);
    git_ok(directory, &["config", "user.name", "Cita Test"]);
}

#[test]
fn init_uses_git_root_and_discovers_parent_bibliography() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    let child = directory.path().join("nested/work");
    fs::create_dir_all(&child).unwrap();
    let output = success(cita(&child, &["init"]));
    assert!(output.contains("Initialized"));
    assert!(directory.path().join("references.bib").is_file());
    assert!(directory.path().join(".cita/files").is_dir());
    assert!(
        fs::read_to_string(directory.path().join(".gitignore"))
            .unwrap()
            .contains("/.cita/files/")
    );
    assert!(success(cita(&child, &["list"])).is_empty());
    assert!(success(cita(&child, &["init"])).contains("Already initialized"));
}

#[test]
fn legacy_toml_is_a_clean_break() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), "schema = 1\n").unwrap();
    let error = failure(cita(directory.path(), &["init"]));
    assert!(error.contains("legacy cita.toml"));
    assert!(error.contains("no automatic migration"));
    assert!(!directory.path().join("references.bib").exists());
    assert!(!directory.path().join(".cita").exists());
}

#[test]
fn list_projects_fields_and_export_command_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    write_bibliography(
        directory.path(),
        &[
            entry("Zed", "Zeta", "Roe, Richard", None, "eprint={2401.00002},"),
            entry(
                "Alpha",
                "Alpha",
                "Doe, Jane and Roe, Richard",
                Some(2023),
                "eprint={2401.00001},",
            ),
        ],
    );
    let output = success(cita(
        directory.path(),
        &["list", "--sort-by", "year", "--order", "desc"],
    ));
    assert!(output.contains("Jane Doe et al."));
    assert!(output.find("Alpha").unwrap() < output.find("Zed").unwrap());
    assert!(
        failure(cita(directory.path(), &["export", "--bibtex"]))
            .contains("unrecognized subcommand")
    );
}

#[test]
fn multi_add_is_atomic_and_existing_keys_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("references.bib"), "").unwrap();
    let valid = entry(
        "A",
        "Alpha",
        "Doe, Jane",
        Some(2024),
        "eprint={2401.00001},",
    );
    let (base, handle) = server(vec![
        ("200 OK", valid.clone()),
        ("200 OK", "not bibtex @".into()),
    ]);
    let error = failure(cita_with_server(
        directory.path(),
        &["add", "2401.00001", "2401.00002"],
        &base,
    ));
    assert!(error.contains("malformed BibTeX"));
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        ""
    );
    assert_eq!(handle.join().unwrap().len(), 2);

    let (base, handle) = server(vec![("200 OK", valid.clone()), ("200 OK", valid)]);
    let output = success(cita_with_server(
        directory.path(),
        &["add", "2401.00001", "doi:10.1/a"],
        &base,
    ));
    assert_eq!(output, "Added A\nAlready present: A\n");
    assert_eq!(handle.join().unwrap().len(), 2);
}

#[test]
fn multi_remove_is_atomic_and_matches_normalized_identifiers() {
    let directory = tempfile::tempdir().unwrap();
    let original = entry(
        "A",
        "Alpha",
        "Doe, Jane",
        Some(2024),
        "doi={10.1/ABC},\n  eprint={2401.00001v2},",
    );
    write_bibliography(directory.path(), std::slice::from_ref(&original));
    let before = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    let error = failure(cita(
        directory.path(),
        &["remove", "doi:10.1/abc", "not-a-locator"],
    ));
    assert!(error.contains("not-a-locator"));
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        before
    );
    assert_eq!(
        success(cita(directory.path(), &["remove", "2401.00001"])),
        "Removed A\n"
    );
}

#[test]
fn sync_refreshes_whole_file_and_skips_an_identical_second_write() {
    let directory = tempfile::tempdir().unwrap();
    write_bibliography(
        directory.path(),
        &[
            entry(
                "A",
                "Old A",
                "Doe, Jane",
                Some(2020),
                "eprint={2401.00001},",
            ),
            entry(
                "B",
                "Old B",
                "Roe, Richard",
                Some(2020),
                "eprint={2401.00002},",
            ),
        ],
    );
    let a = entry(
        "A",
        "New A",
        "Doe, Jane",
        Some(2024),
        "eprint={2401.00001},",
    );
    let b = entry(
        "B",
        "New B",
        "Roe, Richard",
        Some(2025),
        "eprint={2401.00002},",
    );
    let response = format!("{b}\n\n{a}\n");
    let (base, handle) = server(vec![("200 OK", response.clone())]);
    assert_eq!(
        success(cita_with_server(directory.path(), &["sync"], &base)),
        "Synced 2 references\n"
    );
    let requests = handle.join().unwrap();
    assert!(requests[0].contains("q=texkey%3AA+or+texkey%3AB"));
    assert!(requests[0].contains("size=2"));
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        format!("{a}\n\n{b}\n")
    );

    let (base, handle) = server(vec![("200 OK", response)]);
    assert_eq!(
        success(cita_with_server(directory.path(), &["sync"], &base)),
        "Already in sync\n"
    );
    assert_eq!(handle.join().unwrap().len(), 1);
}

#[test]
fn sync_confirms_and_preserves_an_obsolete_texkey() {
    let directory = tempfile::tempdir().unwrap();
    let old = entry(
        "Old",
        "Old title",
        "Doe, Jane",
        Some(2020),
        "eprint={2401.00001},",
    );
    write_bibliography(directory.path(), &[old]);
    let new = entry(
        "New",
        "Fresh title",
        "Doe, Jane",
        Some(2025),
        "eprint={2401.00001},",
    );
    let (base, handle) = server(vec![("200 OK", new.clone()), ("200 OK", new.clone())]);
    let output = cita_with_server(directory.path(), &["sync"], &base);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Synced 1 references\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "warning: INSPIRE now prefers New for Old; preserving Old\n"
    );
    let stored = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(stored.starts_with("@article{Old,"));
    assert!(stored.contains("Fresh title"));
    assert!(!stored.contains("@article{New,"));
    assert_eq!(handle.join().unwrap().len(), 2);
}

#[test]
fn sync_rejects_unexplained_extra_entries_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let a = entry(
        "A",
        "Alpha",
        "Doe, Jane",
        Some(2024),
        "eprint={2401.00001},",
    );
    write_bibliography(directory.path(), std::slice::from_ref(&a));
    let before = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    let extra = entry(
        "Extra",
        "Extra",
        "Roe, Richard",
        Some(2024),
        "eprint={2401.00002},",
    );
    let (base, handle) = server(vec![("200 OK", format!("{a}\n{extra}"))]);
    let error = failure(cita_with_server(directory.path(), &["sync"], &base));
    assert!(error.contains("unexplained entries: Extra"));
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        before
    );
    handle.join().unwrap();
}

#[test]
fn sync_rejects_missing_keys_and_alias_collisions_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let old_one = entry("Old1", "One", "Doe, Jane", Some(2020), "note={one},");
    let old_two = entry("Old2", "Two", "Roe, Richard", Some(2020), "note={two},");
    write_bibliography(directory.path(), &[old_one, old_two]);
    let before = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    let current = entry(
        "New",
        "Current",
        "Doe, Jane",
        Some(2025),
        "eprint={2401.00001},",
    );
    let (base, handle) = server(vec![
        ("200 OK", current.clone()),
        ("200 OK", current.clone()),
        ("200 OK", current),
    ]);
    let error = failure(cita_with_server(directory.path(), &["sync"], &base));
    assert!(error.contains("resolve to the same current INSPIRE record `New`"));
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        before
    );
    assert_eq!(handle.join().unwrap().len(), 3);

    let missing_dir = tempfile::tempdir().unwrap();
    let missing = entry("Missing", "Gone", "Doe, Jane", Some(2020), "note={gone},");
    write_bibliography(missing_dir.path(), &[missing]);
    let before = fs::read_to_string(missing_dir.path().join("references.bib")).unwrap();
    let (base, handle) = server(vec![("200 OK", String::new()), ("200 OK", String::new())]);
    let error = failure(cita_with_server(missing_dir.path(), &["sync"], &base));
    assert!(error.contains("resolved to 0 entries"));
    assert_eq!(
        fs::read_to_string(missing_dir.path().join("references.bib")).unwrap(),
        before
    );
    assert_eq!(handle.join().unwrap().len(), 2);
}

#[test]
fn fetch_dry_run_uses_stored_eprint_without_a_network_request() {
    let directory = tempfile::tempdir().unwrap();
    write_bibliography(
        directory.path(),
        &[entry(
            "A",
            "Alpha",
            "Doe, Jane",
            Some(2024),
            "eprint={1207.7214},",
        )],
    );
    let output = success(cita(directory.path(), &["fetch", "--dry-run", "A"]));
    assert_eq!(
        output,
        "https://arxiv.org/pdf/1207.7214\n[dry run] skipped download\n"
    );
    assert!(!directory.path().join(".cita").exists());
}

#[test]
fn open_no_download_reports_a_cache_miss_without_creating_cache_layout() {
    let directory = tempfile::tempdir().unwrap();
    write_bibliography(
        directory.path(),
        &[entry(
            "A",
            "Alpha",
            "Doe, Jane",
            Some(2024),
            "eprint={1207.7214},",
        )],
    );
    let error = failure(cita(directory.path(), &["open", "--no-download", "A"]));
    assert!(error.contains("rerun without --no-download"));
    assert!(!directory.path().join(".cita").exists());
    assert!(!directory.path().join(".gitignore").exists());
}

#[test]
fn commit_is_scoped_to_references_bib() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    fs::write(directory.path().join("unrelated.txt"), "one\n").unwrap();
    git_ok(directory.path(), &["add", "unrelated.txt"]);
    git_ok(directory.path(), &["commit", "-qm", "initial"]);
    fs::write(directory.path().join("unrelated.txt"), "two\n").unwrap();
    git_ok(directory.path(), &["add", "unrelated.txt"]);
    write_bibliography(
        directory.path(),
        &[entry(
            "A",
            "Alpha",
            "Doe, Jane",
            Some(2024),
            "eprint={2401.00001},",
        )],
    );
    let output = success(cita(directory.path(), &["commit"]));
    assert!(output.contains("references: initialize cita"));
    assert_eq!(
        git_ok(directory.path(), &["show", "HEAD:references.bib"]),
        fs::read_to_string(directory.path().join("references.bib")).unwrap()
    );
    assert_eq!(
        git_ok(directory.path(), &["diff", "--cached", "--name-only"]),
        "unrelated.txt\n"
    );
}
