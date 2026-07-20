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
    success_streams(output).0
}

fn success_streams(output: Output) -> (String, String) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
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
                let headers = if status.starts_with("429") {
                    "Retry-After: 0\r\n"
                } else {
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
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

fn json_record_with_doi(id: u64, key: &str, title: &str, arxiv: &str, doi: &str) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01T00:00:00Z","metadata":{{"titles":[{{"title":"{title}"}}],"authors":[{{"full_name":"Doe, Jane"}}],"texkeys":["{key}"],"arxiv_eprints":[{{"value":"{arxiv}","categories":["hep-th"]}}],"dois":[{{"value":"{doi}"}}],"document_type":["article"]}}}}"#
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

#[cfg(unix)]
fn install_failing_hook(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let hooks = directory.join("test-hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\necho hook rejected commit >&2\nexit 1\n").unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();
    git_success(directory, &["config", "core.hooksPath", "test-hooks"]);
}

fn cita_without_git(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env("PATH", "")
        .output()
        .unwrap()
}

fn sortable_library(directory: &Path) {
    success(cita(directory, &["init"]));
    let input = format!(
        "{}\n{}\n{}",
        entry(
            "K.later",
            "Beta result",
            "author={Zimmerman, Zed}, year={2020},"
        ),
        entry(
            "K.early",
            "Delta result",
            "author={Aaronson, Ann}, year={1990},"
        ),
        entry("K.undated", "Alpha result", "author={Median, Mia},")
    );
    success(cita_stdin(directory, &["import", "-"], &input));
}

fn key_positions(output: &str, keys: [&str; 3]) -> [usize; 3] {
    keys.map(|key| {
        output
            .find(key)
            .unwrap_or_else(|| panic!("{key} missing from:\n{output}"))
    })
}

fn arxiv_library(directory: &Path) {
    success(cita(directory, &["init"]));
    let input = format!(
        "{}\n{}",
        entry("Zed", "Cached reference", "eprint={2001.00001},"),
        entry("Alpha", "No eprint", "doi={10.1000/alpha},")
    );
    success(cita_stdin(directory, &["import", "-"], &input));
}

fn cached_pdf(directory: &Path, bytes: &[u8]) {
    cached_pdf_for(directory, "2001.00001", bytes);
}

fn cached_pdf_for(directory: &Path, arxiv: &str, bytes: &[u8]) {
    let pdf = directory.join(format!(".cita/files/arxiv/{arxiv}.pdf"));
    fs::create_dir_all(pdf.parent().unwrap()).unwrap();
    fs::write(pdf, bytes).unwrap();
}

#[test]
fn init_creates_schema_one_and_imports_an_existing_bibliography() {
    let empty = tempfile::tempdir().unwrap();
    assert!(success(cita(empty.path(), &["init"])).contains("Initialized"));
    assert_eq!(
        fs::read_to_string(empty.path().join("references.bib")).unwrap(),
        ""
    );
    assert!(
        fs::read_to_string(empty.path().join("cita.toml"))
            .unwrap()
            .starts_with("schema = 1")
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
    assert!(manifest.contains("source = \"import\""), "{manifest}");
    assert!(success(cita(imported.path(), &["list"])).contains("Alpha"));
}

#[test]
fn init_defaults_to_the_current_directory_and_accepts_an_explicit_path() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    let nested = directory.path().join("nested/project");
    fs::create_dir_all(&nested).unwrap();

    let output = success(cita(&nested, &["init"]));
    assert!(output.contains(&nested.join("cita.toml").display().to_string()));
    assert!(nested.join("cita.toml").is_file());
    assert!(!directory.path().join("cita.toml").exists());

    let explicit = directory.path().join("explicit");
    fs::create_dir(&explicit).unwrap();
    let output = success(cita(
        &nested,
        &["init", "--path", explicit.to_str().unwrap()],
    ));
    assert!(output.contains(&explicit.join("cita.toml").display().to_string()));
    assert!(explicit.join("cita.toml").is_file());

    let missing = directory.path().join("missing");
    let error = failure(cita(
        &nested,
        &["init", "--path", missing.to_str().unwrap()],
    ));
    assert!(error.contains("is not an existing directory"), "{error}");
    assert!(!missing.exists());
}

#[test]
fn init_ignores_a_malformed_git_directory() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(".git")).unwrap();

    success(cita(directory.path(), &["init"]));
    assert!(directory.path().join("cita.toml").is_file());
    assert!(directory.path().join("references.bib").is_file());
}

#[test]
fn init_ignores_a_broken_git_file() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join(".git"), "gitdir: missing\n").unwrap();

    success(cita(directory.path(), &["init"]));
    assert!(directory.path().join("cita.toml").is_file());
    assert!(directory.path().join("references.bib").is_file());
}

#[test]
fn nested_projects_discover_the_nearest_manifest() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();
    success(cita(&nested, &["init"]));
    success(cita_stdin(
        &nested,
        &["import", "-"],
        &entry("Nested", "Nested project", ""),
    ));
    let child = nested.join("child");
    fs::create_dir(&child).unwrap();

    assert!(success(cita(&child, &["list"])).contains("Nested project"));
    assert!(
        !fs::read_to_string(directory.path().join("cita.toml"))
            .unwrap()
            .contains("Nested")
    );
}

#[test]
fn legacy_manifest_is_rejected_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cita.toml"), "schema = 2\n").unwrap();
    let error = failure(cita(directory.path(), &["init"]));
    assert!(error.contains("unsupported cita.toml schema 2"), "{error}");
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
fn add_key_conflicts_recommend_an_explicit_local_key() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Provider:42", "Imported", "doi={10.1000/imported},"),
    ));
    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);

    let error = failure(cita_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(error.contains("citation key conflict"), "{error}");
    assert!(error.contains("cita add --key <key> <locator>"), "{error}");
}

#[test]
fn cli_prints_every_inspire_retry_to_stderr() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bib = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let (base, handle) = server(vec![
        ("429 Too Many Requests", String::new()),
        ("429 Too Many Requests", String::new()),
        ("429 Too Many Requests", String::new()),
        ("200 OK", json),
        ("200 OK", bib),
    ]);
    let output = cita_with_server(directory.path(), &["add", "2401.00042"], &base);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    for attempt in 1..=3 {
        let message = format!("attempt {attempt} of 3");
        assert_eq!(stderr.matches(&message).count(), 1, "{stderr}");
    }
    assert_eq!(
        stderr.matches("INSPIRE rate limited").count(),
        3,
        "{stderr}"
    );
    assert_eq!(handle.join().unwrap().len(), 5);
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
fn stale_provider_texkeys_remain_selectable_for_save_and_remove() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let initial_json = json_record(42, "Provider:Old", "Old", "2401.00042");
    let initial_bib = entry("Provider:Old", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", initial_json), ("200 OK", initial_bib)]);
    success(cita_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();

    let fresh = json_record(42, "Provider:New", "Fresh", "2401.00042");
    let search_json = format!(r#"{{"hits":{{"hits":[{fresh}]}}}}"#);
    let fresh_bib = entry("Provider:New", "Fresh", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", search_json), ("200 OK", fresh_bib)]);
    success(cita_with_server(directory.path(), &["sync"], &base));
    handle.join().unwrap();

    cached_pdf_for(directory.path(), "2401.00042", b"%PDF-cached");
    let (stdout, stderr) = success_streams(cita_with_server(
        directory.path(),
        &["fetch", "--save", "inspire:42"],
        "http://127.0.0.1:1/",
    ));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".cita/files/arxiv/2401.00042.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        concat!(
            "Already present: Provider:Old\n",
            "Already fetched Provider:Old: https://arxiv.org/pdf/2401.00042\n"
        )
    );
    assert_eq!(
        success(cita(directory.path(), &["remove", "inspire:42"])),
        "Removed Provider:Old\n"
    );
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

#[test]
fn commit_refuses_prestaged_managed_files_without_touching_the_index() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    success(cita(directory.path(), &["commit"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    git_success(directory.path(), &["add", "cita.toml"]);
    let before = git_success(directory.path(), &["diff", "--cached"]);

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("already has staged changes"), "{error}");
    assert!(error.contains("commit or unstage"), "{error}");
    assert_eq!(git_success(directory.path(), &["diff", "--cached"]), before);
}

#[test]
fn commit_help_mentions_the_managed_file_staging_guard() {
    let directory = tempfile::tempdir().unwrap();
    let help = success(cita(directory.path(), &["commit", "--help"]));
    assert!(
        help.contains("refuses to run if either managed file is already staged"),
        "{help}"
    );
}

#[test]
fn commit_checks_the_index_before_reporting_a_no_op() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    success(cita(directory.path(), &["commit"]));
    git_success(directory.path(), &["rm", "--cached", "cita.toml"]);
    let before = git_success(directory.path(), &["diff", "--cached"]);

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("already has staged changes"), "{error}");
    assert_eq!(git_success(directory.path(), &["diff", "--cached"]), before);
    assert!(directory.path().join("cita.toml").is_file());
}

#[test]
fn commit_handles_a_head_that_does_not_contain_managed_paths() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    fs::write(directory.path().join("README"), "existing history\n").unwrap();
    git_success(directory.path(), &["add", "README"]);
    git_success(directory.path(), &["commit", "-qm", "initial"]);
    success(cita(directory.path(), &["init"]));

    assert!(success(cita(directory.path(), &["commit"])).contains("references: initialize cita"));
    let names = git_success(
        directory.path(),
        &["show", "--pretty=format:", "--name-only", "HEAD"],
    );
    assert!(names.contains("cita.toml"), "{names}");
    assert!(names.contains("references.bib"), "{names}");
}

#[cfg(unix)]
#[test]
fn failed_initial_commit_removes_only_citas_new_index_entries() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    fs::write(directory.path().join("notes.txt"), "keep staged\n").unwrap();
    git_success(directory.path(), &["add", "notes.txt"]);
    success(cita(directory.path(), &["init"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    install_failing_hook(directory.path());

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("hook rejected commit"), "{error}");
    assert_eq!(
        git_success(directory.path(), &["diff", "--cached", "--name-only"]),
        "notes.txt\n"
    );
    assert!(directory.path().join("cita.toml").is_file());
    assert!(directory.path().join("references.bib").is_file());
}

#[cfg(unix)]
#[test]
fn failed_later_commit_resets_managed_index_entries_and_preserves_other_staging() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    success(cita(directory.path(), &["commit"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    fs::write(directory.path().join("notes.txt"), "keep staged\n").unwrap();
    git_success(directory.path(), &["add", "notes.txt"]);
    install_failing_hook(directory.path());

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("hook rejected commit"), "{error}");
    assert_eq!(
        git_success(directory.path(), &["diff", "--cached", "--name-only"]),
        "notes.txt\n"
    );
    let unstaged = git_success(directory.path(), &["diff", "--name-only"]);
    assert!(unstaged.contains("cita.toml"), "{unstaged}");
    assert!(unstaged.contains("references.bib"), "{unstaged}");
}

#[cfg(unix)]
#[test]
fn failed_commit_restores_a_tracked_manifest_and_unstages_a_new_bibliography() {
    assert_failed_commit_rolls_back_asymmetric_head("cita.toml", "references.bib");
}

#[cfg(unix)]
#[test]
fn failed_commit_restores_a_tracked_bibliography_and_unstages_a_new_manifest() {
    assert_failed_commit_rolls_back_asymmetric_head("references.bib", "cita.toml");
}

#[cfg(unix)]
fn assert_failed_commit_rolls_back_asymmetric_head(tracked: &str, newly_staged: &str) {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    git_success(directory.path(), &["add", tracked]);
    git_success(
        directory.path(),
        &["commit", "-qm", "partial managed history"],
    );
    let head_index_entry = git_success(directory.path(), &["ls-files", "--stage", tracked]);

    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    let manifest = fs::read(directory.path().join("cita.toml")).unwrap();
    let bibliography = fs::read(directory.path().join("references.bib")).unwrap();
    fs::write(directory.path().join("notes.txt"), "keep staged\n").unwrap();
    git_success(directory.path(), &["add", "notes.txt"]);
    install_failing_hook(directory.path());

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("hook rejected commit"), "{error}");
    assert_eq!(
        git_success(directory.path(), &["diff", "--cached", "--name-only"]),
        "notes.txt\n"
    );
    assert_eq!(
        git_success(directory.path(), &["ls-files", "--stage", tracked]),
        head_index_entry
    );
    assert!(
        !git(
            directory.path(),
            &["ls-files", "--error-unmatch", newly_staged]
        )
        .status
        .success()
    );
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        manifest
    );
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        bibliography
    );
}

#[test]
fn remove_is_atomic_and_resolves_identity_selectors() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let input = format!(
        "{}\n{}",
        entry("Alpha", "First", "doi={10.1000/example},"),
        entry("Zed", "Second", "")
    );
    success(cita_stdin(directory.path(), &["import", "-"], &input));
    let manifest_path = directory.path().join("cita.toml");
    let before = fs::read(&manifest_path).unwrap();
    let error = failure(cita(directory.path(), &["remove", "Alpha", "missing"]));
    assert!(
        error.contains("reference `missing` was not found"),
        "{error}"
    );
    assert_eq!(fs::read(&manifest_path).unwrap(), before);
    assert_eq!(
        success(cita(directory.path(), &["remove", "doi:10.1000/example"])),
        "Removed Alpha\n"
    );
    assert!(
        !fs::read_to_string(&manifest_path)
            .unwrap()
            .contains("Alpha")
    );
    assert!(
        !fs::read_to_string(directory.path().join("references.bib"))
            .unwrap()
            .contains("First")
    );
}

#[test]
fn list_order_desc_reverses_the_key_sort() {
    let directory = tempfile::tempdir().unwrap();
    sortable_library(directory.path());
    let ascending = success(cita(directory.path(), &["list"]));
    let [early, later, undated] = key_positions(&ascending, ["K.early", "K.later", "K.undated"]);
    assert!(early < later && later < undated, "{ascending}");
    let descending = success(cita(directory.path(), &["list", "--order", "desc"]));
    let [early, later, undated] = key_positions(&descending, ["K.early", "K.later", "K.undated"]);
    assert!(undated < later && later < early, "{descending}");
}

#[test]
fn list_sorts_by_title_and_author() {
    let directory = tempfile::tempdir().unwrap();
    sortable_library(directory.path());
    let by_title = success(cita(directory.path(), &["list", "--sort-by", "title"]));
    let [early, later, undated] = key_positions(&by_title, ["K.early", "K.later", "K.undated"]);
    assert!(undated < later && later < early, "{by_title}");
    let by_author = success(cita(directory.path(), &["list", "--sort-by", "author"]));
    let [early, later, undated] = key_positions(&by_author, ["K.early", "K.later", "K.undated"]);
    assert!(early < undated && undated < later, "{by_author}");
}

#[test]
fn list_displays_authors_then_collaboration_then_placeholder() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let input = [
        entry("One", "One", "author={Alice},"),
        entry("Many", "Many", "author={Alice and Bob},"),
        entry("Team", "Team", "collaboration={ATLAS Collaboration},"),
        entry("Nobody", "Nobody", "note={none},"),
    ]
    .join("\n");
    success(cita_stdin(directory.path(), &["import", "-"], &input));
    let output = success(cita(directory.path(), &["list"]));
    let one = output
        .lines()
        .find(|line| line.starts_with("One "))
        .unwrap();
    let many = output
        .lines()
        .find(|line| line.starts_with("Many "))
        .unwrap();
    let team = output
        .lines()
        .find(|line| line.starts_with("Team "))
        .unwrap();
    let nobody = output
        .lines()
        .find(|line| line.starts_with("Nobody "))
        .unwrap();
    assert!(one.contains("Alice"), "{one}");
    assert!(!one.contains("et al."), "{one}");
    assert!(many.contains("Alice et al."), "{many}");
    assert!(team.contains("ATLAS Collaboration"), "{team}");
    assert!(nobody.contains('—'), "{nobody}");
}

#[test]
fn list_sorts_by_year_and_keeps_missing_years_last() {
    let directory = tempfile::tempdir().unwrap();
    sortable_library(directory.path());
    let ascending = success(cita(directory.path(), &["list", "--sort-by", "year"]));
    let [early, later, undated] = key_positions(&ascending, ["K.early", "K.later", "K.undated"]);
    assert!(early < later && later < undated, "{ascending}");
    let descending = success(cita(
        directory.path(),
        &["list", "--sort-by", "year", "--order", "desc"],
    ));
    let [early, later, undated] = key_positions(&descending, ["K.early", "K.later", "K.undated"]);
    assert!(later < early && early < undated, "{descending}");
}

#[test]
fn fetch_reuses_cached_pdf_and_reports_missing_arxiv_id() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_pdf(directory.path(), b"%PDF-cached");
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let (stdout, stderr) = success_streams(cita(&nested, &["fetch", "Zed"]));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".cita/files/arxiv/2001.00001.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        "Already fetched Zed: https://arxiv.org/pdf/2001.00001\n"
    );
    let (stdout, stderr) = success_streams(cita(&nested, &["fetch", "--cache-only", "Zed"]));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".cita/files/arxiv/2001.00001.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        "Already fetched Zed: https://arxiv.org/pdf/2001.00001\n"
    );
    let error = failure(cita(&nested, &["fetch", "--force", "Alpha"]));
    assert!(
        error.contains("reference `Alpha` has no arXiv eprint"),
        "{error}"
    );
}

#[test]
fn fetch_url_prints_only_the_url_without_touching_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    fs::remove_dir_all(directory.path().join(".cita")).unwrap();
    assert_eq!(
        success(cita(directory.path(), &["fetch", "-u", "Zed"])),
        "https://arxiv.org/pdf/2001.00001\n"
    );
    assert!(!directory.path().join(".cita").exists());
}

#[test]
fn fetch_url_can_save_metadata_without_creating_the_pdf_cache() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    fs::remove_dir_all(directory.path().join(".cita")).unwrap();
    let record = json_record(42, "Provider:42", "Saved", "2401.00042");
    let bibtex = entry("Provider:42", "Saved", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", record), ("200 OK", bibtex)]);
    let (stdout, stderr) = success_streams(cita_with_server(
        directory.path(),
        &["fetch", "--url", "--save", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert_eq!(stdout, "https://arxiv.org/pdf/2401.00042\n");
    assert_eq!(stderr, "Added Provider:42\n");
    assert!(!directory.path().join(".cita").exists());
    assert!(
        fs::read_to_string(directory.path().join("cita.toml"))
            .unwrap()
            .contains("record_id = 42")
    );
}

#[test]
fn fetch_save_uses_the_actual_existing_local_key() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    let old_json = json_record(42, "Provider:Old", "Old", "2401.00042");
    let old_bib = entry("Provider:Old", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", old_json), ("200 OK", old_bib)]);
    success(cita_with_server(
        directory.path(),
        &["add", "--key", "Local", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    cached_pdf_for(directory.path(), "2401.00042", b"%PDF-cached");

    let new_json = json_record_with_doi(42, "Provider:New", "New", "2401.00042", "10.1000/new");
    let new_bib = entry(
        "Provider:New",
        "New",
        "eprint={2401.00042}, doi={10.1000/new},",
    );
    let (base, handle) = server(vec![("200 OK", new_json), ("200 OK", new_bib)]);
    let (_, stderr) = success_streams(cita_with_server(
        directory.path(),
        &["fetch", "--save", "doi:10.1000/new"],
        &base,
    ));
    handle.join().unwrap();
    assert_eq!(
        stderr,
        concat!(
            "Already present: Local\n",
            "Already fetched Local: https://arxiv.org/pdf/2401.00042\n"
        )
    );
}

#[test]
fn fetch_save_key_conflicts_recommend_cita_add_with_a_key() {
    let directory = tempfile::tempdir().unwrap();
    success(cita(directory.path(), &["init"]));
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Provider:42", "Imported", "doi={10.1000/imported},"),
    ));
    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    let error = failure(cita_with_server(
        directory.path(),
        &["fetch", "--url", "--save", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(error.contains("citation key conflict"), "{error}");
    assert!(error.contains("cita add --key <key> <locator>"), "{error}");
}

#[test]
fn fetch_force_and_url_are_mutually_exclusive() {
    let directory = tempfile::tempdir().unwrap();
    let error = failure(cita(
        directory.path(),
        &["fetch", "--force", "--url", "Zed"],
    ));
    assert!(error.contains("cannot be used with"), "{error}");
}

#[test]
fn fetch_suggests_force_for_an_invalid_cached_pdf() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_pdf(directory.path(), b"not a PDF");
    let error = failure(cita(directory.path(), &["fetch", "Zed"]));
    assert!(error.contains("retry with --force"), "{error}");
}

#[test]
fn fetch_cache_only_errors_on_a_cache_miss_without_creating_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    fs::remove_dir_all(directory.path().join(".cita")).unwrap();
    let error = failure(cita(directory.path(), &["fetch", "--cache-only", "Zed"]));
    assert!(error.contains("PDF is not cached"), "{error}");
    assert!(error.contains("rerun without --cache-only"), "{error}");
    assert!(!directory.path().join(".cita").exists());
}

#[test]
fn fetch_cache_only_suggests_force_for_an_invalid_cached_pdf() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_pdf(directory.path(), b"not a PDF");
    let error = failure(cita(directory.path(), &["fetch", "--cache-only", "Zed"]));
    assert!(error.contains("retry with --force"), "{error}");
}

#[test]
fn commit_reports_a_git_launch_failure_instead_of_guessing() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    let error = failure(cita_without_git(directory.path(), &["commit"]));
    assert!(error.contains("could not run git"), "{error}");
}

#[test]
fn commit_treats_a_present_but_unusable_git_marker_as_an_error() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(".git")).unwrap();
    success(cita(directory.path(), &["init"]));

    let error = failure(cita(directory.path(), &["commit"]));
    assert!(
        error.contains("git rev-parse --show-toplevel failed"),
        "{error}"
    );
    assert!(!error.contains("is not inside a Git repository"), "{error}");
}

#[test]
fn init_does_not_require_an_available_git_executable() {
    let directory = tempfile::tempdir().unwrap();
    success(cita_without_git(directory.path(), &["init"]));
    assert!(directory.path().join("cita.toml").is_file());
}

#[test]
fn commit_surfaces_corrupt_history_instead_of_pretending_a_first_commit() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());
    success(cita(directory.path(), &["init"]));
    success(cita(directory.path(), &["commit"]));
    corrupt_loose_objects(directory.path());
    success(cita_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    let error = failure(cita(directory.path(), &["commit"]));
    assert!(error.contains("git diff --cached failed"), "{error}");
}

fn corrupt_loose_objects(directory: &Path) {
    for shard in fs::read_dir(directory.join(".git/objects")).unwrap() {
        let shard = shard.unwrap();
        if !shard.file_type().unwrap().is_dir() || shard.file_name().len() != 2 {
            continue;
        }
        for object in fs::read_dir(shard.path()).unwrap() {
            // Loose objects are read-only; replace them instead of rewriting.
            let path = object.unwrap().path();
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"garbage").unwrap();
        }
    }
}
