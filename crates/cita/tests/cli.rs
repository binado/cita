use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
};

struct Harness {
    _root: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("cita-home");
        let work = root.path().join("work");
        fs::create_dir(&work).unwrap();
        Self {
            _root: root,
            home,
            work,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cita"));
        command
            .current_dir(&self.work)
            .args(args)
            .env("CITA_HOME", &self.home)
            .env("NO_COLOR", "1");
        command
    }

    fn cita(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn cita_with_base(&self, args: &[&str], base: &str) -> Output {
        self.command(args)
            .env("CITA_INSPIRE_BASE_URL", base)
            .output()
            .unwrap()
    }

    fn cita_stdin(&self, args: &[&str], input: &str) -> Output {
        self.cita_stdin_with_base(args, input, None)
    }

    fn cita_stdin_base(&self, args: &[&str], input: &str, base: &str) -> Output {
        self.cita_stdin_with_base(args, input, Some(base))
    }

    fn cita_stdin_with_base(&self, args: &[&str], input: &str, base: Option<&str>) -> Output {
        let mut command = self.command(args);
        if let Some(base) = base {
            command.env("CITA_INSPIRE_BASE_URL", base);
        }
        let mut child = command
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
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stderr).unwrap()
}

fn entry(key: &str, title: &str, extra: &str) -> String {
    format!("@misc{{{key},\n  title = {{{title}}},\n  {extra}\n}}")
}

fn inspire_json(id: u64, key: &str, title: &str, doi: &str) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01","metadata":{{"titles":[{{"title":"{title}"}}],"texkeys":["{key}"],"dois":[{{"value":"{doi}"}}]}}}}"#
    )
}

fn inspire_bib(key: &str, title: &str, doi: &str) -> String {
    entry(key, title, &format!("doi = {{{doi}}},"))
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

#[test]
fn lazy_initialization_uses_sqlite_and_ignores_legacy_files() {
    let harness = Harness::new();
    fs::create_dir_all(&harness.home).unwrap();
    fs::write(harness.home.join("library.toml"), "legacy = true\n").unwrap();
    assert_eq!(success(harness.cita(&["list"])), "");
    assert!(harness.home.join("library.sqlite3").is_file());
    assert!(harness.home.join("files").is_dir());
    assert_eq!(
        fs::read_to_string(harness.home.join("library.toml")).unwrap(),
        "legacy = true\n"
    );
    assert!(!harness.home.join("locks").exists());
    assert!(!harness.home.join("shelves").exists());
}

#[test]
fn init_is_idempotent_and_from_file_requires_overwrite() {
    let harness = Harness::new();
    assert!(success(harness.cita(&["init"])).contains("Initialized"));
    assert!(success(harness.cita(&["init"])).contains("Already initialized"));
    success(harness.cita_stdin(&["import", "-"], &entry("One", "Original", "")));
    success(harness.cita(&["export", "--all-shelves", "--format", "toml", "cita.toml"]));
    success(harness.cita_stdin(
        &["import", "--overwrite", "-"],
        &entry("One", "Replacement", ""),
    ));
    let error = failure(harness.cita(&["init", "--from-file", "cita.toml"]));
    assert!(error.contains("--overwrite"), "{error}");
    success(harness.cita(&["init", "--from-file", "cita.toml", "--overwrite"]));
    assert!(success(harness.cita(&["list"])).contains("Original"));
}

#[test]
fn one_global_reference_can_use_different_shelf_keys() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    let (base, handle) = server(vec![
        ("404 Not Found", String::new()),
        ("404 Not Found", String::new()),
    ]);
    success(harness.cita_stdin_base(
        &["import", "-"],
        &entry("MainKey", "Shared", "doi = {10.1/shared},"),
        &base,
    ));
    success(harness.cita_stdin_base(
        &["import", "-s", "paper", "-"],
        &entry("PaperKey", "Incoming rendition", "doi = {10.1/shared},"),
        &base,
    ));
    handle.join().unwrap();
    assert!(success(harness.cita(&["list"])).contains("MainKey"));
    let paper = success(harness.cita(&["list", "-s", "paper"]));
    assert!(
        paper.contains("PaperKey") && paper.contains("Shared"),
        "{paper}"
    );
    success(harness.cita(&["export", "--all-shelves", "--format", "json", "cita.json"]));
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(harness.work.join("cita.json")).unwrap()).unwrap();
    assert_eq!(value["references"].as_array().unwrap().len(), 1);
}

#[test]
fn overwrite_replaces_only_the_selected_shelf_membership() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    let (base, handle) = server(vec![
        ("404 Not Found", String::new()),
        ("404 Not Found", String::new()),
    ]);
    success(harness.cita_stdin_base(
        &["import", "-"],
        &entry("Main", "Shared", "doi = {10.1/shared},"),
        &base,
    ));
    success(harness.cita_stdin_base(
        &["import", "-s", "paper", "-"],
        &entry("Local", "Shared rendition", "doi = {10.1/shared},"),
        &base,
    ));
    handle.join().unwrap();
    success(harness.cita_stdin(
        &["import", "-s", "paper", "--overwrite", "-"],
        &entry("Local", "Paper only", ""),
    ));
    assert!(success(harness.cita(&["list"])).contains("Shared"));
    let paper = success(harness.cita(&["list", "-s", "paper"]));
    assert!(paper.contains("Paper only"), "{paper}");
}

#[test]
fn import_canonicalizes_through_inspire() {
    let harness = Harness::new();
    let (base, handle) = server(vec![
        (
            "200 OK",
            inspire_json(42, "Provider:42", "Canonical", "10.1/source"),
        ),
        (
            "200 OK",
            inspire_bib("Provider:42", "Canonical", "10.1/source"),
        ),
    ]);
    assert_eq!(
        success(harness.cita_stdin_base(
            &["import", "-"],
            &entry("Local", "Imported", "doi = {10.1/source},"),
            &base,
        )),
        "added Local\n"
    );
    handle.join().unwrap();
    assert!(success(harness.cita(&["list"])).contains("Canonical"));
    success(harness.cita(&["export", "--format", "json", "main.json"]));
    let json = fs::read_to_string(harness.work.join("main.json")).unwrap();
    assert!(json.contains("\"source\": \"inspire\""), "{json}");
    assert!(json.contains("\"record_id\": 42"), "{json}");
}

#[test]
fn transient_canonicalization_failure_keeps_the_import() {
    let harness = Harness::new();
    let (base, handle) = server(vec![("500 Internal Server Error", "later".into())]);
    let output = harness.cita_stdin_base(
        &["import", "-"],
        &entry("Local", "Imported", "doi = {10.1/source},"),
        &base,
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("kept imported BibTeX"));
    handle.join().unwrap();
    assert!(success(harness.cita(&["list"])).contains("Imported"));
}

#[test]
fn import_is_atomic_by_default_and_skip_errors_commits_valid_entries() {
    let source = format!(
        "{}\n\n{}",
        entry("Bad", "Bad", "doi = {10.1/bad},"),
        entry("Good", "Good", "")
    );

    let atomic = Harness::new();
    let (base, handle) = server(vec![("200 OK", "{bad json".into())]);
    let error = failure(atomic.cita_stdin_base(&["import", "-"], &source, &base));
    assert!(error.contains("canonicalize"), "{error}");
    handle.join().unwrap();
    assert_eq!(success(atomic.cita(&["list"])), "");

    let partial = Harness::new();
    let (base, handle) = server(vec![("200 OK", "{bad json".into())]);
    let output = partial.cita_stdin_base(&["import", "--skip-errors", "-"], &source, &base);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Skipped 1"));
    handle.join().unwrap();
    assert!(success(partial.cita(&["list"])).contains("Good"));
}

#[test]
fn skip_errors_can_isolate_semantically_invalid_bibtex_entries() {
    let source = "@misc{Bad,\n note={missing title}\n}\n\n@misc{Good,\n title={Good}\n}\n";
    let atomic = Harness::new();
    assert!(failure(atomic.cita_stdin(&["import", "-"], source)).contains("title"));
    assert_eq!(success(atomic.cita(&["list"])), "");

    let partial = Harness::new();
    let output = partial.cita_stdin(&["import", "--skip-errors", "-"], source);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Skipped Bad"));
    assert!(success(partial.cita(&["list"])).contains("Good"));
}

#[test]
fn sync_promotes_imports_and_shared_memberships_observe_it() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    let (base, handle) = server(vec![
        ("404 Not Found", String::new()),
        ("404 Not Found", String::new()),
    ]);
    success(harness.cita_stdin_base(
        &["import", "-"],
        &entry("Main", "Imported", "doi = {10.1/promote},"),
        &base,
    ));
    success(harness.cita_stdin_base(
        &["import", "-s", "paper", "-"],
        &entry("Paper", "Other rendition", "doi = {10.1/promote},"),
        &base,
    ));
    handle.join().unwrap();

    let (base, handle) = server(vec![
        (
            "200 OK",
            inspire_json(7, "Provider:7", "Canonical", "10.1/promote"),
        ),
        (
            "200 OK",
            inspire_bib("Provider:7", "Canonical", "10.1/promote"),
        ),
    ]);
    let output = success(harness.cita_with_base(&["sync", "--all-shelves"], &base));
    assert!(output.contains("1 promoted"), "{output}");
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 2, "shared reference must resolve only once");
    assert!(success(harness.cita(&["list"])).contains("Canonical"));
    assert!(success(harness.cita(&["list", "-s", "paper"])).contains("Canonical"));
}

#[test]
fn malformed_managed_refresh_leaves_the_selected_set_unchanged() {
    let harness = Harness::new();
    let (base, handle) = server(vec![
        (
            "200 OK",
            inspire_json(9, "Provider:9", "Managed", "10.1/managed"),
        ),
        (
            "200 OK",
            inspire_bib("Provider:9", "Managed", "10.1/managed"),
        ),
    ]);
    success(harness.cita_stdin_base(
        &["import", "-"],
        &entry("Local", "Imported", "doi = {10.1/managed},"),
        &base,
    ));
    handle.join().unwrap();
    success(harness.cita(&["export", "--format", "json", "before.json"]));

    let (base, handle) = server(vec![("200 OK", "{malformed".into())]);
    let error = failure(harness.cita_with_base(&["sync"], &base));
    assert!(error.contains("malformed"), "{error}");
    handle.join().unwrap();
    success(harness.cita(&["export", "--format", "json", "after.json"]));
    assert_eq!(
        fs::read(harness.work.join("before.json")).unwrap(),
        fs::read(harness.work.join("after.json")).unwrap()
    );
}

#[test]
fn bibtex_and_lossless_exports_are_deterministic_and_round_trip() {
    let harness = Harness::new();
    success(harness.cita_stdin(
        &["import", "-"],
        &entry(
            "Paper",
            "Exported",
            "eprint = {2001.00001},\n  archivePrefix = {arXiv},",
        ),
    ));
    success(harness.cita(&["export", "references.bib"]));
    let first = fs::read(harness.work.join("references.bib")).unwrap();
    assert!(String::from_utf8_lossy(&first).contains("url = {https://arxiv.org/pdf/2001.00001}"));
    success(harness.cita(&["export", "references.bib"]));
    assert_eq!(
        fs::read(harness.work.join("references.bib")).unwrap(),
        first
    );

    for format in ["json", "toml"] {
        let file = format!("cita.{format}");
        success(harness.cita(&["export", "--all-shelves", "--format", format, &file]));
        let restored = Harness::new();
        fs::copy(harness.work.join(&file), restored.work.join(&file)).unwrap();
        success(restored.cita(&["init", "--from-file", &file]));
        success(restored.cita(&["export", "references.bib"]));
        assert_eq!(
            fs::read(restored.work.join("references.bib")).unwrap(),
            first
        );
    }
}

#[test]
fn remove_is_atomic_and_collects_orphans() {
    let harness = Harness::new();
    success(harness.cita_stdin(
        &["import", "-"],
        &format!(
            "{}\n\n{}",
            entry("Alpha", "Alpha", ""),
            entry("Beta", "Beta", "")
        ),
    ));
    let error = failure(harness.cita(&["remove", "Alpha", "missing"]));
    assert!(error.contains("missing"), "{error}");
    let listed = success(harness.cita(&["list"]));
    assert!(listed.contains("Alpha") && listed.contains("Beta"));
    success(harness.cita(&["remove", "Alpha", "Beta"]));
    assert_eq!(success(harness.cita(&["list"])), "");
    success(harness.cita(&["export", "--all-shelves", "--format", "json", "empty.json"]));
    let json = fs::read_to_string(harness.work.join("empty.json")).unwrap();
    assert!(json.contains("\"references\": []"), "{json}");
}

fn spawn_import(harness: &Harness, path: &str) -> Child {
    harness
        .command(&["import", path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn concurrent_imports_do_not_lose_updates() {
    let harness = Harness::new();
    success(harness.cita(&["init"]));
    fs::write(harness.work.join("a.bib"), entry("Alpha", "Alpha", "")).unwrap();
    fs::write(harness.work.join("b.bib"), entry("Beta", "Beta", "")).unwrap();
    let first = spawn_import(&harness, "a.bib");
    let second = spawn_import(&harness, "b.bib");
    success(first.wait_with_output().unwrap());
    success(second.wait_with_output().unwrap());
    let listed = success(harness.cita(&["list"]));
    assert!(
        listed.contains("Alpha") && listed.contains("Beta"),
        "{listed}"
    );
}

#[test]
fn document_cache_remains_shared_across_shelves() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    let input = entry(
        "Cached",
        "Cached",
        "eprint = {2001.00001},\n  archivePrefix = {arXiv},",
    );
    for shelf in ["main", "paper"] {
        success(harness.cita_stdin(&["import", "-s", shelf, "-"], &input));
    }
    let pdf = harness.home.join("files/arxiv/2001.00001.pdf");
    fs::create_dir_all(pdf.parent().unwrap()).unwrap();
    fs::write(&pdf, b"%PDF-1.4\n").unwrap();
    let main = success(harness.cita(&["fetch", "--cache-only", "Cached"]));
    let paper = success(harness.cita(&["fetch", "--cache-only", "-s", "paper", "Cached"]));
    assert_eq!(main.trim(), pdf.display().to_string());
    assert_eq!(paper.trim(), pdf.display().to_string());
}

#[test]
fn exports_still_refuse_the_store_and_symlink_aliases() {
    let harness = Harness::new();
    success(harness.cita(&["init"]));
    let forbidden = harness.home.join("out.json");
    let error = failure(harness.cita(&["export", "--format", "json", forbidden.to_str().unwrap()]));
    assert!(error.contains("inside the global cita store"), "{error}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&harness.home, harness.work.join("store-alias")).unwrap();
        let error = failure(harness.cita(&["export", "store-alias/out.bib"]));
        assert!(error.contains("inside the global cita store"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn cita_home_defaults_to_dot_cita() {
    let harness = Harness::new();
    let home = harness.work.join("fake-home");
    fs::create_dir(&home).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(&harness.work)
        .arg("init")
        .env_remove("CITA_HOME")
        .env("HOME", &home)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let reported = success(output);
    assert!(home.join(".cita/library.sqlite3").is_file(), "{reported}");
}
