use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output, Stdio},
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

fn cita_stdin(cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
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

fn entry(key: &str, title: &str, extra: &str) -> String {
    format!("@misc{{{key},\n  title = {{{title}}},\n  {extra}\n}}")
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

fn json_record(id: u64, key: &str, title: &str, arxiv: &str) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01T00:00:00Z","metadata":{{"titles":[{{"title":"{title}"}}],"authors":[{{"full_name":"Doe, Jane"}}],"texkeys":["{key}"],"arxiv_eprints":[{{"value":"{arxiv}","categories":["hep-th"]}}],"document_type":["article"]}}}}"#
    )
}

fn git(directory: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap()
}

fn git_success(directory: &Path, args: &[&str]) -> String {
    success(git(directory, args))
}

fn init_git(directory: &Path) {
    git_success(directory, &["init", "-q"]);
    git_success(directory, &["config", "user.email", "cita@example.test"]);
    git_success(directory, &["config", "user.name", "Cita Test"]);
}

#[test]
fn init_creates_schema_two_and_imports_an_existing_bibliography() {
    let empty = tempfile::tempdir().unwrap();
    assert!(success(cita(empty.path(), &["init"])).contains("Initialized"));
    assert_eq!(
        fs::read_to_string(empty.path().join("references.bib")).unwrap(),
        ""
    );
    assert!(
        fs::read_to_string(empty.path().join("cita.toml"))
            .unwrap()
            .starts_with("schema = 2")
    );
    success(cita(empty.path(), &["list"]));

    let imported = tempfile::tempdir().unwrap();
    fs::write(
        imported.path().join("references.bib"),
        format!("{}\n", entry("A", "Alpha", "eprint={2401.00001},")),
    )
    .unwrap();
    success(cita(imported.path(), &["init"]));
    let manifest = fs::read_to_string(imported.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("kind = \"bibtex\""), "{manifest}");
    assert!(success(cita(imported.path(), &["list"])).contains("Alpha"));
}

#[test]
fn legacy_manifest_is_rejected_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), "schema = 1\n").unwrap();
    let error = failure(cita(directory.path(), &["init"]));
    assert!(error.contains("unsupported cita.toml schema 1"), "{error}");
    assert!(!directory.path().join("references.bib").exists());
}

#[test]
fn file_and_stdin_import_are_atomic_and_source_preserving() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let input = entry("B", "Beta", "doi={10.1/B},");
    assert_eq!(
        success(cita_stdin(directory.path(), &["import", "-"], &input)),
        "Added B\n"
    );
    let before_manifest = fs::read(directory.path().join("cita.toml")).unwrap();
    let before_bib = fs::read(directory.path().join("references.bib")).unwrap();
    let conflicting = format!(
        "{}\n{}",
        entry("C", "Gamma", "doi={10.1/C},"),
        entry("D", "Delta", "doi={10.1/b},")
    );
    let error = failure(cita_stdin(directory.path(), &["import", "-"], &conflicting));
    assert!(error.contains("identifier conflict"), "{error}");
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        before_bib
    );
}

#[test]
fn add_uses_json_and_bibtex_and_preserves_an_explicit_local_key() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bib = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    assert_eq!(
        success(cita_with_server(
            directory.path(),
            &["add", "--key", "Local:42", "2401.00042"],
            &base
        )),
        "Added Local:42\n"
    );
    let requests = handle.join().unwrap();
    assert!(requests[0].contains("format=json"));
    assert!(requests[1].contains("format=bibtex"));
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(
        bibliography.starts_with("@misc{Local:42,"),
        "{bibliography}"
    );
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("record_id = 42"));
    assert!(manifest.contains("Provider:42"));
}

#[test]
fn drift_blocks_reads_and_generate_repairs_output() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("references.bib"),
        format!("{}\n", entry("A", "Alpha", "")),
    )
    .unwrap();
    success(cita(directory.path(), &["init"]));
    fs::write(directory.path().join("references.bib"), "edited\n").unwrap();
    assert!(failure(cita(directory.path(), &["list"])).contains("run `cita generate`"));
    success(cita(directory.path(), &["generate"]));
    assert!(success(cita(directory.path(), &["list"])).contains("Alpha"));
}

#[test]
fn sync_refreshes_managed_records_by_id_and_leaves_imports_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let initial_json = json_record(42, "Provider:42", "Old", "2401.00042");
    let initial_bib = entry("Provider:42", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", initial_json), ("200 OK", initial_bib)]);
    success(cita_with_server(
        directory.path(),
        &["add", "--key", "Local", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Imported", "Untouched", "eprint={2401.00999},"),
    ));

    let fresh = json_record(42, "Current:42", "Fresh", "2401.00042");
    let search_json = format!(r#"{{"hits":{{"hits":[{fresh}]}}}}"#);
    let fresh_bib = entry("Current:42", "Fresh", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", search_json), ("200 OK", fresh_bib)]);
    let output = success(cita_with_server(directory.path(), &["sync"], &base));
    assert!(
        output.contains("1 managed references; left 1 imported unchanged"),
        "{output}"
    );
    let requests = handle.join().unwrap();
    assert!(
        requests[0].contains("control_number%3A42"),
        "{}",
        requests[0]
    );
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{Local,"));
    assert!(bibliography.contains("Fresh"));
    assert!(bibliography.contains("@misc{Imported,"));
    assert!(bibliography.contains("Untouched"));
}

#[test]
fn imported_only_sync_performs_no_network_work() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("references.bib"),
        format!("{}\n", entry("A", "Alpha", "")),
    )
    .unwrap();
    success(cita(directory.path(), &["init"]));
    assert_eq!(
        success(cita_with_server(
            directory.path(),
            &["sync"],
            "http://127.0.0.1:1/"
        )),
        "Already in sync: 0 managed, 1 imported\n"
    );
}

#[test]
fn commit_forces_only_the_two_tracked_artifacts_and_leaves_other_staging_alone() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    fs::write(directory.path().join(".gitignore"), "*.bib\n").unwrap();
    fs::write(directory.path().join("notes.txt"), "keep staged\n").unwrap();
    git_success(directory.path(), &["add", "notes.txt"]);
    success(cita(directory.path(), &["init"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    success(cita(directory.path(), &["commit"]));
    let committed = git_success(
        directory.path(),
        &["show", "--pretty=format:", "--name-only", "HEAD"],
    );
    assert!(committed.contains("cita.toml"), "{committed}");
    assert!(committed.contains("references.bib"), "{committed}");
    assert!(!committed.contains("notes.txt"), "{committed}");
    assert_eq!(
        git_success(directory.path(), &["diff", "--cached", "--name-only"]),
        "notes.txt\n"
    );
}
