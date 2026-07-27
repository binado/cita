use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
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

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cita"));
        command
            .current_dir(cwd)
            .args(args)
            .env("CITA_HOME", &self.home)
            .env("NO_COLOR", "1");
        command
    }

    fn cita(&self, args: &[&str]) -> Output {
        self.command(&self.work, args).output().unwrap()
    }

    fn cita_from(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().unwrap()
    }

    fn cita_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(&self.work, args)
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

    fn manifest(&self, shelf: &str) -> PathBuf {
        self.home.join("shelves").join(shelf).join("shelf.toml")
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

#[test]
fn first_data_command_initializes_the_global_main_shelf() {
    let harness = Harness::new();
    assert_eq!(success(harness.cita(&["list"])), "");
    assert!(harness.home.join("library.toml").is_file());
    assert!(harness.manifest("main").is_file());
    assert!(harness.home.join("files").is_dir());
    assert!(!harness.work.join("cita.toml").exists());
    assert!(!harness.work.join("references.bib").exists());
    assert!(!harness.work.join(".gitignore").exists());

    let shelves = success(harness.cita(&["shelf", "list"]));
    assert!(shelves.contains("main   yes"), "{shelves}");
}

#[test]
fn shelf_list_columns_stay_aligned_for_longer_names() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "a-very-long-shelf-name"]));
    let shelves = success(harness.cita(&["shelf", "list"]));
    let default_column = |line: &str| line.find("Default").or_else(|| line.find("yes"));
    let mut lines = shelves.lines();
    let header = default_column(lines.next().unwrap()).unwrap();
    for line in lines.filter(|line| line.contains("yes")) {
        assert_eq!(default_column(line), Some(header), "{shelves}");
    }
}

#[test]
fn explicit_init_is_idempotent_and_reports_the_global_registry() {
    let harness = Harness::new();
    let first = success(harness.cita(&["init"]));
    assert!(first.contains("Initialized"), "{first}");
    assert!(first.contains("library.toml"), "{first}");
    let second = success(harness.cita(&["init"]));
    assert!(second.contains("Already initialized"), "{second}");
}

#[test]
fn init_repairs_a_deleted_default_shelf() {
    let harness = Harness::new();
    success(harness.cita(&["init"]));
    fs::remove_dir_all(harness.home.join("shelves/main")).unwrap();

    let repaired = success(harness.cita(&["init"]));
    assert!(repaired.contains("Repaired"), "{repaired}");
    assert!(harness.manifest("main").is_file());
    // The store is usable again, rather than permanently wedged.
    assert_eq!(success(harness.cita(&["list"])), "");
}

#[test]
fn a_damaged_shelf_does_not_block_creating_another() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "bar"]));
    fs::remove_dir_all(harness.home.join("shelves/bar")).unwrap();

    success(harness.cita(&["shelf", "new", "foo"]));
    let shelves = success(harness.cita(&["shelf", "list"]));
    assert!(shelves.contains("foo"), "{shelves}");
    assert!(harness.manifest("foo").is_file());
}

#[test]
fn a_shelf_named_library_does_not_reuse_the_registry_lock() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "library"]));
    success(harness.cita_stdin(&["import", "-s", "library", "-"], &entry("K", "T", "")));
    assert!(harness.home.join("locks/registry.lock").is_file());
    assert!(harness.home.join("locks/shelf-library.lock").is_file());
    // Other shelves keep working, which they would not if `library` had taken the
    // registry's own lock file.
    success(harness.cita(&["list"]));
}

#[test]
fn errors_never_repeat_their_underlying_cause() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    fs::remove_dir_all(harness.home.join("shelves/paper")).unwrap();
    let error = failure(harness.cita(&["list", "-s", "paper"]));

    // A registered-but-absent shelf is a domain state, so it reads as one rather
    // than leaking a bare ENOENT.
    assert!(error.contains("is registered but"), "{error}");
    assert!(error.contains("cita init"), "{error}");
    assert!(
        !error.contains("os error"),
        "a domain state should not surface a raw io error: {error}"
    );
    // The doubling this guards against: the cause in the Display string *and*
    // again in the source chain that anyhow's `{:#}` walks.
    assert!(
        !error.contains("(os error 2): No such file or directory"),
        "{error}"
    );
}

#[test]
fn cita_home_must_be_nonempty_and_absolute() {
    let harness = Harness::new();
    let mut empty = Command::new(env!("CARGO_BIN_EXE_cita"));
    let empty = empty
        .current_dir(&harness.work)
        .arg("list")
        .env("CITA_HOME", "")
        .output()
        .unwrap();
    assert!(failure(empty).contains("cannot be empty"));

    let mut relative = Command::new(env!("CARGO_BIN_EXE_cita"));
    let relative = relative
        .current_dir(&harness.work)
        .arg("list")
        .env("CITA_HOME", "relative")
        .output()
        .unwrap();
    assert!(failure(relative).contains("must be an absolute path"));
}

#[test]
fn shelf_lifecycle_is_deterministic_and_unknown_names_never_create() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "zeta"]));
    success(harness.cita(&["shelf", "create", "alpha"]));
    let listed = success(harness.cita(&["shelf", "ls"]));
    assert!(listed.find("alpha").unwrap() < listed.find("main").unwrap());
    assert!(listed.find("main").unwrap() < listed.find("zeta").unwrap());
    assert!(harness.manifest("alpha").is_file());

    let error = failure(harness.cita(&["list", "-s", "missing"]));
    assert!(error.contains("unknown shelf `missing`"), "{error}");
    assert!(error.contains("registered: alpha, main, zeta"), "{error}");
    assert!(error.contains("cita shelf new missing"), "{error}");
    assert!(!harness.home.join("shelves/missing").exists());
}

#[test]
fn imports_are_caller_relative_and_shelves_are_isolated() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    fs::write(
        harness.work.join("incoming.bib"),
        format!("{}\n", entry("Paper", "Shelf paper", "")),
    )
    .unwrap();
    success(harness.cita(&["import", "-s", "paper", "incoming.bib"]));
    assert!(success(harness.cita(&["list", "-s", "paper"])).contains("Shelf paper"));
    assert_eq!(success(harness.cita(&["list"])), "");
    assert!(
        fs::read_to_string(harness.manifest("paper"))
            .unwrap()
            .contains("source = \"import\"")
    );
}

#[test]
fn manual_legacy_bibtex_migration_preserves_entries_but_ignores_local_metadata() {
    let harness = Harness::new();
    fs::write(harness.work.join("cita.toml"), "schema = 1\n").unwrap();
    fs::write(
        harness.work.join("references.bib"),
        format!("{}\n", entry("Legacy", "Legacy entry", "")),
    )
    .unwrap();
    success(harness.cita(&["shelf", "new", "paper"]));
    success(harness.cita(&["import", "-s", "paper", "references.bib"]));
    let manifest = fs::read_to_string(harness.manifest("paper")).unwrap();
    assert!(manifest.contains("[references.Legacy]"), "{manifest}");
    assert!(manifest.contains("source = \"import\""), "{manifest}");
    assert_eq!(
        fs::read_to_string(harness.work.join("cita.toml")).unwrap(),
        "schema = 1\n"
    );
}

#[test]
fn add_remove_and_list_use_the_selected_global_shelf() {
    let harness = Harness::new();
    let json = r#"{"id":"42","updated":"2026-01-01","metadata":{"titles":[{"title":"Provider title"}],"authors":[{"full_name":"Doe, Jane"}],"texkeys":["Provider:42"],"arxiv_eprints":[{"value":"2401.00042","categories":["hep-th"]}],"document_type":["article"]}}"#.to_owned();
    let bib = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    let output = harness
        .command(&harness.work, &["add", "--key", "Local", "inspire:42"])
        .env("CITA_INSPIRE_BASE_URL", base)
        .output()
        .unwrap();
    assert_eq!(success(output), "added Local\n");
    handle.join().unwrap();
    assert!(success(harness.cita(&["list"])).contains("Provider title"));
    assert_eq!(
        success(harness.cita(&["remove", "inspire:42"])),
        "Removed Local\n"
    );
    assert_eq!(success(harness.cita(&["list"])), "");
}

#[test]
fn export_is_url_enriched_positional_and_byte_stable() {
    let harness = Harness::new();
    let input = entry(
        "Paper",
        "Exported",
        "eprint={2001.00001}, doi={10.1/example},",
    );
    success(harness.cita_stdin(&["import", "-"], &input));

    success(harness.cita(&["export"]));
    let default = harness.work.join("main.bib");
    let rendered = fs::read_to_string(&default).unwrap();
    assert!(rendered.contains("url = {https://arxiv.org/pdf/2001.00001}"));
    assert!(!harness.home.join("shelves/main/references.bib").exists());

    success(harness.cita(&["export", "references.bib"]));
    let explicit = harness.work.join("references.bib");
    let first = fs::read(&explicit).unwrap();
    success(harness.cita(&["export", "references.bib"]));
    assert_eq!(fs::read(&explicit).unwrap(), first);
}

#[test]
fn export_preserves_authored_urls_and_refuses_the_store() {
    let harness = Harness::new();
    let input = entry(
        "Paper",
        "Authored URL",
        "eprint={2001.00001}, url={https://example.test/paper},",
    );
    success(harness.cita_stdin(&["import", "-"], &input));
    success(harness.cita(&["export", "out.bib"]));
    let rendered = fs::read_to_string(harness.work.join("out.bib")).unwrap();
    assert!(rendered.contains("https://example.test/paper"));
    assert!(!rendered.contains("arxiv.org"));

    let store_output = harness.home.join("forbidden.bib");
    let error = failure(harness.cita(&["export", store_output.to_str().unwrap()]));
    assert!(error.contains("inside the global cita store"), "{error}");
    assert!(!store_output.exists());
}

#[cfg(unix)]
#[test]
fn export_refuses_a_symlink_alias_into_the_store() {
    use std::os::unix::fs::symlink;

    let harness = Harness::new();
    success(harness.cita(&["init"]));
    symlink(&harness.home, harness.work.join("store-alias")).unwrap();
    let error = failure(harness.cita(&["export", "store-alias/out.bib"]));
    assert!(error.contains("inside the global cita store"), "{error}");
}

#[test]
fn batch_export_uses_an_existing_directory_and_continues_after_failures() {
    let harness = Harness::new();
    for name in ["alpha", "zeta"] {
        success(harness.cita(&["shelf", "new", name]));
        success(harness.cita_stdin(&["import", "-s", name, "-"], &entry("Key", name, "")));
    }
    success(harness.cita(&["export", "--all-shelves"]));
    for name in ["alpha", "main", "zeta"] {
        assert!(harness.work.join(format!("{name}.bib")).is_file());
    }
    let exports = harness.work.join("exports");
    fs::create_dir(&exports).unwrap();
    fs::write(harness.manifest("main"), "schema = 1\nunknown = true\n").unwrap();
    let output = harness.cita(&["export", "--all-shelves", "exports"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    // Successes stay on stdout in name order, failures and the summary on stderr,
    // so redirecting the data stream still surfaces the errors.
    assert!(
        stdout.find("Shelf alpha: exported").unwrap()
            < stdout.find("Shelf zeta: exported").unwrap()
    );
    assert!(!stdout.contains("failed"), "{stdout}");
    assert!(stderr.contains("Shelf main: failed"), "{stderr}");
    assert!(stderr.contains("1 of 3 shelves failed"), "{stderr}");
    assert!(exports.join("alpha.bib").is_file());
    assert!(exports.join("zeta.bib").is_file());
    assert!(!exports.join("main.bib").exists());

    let error = failure(harness.cita(&["export", "--all-shelves", "missing"]));
    assert!(error.contains("not an existing directory"), "{error}");
}

#[test]
fn batch_ops_continue_when_a_registered_shelf_directory_is_missing() {
    let harness = Harness::new();
    for name in ["alpha", "zeta"] {
        success(harness.cita(&["shelf", "new", name]));
        success(harness.cita_stdin(&["import", "-s", name, "-"], &entry("Key", name, "")));
    }
    // A non-default shelf: `main` is repaired automatically on every open, so it
    // cannot be used to stage a persistently damaged shelf.
    success(harness.cita(&["shelf", "new", "mid"]));
    fs::remove_dir_all(harness.home.join("shelves/mid")).unwrap();

    let export = harness.cita(&["export", "--all-shelves"]);
    assert!(!export.status.success());
    let export_out = String::from_utf8(export.stdout).unwrap();
    let export_err = String::from_utf8(export.stderr).unwrap();
    assert!(
        export_out.find("Shelf alpha: exported").unwrap()
            < export_out.find("Shelf zeta: exported").unwrap()
    );
    assert!(export_err.contains("Shelf mid: failed"), "{export_err}");
    assert!(export_err.contains("1 of 4 shelves failed"), "{export_err}");
    assert!(harness.work.join("alpha.bib").is_file());
    assert!(harness.work.join("zeta.bib").is_file());
    assert!(!harness.work.join("mid.bib").exists());

    let sync = harness.cita(&["sync", "--all-shelves"]);
    assert!(!sync.status.success());
    let sync_out = String::from_utf8(sync.stdout).unwrap();
    let sync_err = String::from_utf8(sync.stderr).unwrap();
    assert!(
        sync_out.find("Shelf alpha: already in sync").unwrap()
            < sync_out.find("Shelf zeta: already in sync").unwrap()
    );
    assert!(sync_err.contains("Shelf mid: failed"), "{sync_err}");
}

#[test]
fn batch_failures_are_absent_from_stdout_and_present_on_stderr() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "alpha"]));
    fs::write(harness.manifest("main"), "schema = 1\nunknown = true\n").unwrap();

    let output = harness.cita(&["export", "--all-shelves"]);
    assert!(!output.status.success());
    // The exit status is only actionable if the reason reaches a stream the caller
    // has not redirected away.
    assert!(
        !String::from_utf8(output.stdout).unwrap().contains("main"),
        "failure text must not land on the data stream"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Shelf main: failed"), "{stderr}");
    assert!(stderr.contains("1 of 2 shelves failed"), "{stderr}");
}

#[test]
fn imported_shelves_sync_without_network_and_batch_in_name_order() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    success(harness.cita_stdin(
        &["import", "-s", "paper", "-"],
        &entry("Key", "Imported", ""),
    ));
    assert_eq!(
        success(harness.cita(&["sync", "-s", "paper"])),
        "Already in sync: 0 managed, 1 imported\n"
    );
    let output = success(harness.cita(&["sync", "--all-shelves"]));
    assert!(output.find("Shelf main:").unwrap() < output.find("Shelf paper:").unwrap());
}

#[test]
fn document_cache_is_shared_across_shelves() {
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "paper"]));
    let input = entry("Cached", "Cached", "eprint={2001.00001},");
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

fn spawn_import(harness: &Harness, path: &str) -> Child {
    harness
        .command(&harness.work, &["import", path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn concurrent_mutations_do_not_lose_updates() {
    let harness = Harness::new();
    success(harness.cita(&["init"]));
    fs::write(harness.work.join("a.bib"), entry("Alpha", "Alpha", "")).unwrap();
    fs::write(harness.work.join("b.bib"), entry("Beta", "Beta", "")).unwrap();
    let first = spawn_import(&harness, "a.bib");
    let second = spawn_import(&harness, "b.bib");
    success(first.wait_with_output().unwrap());
    success(second.wait_with_output().unwrap());
    let manifest = fs::read_to_string(harness.manifest("main")).unwrap();
    assert!(manifest.contains("[references.Alpha]"), "{manifest}");
    assert!(manifest.contains("[references.Beta]"), "{manifest}");
}

/// Hold a store lock file for as long as the returned handle lives.
fn hold_lock(path: &Path) -> fs::File {
    use fs2::FileExt;

    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.lock_exclusive().unwrap();
    file
}

#[test]
fn shelf_locks_are_exclusive_and_scoped_to_one_shelf() {
    // Deterministic where `concurrent_mutations_do_not_lose_updates` is only
    // probabilistic: the lock is held by this process, so the child provably
    // blocks rather than merely racing.
    let harness = Harness::new();
    success(harness.cita(&["shelf", "new", "held"]));
    success(harness.cita(&["shelf", "new", "free"]));
    fs::write(harness.work.join("a.bib"), entry("Alpha", "Alpha", "")).unwrap();

    let lock = hold_lock(&harness.home.join("locks/shelf-held.lock"));

    let mut blocked = harness
        .command(&harness.work, &["import", "-s", "held", "a.bib"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A different shelf must not be serialized behind `held`.
    success(harness.cita(&["import", "-s", "free", "a.bib"]));
    assert!(
        blocked.try_wait().unwrap().is_none(),
        "import proceeded while another process held the shelf lock"
    );

    drop(lock);
    success(blocked.wait_with_output().unwrap());
    let manifest = fs::read_to_string(harness.manifest("held")).unwrap();
    assert!(manifest.contains("[references.Alpha]"), "{manifest}");
}

#[cfg(unix)]
#[test]
fn cita_home_defaults_to_dot_cita_in_the_home_directory() {
    // The only test that exercises the path every real user's library lives at;
    // it stays hermetic by pointing HOME at a temporary directory.
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
    assert!(home.join(".cita/library.toml").is_file(), "{reported}");
    assert!(home.join(".cita/shelves/main/shelf.toml").is_file());
    assert!(reported.contains(".cita/library.toml"), "{reported}");
}

#[test]
fn remove_resolves_identity_selectors_and_is_atomic() {
    let harness = Harness::new();
    success(harness.cita_stdin(
        &["import", "-"],
        &format!(
            "{}\n\n{}",
            entry("Alpha", "Alpha", "doi = {10.1000/example},"),
            entry(
                "Beta",
                "Beta",
                "eprint = {2101.00001},\n  archivePrefix = {arXiv},"
            )
        ),
    ));

    // A single unresolvable selector must abort the whole batch, leaving the
    // manifest byte-identical rather than removing the resolvable ones.
    let before = fs::read_to_string(harness.manifest("main")).unwrap();
    let error = failure(harness.cita(&["remove", "Alpha", "missing"]));
    assert!(error.contains("missing"), "{error}");
    assert_eq!(
        fs::read_to_string(harness.manifest("main")).unwrap(),
        before
    );

    // DOI and arXiv selectors resolve through normalization, not just local keys.
    let removed = success(harness.cita(&["remove", "https://doi.org/10.1000/EXAMPLE"]));
    assert!(removed.contains("Removed Alpha"), "{removed}");
    let removed = success(harness.cita(&["remove", "arXiv:2101.00001"]));
    assert!(removed.contains("Removed Beta"), "{removed}");
    assert_eq!(success(harness.cita(&["list"])), "");
}

#[test]
fn commands_are_independent_of_the_working_directory() {
    let harness = Harness::new();
    let other = harness.work.join("nested/elsewhere");
    fs::create_dir_all(&other).unwrap();
    success(harness.cita_stdin(&["import", "-"], &entry("Global", "Global", "")));
    assert!(success(harness.cita_from(&other, &["list"])).contains("Global"));
    success(harness.cita_from(&other, &["export"]));
    assert!(other.join("main.bib").is_file());
}
