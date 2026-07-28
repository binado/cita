use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
    thread,
};

fn bibi(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("BIBI_BIB")
        .output()
        .unwrap()
}

fn bibi_with_env(cwd: &Path, args: &[&str], key: &str, value: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("BIBI_BIB")
        .env(key, value)
        .output()
        .unwrap()
}

fn bibi_with_server(cwd: &Path, args: &[&str], base: &str) -> Output {
    bibi_with_env(cwd, args, "BIBI_INSPIRE_BASE_URL", base)
}

fn bibi_stdin(cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("BIBI_BIB")
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

/// An empty bibliography, which is all a bibi project is.
fn bib(directory: &Path) {
    fs::write(directory.join("references.bib"), "").unwrap();
}

fn read_bib(directory: &Path) -> String {
    fs::read_to_string(directory.join("references.bib")).unwrap()
}

fn entry(key: &str, title: &str, extra: &str) -> String {
    format!("@misc{{{key},\n  title = {{{title}}},\n  {extra}\n}}")
}

/// Canned responses plus the request lines the server saw.
struct TestServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serve canned responses matched by request content.
///
/// Each response pairs with a substring of the request line (usually
/// `format=json` or `format=bibtex`), because the client fetches both halves
/// of a lookup concurrently and either can arrive first. When nothing
/// matches, the last served response repeats, so failure and not-found tests
/// stay deterministic for both request halves. The accept loop runs for the
/// rest of the test; callers read the shared request log once bibi exits.
fn server(responses: Vec<(&'static str, &'static str, String)>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let logged = Arc::clone(&requests);
    thread::spawn(move || {
        let mut pending = responses;
        let mut last: Option<String> = None;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut bytes = [0; 32768];
            let Ok(length) = stream.read(&mut bytes) else {
                continue;
            };
            if length == 0 {
                continue;
            }
            let line = String::from_utf8_lossy(&bytes[..length])
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            let response = match pending
                .iter()
                .position(|(matcher, _, _)| line.contains(matcher))
            {
                Some(at) => {
                    let (_, status, body) = pending.remove(at);
                    let raw = wire(status, &body);
                    last = Some(raw.clone());
                    raw
                }
                None => last
                    .clone()
                    .unwrap_or_else(|| wire("500 Internal Server Error", "no canned response")),
            };
            // bibi may have cancelled its half of a concurrent pair; a failed
            // write is not a test failure.
            let _ = stream.write_all(response.as_bytes());
            logged.lock().unwrap().push(line);
        }
    });
    TestServer {
        base: format!("http://{address}/"),
        requests,
    }
}

fn wire(status: &str, body: &str) -> String {
    let headers = if status.starts_with("429") {
        "Retry-After: 0\r\n"
    } else {
        ""
    };
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn json_record(id: u64, key: &str, title: &str, arxiv: &str) -> String {
    json_record_at(id, key, title, arxiv, "2026-01-01T00:00:00Z")
}

fn json_record_at(id: u64, key: &str, title: &str, arxiv: &str, updated: &str) -> String {
    format!(
        r#"{{"id":"{id}","updated":"{updated}","metadata":{{"titles":[{{"title":"{title}"}}],"authors":[{{"full_name":"Doe, Jane"}}],"texkeys":["{key}"],"arxiv_eprints":[{{"value":"{arxiv}","categories":["hep-th"]}}],"document_type":["article"]}}}}"#
    )
}

fn cached_pdf_for(directory: &Path, arxiv: &str, bytes: &[u8]) {
    let pdf = directory.join(format!(".bibi/files/arxiv/{arxiv}.pdf"));
    fs::create_dir_all(pdf.parent().unwrap()).unwrap();
    fs::write(pdf, bytes).unwrap();
}

fn cached_source_for(directory: &Path, arxiv: &str, name: &str, bytes: &[u8]) {
    let source = directory.join(format!(".bibi/files/arxiv/{arxiv}/source/{name}"));
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(source, bytes).unwrap();
}

fn arxiv_library(directory: &Path) {
    bib(directory);
    let input = format!(
        "{}\n{}",
        entry("Zed", "Cached reference", "eprint={2001.00001},"),
        entry("Alpha", "No eprint", "doi={10.1000/alpha},")
    );
    success(bibi_stdin(directory, &["import", "-"], &input));
}

// ---------------------------------------------------------------- resolution

#[test]
fn a_missing_bibliography_is_reported_with_the_fix_and_never_created() {
    let directory = tempfile::tempdir().unwrap();
    let error = failure(bibi(directory.path(), &["list"]));
    assert!(error.contains("no references.bib at"), "{error}");
    assert!(error.contains("touch references.bib"), "{error}");
    assert!(error.contains("--path"), "{error}");
    assert!(!directory.path().join("references.bib").exists());
}

/// A bibliography in a parent directory is deliberately *not* discovered: a
/// `.bib` is not a project marker, so silently adopting one would be a trap.
#[test]
fn resolution_never_walks_up_to_a_parent_bibliography() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let nested = directory.path().join("chapters");
    fs::create_dir(&nested).unwrap();
    assert!(failure(bibi(&nested, &["list"])).contains("no references.bib at"));
}

#[test]
fn path_and_the_environment_both_select_a_bibliography() {
    let directory = tempfile::tempdir().unwrap();
    let elsewhere = directory.path().join("papers");
    fs::create_dir(&elsewhere).unwrap();
    bib(&elsewhere);
    success(bibi_stdin(
        directory.path(),
        &["--path", "papers/references.bib", "import", "-"],
        &entry("Viaflag", "Chosen by flag", ""),
    ));

    // The flag accepts a `.bib` file path, including after the subcommand.
    assert!(
        success(bibi(
            directory.path(),
            &["list", "-p", "papers/references.bib"]
        ))
        .contains("Chosen by flag")
    );
    let via_env = bibi_with_env(
        directory.path(),
        &["list"],
        "BIBI_BIB",
        elsewhere.join("references.bib").to_str().unwrap(),
    );
    assert!(success(via_env).contains("Chosen by flag"));
    assert!(!directory.path().join("references.bib").exists());
}

#[test]
fn a_directory_path_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let papers = directory.path().join("papers");
    fs::create_dir(&papers).unwrap();
    bib(&papers);
    let error = failure(bibi(directory.path(), &["list", "-p", "papers"]));
    assert!(error.contains("must be a `.bib` file"), "{error}");
    assert!(error.contains("papers"), "{error}");
}

#[test]
fn a_path_without_a_bib_extension_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let error = failure(bibi(directory.path(), &["list", "-p", "papers"]));
    assert!(error.contains("must be a `.bib` file"), "{error}");
}

/// `add` is how a new bibliography comes into being: a missing target starts
/// empty and is created, parent directories included, on the first write.
#[test]
fn add_creates_a_missing_bibliography_file() {
    let directory = tempfile::tempdir().unwrap();
    let json = json_record(42, "Provider:42", "Fresh start", "2401.00042");
    let bibtex = entry("Provider:42", "Fresh start", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    let stdout = success(bibi_with_server(
        directory.path(),
        &["-p", "papers/references.bib", "add", "2401.00042"],
        &server.base,
    ));
    assert!(stdout.contains("{Provider:42,"), "{stdout}");
    let written = fs::read_to_string(directory.path().join("papers/references.bib")).unwrap();
    assert!(written.contains("x-bibi-inspire-id = {42}"), "{written}");
}

/// Same for `import`: a missing target is a fresh start, and the freshly
/// created file is exactly what the command prints.
#[test]
fn import_creates_a_missing_bibliography_and_prints_it() {
    let directory = tempfile::tempdir().unwrap();
    let stdout = success(bibi_stdin(
        directory.path(),
        &["-p", "fresh/references.bib", "import", "-"],
        &entry("First", "First entry", ""),
    ));
    let written = fs::read_to_string(directory.path().join("fresh/references.bib")).unwrap();
    assert_eq!(stdout, written);
    assert!(written.contains("{First,"), "{written}");
}

// -------------------------------------------------------- byte preservation

/// The governing promise of a hand-edited source of truth: a mutation rewrites
/// the entries it touches and nothing else.
#[test]
fn mutations_leave_every_untouched_byte_alone() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    let original = "% Chapter one sources\n@article{Alpha,\n\ttitle = {Alpha},\n\teprint = {2001.00001}\n}\n\n% Chapter two\n@book{Beta,\n  title = {Beta},\n  year = {1995},\n}\n";
    fs::write(&path, original).unwrap();

    success(bibi(directory.path(), &["rekey", "Alpha", "Gamma"]));
    assert_eq!(
        read_bib(directory.path()),
        original.replace("@article{Alpha,", "@article{Gamma,")
    );

    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Delta", "Delta", ""),
    ));
    let after_import = read_bib(directory.path());
    assert!(
        after_import.starts_with("% Chapter one sources"),
        "{after_import}"
    );
    assert!(after_import.contains("% Chapter two"), "{after_import}");
    assert!(
        after_import.contains("\ttitle = {Alpha},"),
        "{after_import}"
    );
    // Appended, not resorted: the user owns the ordering.
    assert!(after_import.trim_end().ends_with('}'), "{after_import}");
    assert!(
        after_import.find("@misc{Delta,") > after_import.find("@book{Beta,"),
        "{after_import}"
    );
}

#[test]
fn removing_the_first_entry_keeps_a_file_header() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    fs::write(
        &path,
        "% my library\n@misc{First,\n  title = {First},\n}\n\n@misc{Second,\n  title = {Second},\n}\n",
    )
    .unwrap();
    success(bibi(directory.path(), &["remove", "First"]));
    assert_eq!(
        read_bib(directory.path()),
        "% my library\n@misc{Second,\n  title = {Second},\n}\n"
    );
}

// -------------------------------------------------------------------- import

#[test]
fn import_preserves_source_bytes_and_skips_duplicates_by_default() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let source = entry("Alpha", "Alpha title", "doi={10.1/ALPHA},");
    // The result of an import is the bibliography itself, printed whole.
    let stdout = success(bibi_stdin(directory.path(), &["import", "-"], &source));
    assert_eq!(stdout, read_bib(directory.path()));
    assert!(stdout.contains(&source), "{stdout}");

    // Same identity under a new key is a collision, not a second entry: the
    // kept entry prints again and the skip is warned about on stderr.
    let again = entry("Beta", "Beta title", "doi={10.1/alpha},");
    let (stdout, stderr) = success_streams(bibi_stdin(directory.path(), &["import", "-"], &again));
    assert!(
        stderr.contains("skipped Beta: already present as Alpha"),
        "{stderr}"
    );
    assert_eq!(stdout, read_bib(directory.path()));
    assert!(!read_bib(directory.path()).contains("Beta title"));
}

#[test]
fn import_overwrite_replaces_the_colliding_entry() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Alpha", "Old", "doi={10.1/X},"),
    ));
    let (stdout, stderr) = success_streams(bibi_stdin(
        directory.path(),
        &["import", "--overwrite", "-"],
        &entry("Renamed", "New", "doi={10.1/X},"),
    ));
    assert!(stderr.contains("overwrote Alpha -> Renamed"), "{stderr}");
    assert_eq!(stdout, read_bib(directory.path()));
    let bibliography = read_bib(directory.path());
    assert!(bibliography.contains("@misc{Renamed,"), "{bibliography}");
    assert!(!bibliography.contains("Old"), "{bibliography}");
}

/// An incoming file is somebody else's working bibliography, so it will have
/// comments in it and must still import.
#[test]
fn import_tolerates_comments_and_directives_in_the_source() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let source = format!(
        "% their notes\n@string{{j = {{Journal}}}}\n\n{}\n\n% trailing\n",
        entry("Theirs", "Their paper", "")
    );
    let stdout = success(bibi_stdin(directory.path(), &["import", "-"], &source));
    assert_eq!(stdout, read_bib(directory.path()));
    let bibliography = read_bib(directory.path());
    assert!(bibliography.contains("@misc{Theirs,"), "{bibliography}");
    // Their commentary is theirs; only entries cross over.
    assert!(!bibliography.contains("their notes"), "{bibliography}");
}

/// Bookkeeping written by someone else's bibi is not evidence about this file.
#[test]
fn import_strips_bibis_own_fields_from_incoming_entries() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let source = entry(
        "Theirs",
        "Their paper",
        "eprint={2001.00001},\n  x-bibi-inspire-id = {999999},\n  x-bibi-frozen = {true},",
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &source));
    let bibliography = read_bib(directory.path());
    assert!(!bibliography.contains("x-bibi-"), "{bibliography}");
    // Their spacing survives verbatim; only bibi's namespace is taken out.
    assert!(
        bibliography.contains("eprint={2001.00001},"),
        "{bibliography}"
    );
}

#[test]
fn import_accepts_several_sources_and_refuses_the_bibliography_itself() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    for (name, key) in [("one.bib", "One"), ("two.bib", "Two")] {
        fs::write(
            directory.path().join(name),
            format!("{}\n", entry(key, key, "")),
        )
        .unwrap();
    }
    let output = success(bibi(directory.path(), &["import", "one.bib", "two.bib"]));
    assert!(
        output.contains("@misc{One,") && output.contains("@misc{Two,"),
        "{output}"
    );

    let error = failure(bibi(directory.path(), &["import", "references.bib"]));
    assert!(error.contains("into itself"), "{error}");
}

// ------------------------------------------------------------ remove / rekey

#[test]
fn remove_resolves_identity_selectors_and_is_atomic() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let error = failure(bibi(directory.path(), &["remove", "Zed", "missing"]));
    assert!(error.contains("no reference matches `missing`"), "{error}");
    assert!(read_bib(directory.path()).contains("@misc{Zed,"));

    assert_eq!(
        success(bibi(directory.path(), &["remove", "2001.00001"])),
        "Removed Zed\n"
    );
    assert!(!read_bib(directory.path()).contains("@misc{Zed,"));
}

#[test]
fn rekey_renames_by_any_selector_and_refuses_a_taken_key() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    assert_eq!(
        success(bibi(directory.path(), &["rekey", "2001.00001", "Higgs"])),
        "Renamed Zed -> Higgs\n"
    );
    assert!(read_bib(directory.path()).contains("@misc{Higgs,"));
    assert!(
        failure(bibi(directory.path(), &["rekey", "Higgs", "Alpha"])).contains("already in use")
    );
}

// --------------------------------------------------------------------- check

#[test]
fn check_passes_a_clean_file_and_reports_every_problem_at_once() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    assert!(success(bibi(directory.path(), &["check"])).contains("is valid: 2 references"));

    fs::write(
        directory.path().join("references.bib"),
        format!(
            "{}\n\n{}\n\n{}\n",
            entry("A", "A", "doi={10.1/DUP},"),
            entry("B", "B", "doi={10.1/dup},"),
            entry("C", "C", "eprint={2001.00001},"),
        ),
    )
    .unwrap();
    let error = failure(bibi(directory.path(), &["check"]));
    assert!(
        error.contains("B: shares the identity doi:10.1/dup with `A`"),
        "{error}"
    );
    assert!(error.contains("has 1 problem"), "{error}");
}

// ---------------------------------------------------------------------- list

#[test]
fn list_sorts_by_the_requested_field() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
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
    success(bibi_stdin(directory.path(), &["import", "-"], &input));

    let by_year = success(bibi(directory.path(), &["list", "--sort-by", "year"]));
    let early = by_year.find("K.early").unwrap();
    let later = by_year.find("K.later").unwrap();
    let undated = by_year.find("K.undated").unwrap();
    assert!(early < later && later < undated, "{by_year}");

    let by_title = success(bibi(directory.path(), &["list", "--sort-by", "title"]));
    assert!(
        by_title.find("Alpha result") < by_title.find("Beta result"),
        "{by_title}"
    );
}

// -------------------------------------------------------------------- export

#[test]
fn export_strips_bibi_fields_and_adds_the_arxiv_url() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bibtex = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    success(bibi_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &server.base,
    ));
    assert!(read_bib(directory.path()).contains("x-bibi-inspire-id"));

    let exported = success(bibi(directory.path(), &["export", "-o", "-"]));
    assert!(!exported.contains("x-bibi-"), "{exported}");
    assert!(
        exported.contains("url = {https://arxiv.org/pdf/2401.00042}"),
        "{exported}"
    );

    let kept = success(bibi(
        directory.path(),
        &["export", "-o", "-", "--keep-metadata"],
    ));
    assert!(kept.contains("x-bibi-inspire-id"), "{kept}");
}

/// Curated `x-bibi-arxiv` still decides the PDF link after the namespace is
/// stripped — projecting the stripped text would lose the id.
#[test]
fn export_uses_curated_arxiv_when_eprint_is_absent() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    fs::write(
        directory.path().join("references.bib"),
        format!(
            "{}\n",
            entry("Curated", "Curated title", "x-bibi-arxiv = {2401.00042},")
        ),
    )
    .unwrap();

    let exported = success(bibi(directory.path(), &["export", "-o", "-"]));
    assert!(!exported.contains("x-bibi-"), "{exported}");
    assert!(
        exported.contains("url = {https://arxiv.org/pdf/2401.00042}"),
        "{exported}"
    );
}

#[test]
fn export_defaults_beside_the_bibliography_and_refuses_to_overwrite_it() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    let output = success(bibi(directory.path(), &["export"]));
    assert!(output.contains(&format!("{name}.bib")), "{output}");
    assert!(directory.path().join(format!("{name}.bib")).exists());

    let error = failure(bibi(directory.path(), &["export", "-o", "references.bib"]));
    assert!(error.contains("the bibliography itself"), "{error}");
}

#[test]
fn export_is_byte_stable_and_keeps_an_authored_url() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry(
            "Authored",
            "Authored",
            "eprint={2001.00001},\n  url = {https://example.test/paper},",
        ),
    ));
    let first = success(bibi(directory.path(), &["export", "-o", "-"]));
    let second = success(bibi(directory.path(), &["export", "-o", "-"]));
    assert_eq!(first, second);
    assert!(first.contains("https://example.test/paper"), "{first}");
    assert!(!first.contains("arxiv.org/pdf"), "{first}");
}

// ----------------------------------------------------------------- add /sync

#[test]
fn add_stores_provider_bookkeeping_as_entry_fields() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bibtex = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    // The output of an add is the stored entry's BibTeX.
    let stdout = success(bibi_with_server(
        directory.path(),
        &[
            "add",
            "--key",
            "Local:42",
            "https://arxiv.org/abs/2401.00042",
        ],
        &server.base,
    ));
    assert!(stdout.contains("@misc{Local:42,"), "{stdout}");
    assert!(stdout.contains("x-bibi-inspire-id = {42}"), "{stdout}");
    // The JSON and BibTeX halves of the lookup are fetched concurrently.
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("format=json")),
        "{requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("format=bibtex")),
        "{requests:?}"
    );

    let bibliography = read_bib(directory.path());
    assert!(bibliography.contains("@misc{Local:42,"), "{bibliography}");
    assert!(
        bibliography.contains("x-bibi-inspire-id = {42}"),
        "{bibliography}"
    );
    assert!(
        bibliography.contains("x-bibi-arxiv = {2401.00042}"),
        "{bibliography}"
    );
    // The provider texkey is not the local key and never becomes one.
    assert!(
        !bibliography.contains("@misc{Provider:42,"),
        "{bibliography}"
    );
}

/// A failed lookup must not leave a stub behind in a file the user owns.
#[test]
fn a_failed_add_leaves_the_bibliography_untouched() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let before = read_bib(directory.path());
    let server = server(vec![("", "500 Internal Server Error", String::new())]);
    assert!(
        !failure(bibi_with_server(
            directory.path(),
            &["add", "2401.00042"],
            &server.base
        ))
        .is_empty()
    );
    assert_eq!(read_bib(directory.path()), before);
}

#[test]
fn sync_refreshes_managed_entries_and_leaves_the_rest_alone() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let json = json_record(42, "Provider:42", "Old", "2401.00042");
    let bibtex = entry("Provider:42", "Old", "eprint={2401.00042},");
    let seed = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    success(bibi_with_server(
        directory.path(),
        &["add", "--key", "Local", "2401.00042"],
        &seed.base,
    ));
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        // No identifiers at all, so sync has nothing to look this one up by
        // and makes no request for it.
        &entry("Imported", "Untouched", ""),
    ));

    let fresh = json_record_at(
        42,
        "Current:42",
        "Fresh",
        "2401.00042",
        "2026-06-01T00:00:00Z",
    );
    let search = format!(r#"{{"hits":{{"hits":[{fresh}]}}}}"#);
    let fresh_bib = entry("Current:42", "Fresh", "eprint={2401.00042},");
    let refresh = server(vec![
        ("format=json", "200 OK", search),
        ("format=bibtex", "200 OK", fresh_bib),
    ]);
    let output = success(bibi_with_server(directory.path(), &["sync"], &refresh.base));
    assert!(
        output.contains("refreshed 1 of 1 managed entries"),
        "{output}"
    );
    assert!(output.contains("1 entries not on INSPIRE"), "{output}");
    let requests = refresh.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.contains("control_number%3A42")),
        "{requests:?}"
    );

    let bibliography = read_bib(directory.path());
    assert!(bibliography.contains("@misc{Local,"), "{bibliography}");
    assert!(bibliography.contains("Fresh"), "{bibliography}");
    assert!(bibliography.contains("@misc{Imported,"), "{bibliography}");
    assert!(bibliography.contains("Untouched"), "{bibliography}");
}

/// An unchanged provider timestamp means there is nothing to write, so the
/// file must come back byte-identical rather than merely equivalent.
#[test]
fn sync_writes_nothing_when_the_provider_timestamp_is_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let json = json_record(42, "Provider:42", "Same", "2401.00042");
    let bibtex = entry("Provider:42", "Same", "eprint={2401.00042},");
    let seed = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    success(bibi_with_server(
        directory.path(),
        &["add", "--key", "Local", "2401.00042"],
        &seed.base,
    ));
    let before = read_bib(directory.path());

    let same = json_record(42, "Provider:42", "Same", "2401.00042");
    let search = format!(r#"{{"hits":{{"hits":[{same}]}}}}"#);
    let refresh = server(vec![
        ("format=json", "200 OK", search),
        (
            "format=bibtex",
            "200 OK",
            entry("Provider:42", "Same", "eprint={2401.00042},"),
        ),
    ]);
    let output = success(bibi_with_server(directory.path(), &["sync"], &refresh.base));
    assert!(
        output.contains("refreshed 0 of 1 managed entries"),
        "{output}"
    );
    assert_eq!(read_bib(directory.path()), before);
}

#[test]
fn an_import_only_bibliography_performs_no_network_work() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", ""),
    ));
    assert_eq!(
        success(bibi_with_server(
            directory.path(),
            &["sync"],
            "http://127.0.0.1:1/"
        )),
        "refreshed 0 of 0 managed entries\n1 entries not on INSPIRE (--verbose to list)\n"
    );
}

#[test]
fn cli_prints_every_inspire_retry_to_stderr() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let json = json_record(42, "Provider:42", "Retried", "2401.00042");
    let bibtex = entry("Provider:42", "Retried", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "429 Too Many Requests", String::new()),
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &server.base,
    ));
    assert!(stdout.contains("{Provider:42,"), "{stdout}");
    assert!(stderr.contains("INSPIRE rate limited"), "{stderr}");
}

// --------------------------------------------------------------------- fetch

#[test]
fn fetch_reuses_a_cached_pdf_and_reports_a_missing_arxiv_id() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_pdf_for(directory.path(), "2001.00001", b"%PDF-1.4 body");
    let (stdout, stderr) = success_streams(bibi(directory.path(), &["fetch", "Zed"]));
    assert!(
        stdout.trim().ends_with(".bibi/files/arxiv/2001.00001.pdf"),
        "{stdout}"
    );
    assert!(stderr.contains("Already fetched Zed"), "{stderr}");

    let error = failure(bibi(directory.path(), &["fetch", "Alpha"]));
    assert!(error.contains("has no arXiv eprint"), "{error}");
}

#[test]
fn fetch_url_prints_only_the_url_without_touching_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    assert_eq!(
        success(bibi(directory.path(), &["fetch", "-u", "Zed"])),
        "https://arxiv.org/pdf/2001.00001\n"
    );
    assert!(!directory.path().join(".bibi").exists());
}

#[test]
fn fetch_source_reuses_the_cached_directory() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_source_for(
        directory.path(),
        "2001.00001",
        "main.tex",
        b"\\documentclass{article}",
    );
    let (stdout, stderr) = success_streams(bibi(directory.path(), &["fetch", "--source", "Zed"]));
    assert!(
        stdout
            .trim()
            .ends_with(".bibi/files/arxiv/2001.00001/source"),
        "{stdout}"
    );
    assert!(
        stderr.contains("Already fetched source for Zed"),
        "{stderr}"
    );
}

#[test]
fn fetch_cache_only_errors_on_a_miss_without_creating_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let error = failure(bibi(directory.path(), &["fetch", "--cache-only", "Zed"]));
    assert!(error.contains("rerun without --cache-only"), "{error}");
    assert!(!directory.path().join(".bibi").exists());
}

#[test]
fn fetch_save_stores_an_unmatched_locator_before_fetching() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let json = json_record(42, "Provider:42", "Saved", "2401.00042");
    let bibtex = entry("Provider:42", "Saved", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
    ]);
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--url", "--save", "2401.00042"],
        &server.base,
    ));
    assert_eq!(stdout, "https://arxiv.org/pdf/2401.00042\n");
    assert!(stderr.contains("added Provider:42"), "{stderr}");
    assert!(read_bib(directory.path()).contains("x-bibi-inspire-id = {42}"));
}

/// Resolving an unmanaged entry and refreshing a managed one are one command,
/// because which of the two applies is bookkeeping the user should not track.
#[test]
fn sync_adopts_an_entry_inspire_recognizes_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let mine = entry("Mine", "My own wording", "eprint={2401.00042},");
    success(bibi_stdin(directory.path(), &["import", "-"], &mine));

    let json = json_record(42, "Provider:42", "Provider wording", "2401.00042");
    let bibtex = entry("Provider:42", "Provider wording", "eprint={2401.00042},");
    let server = server(vec![
        ("format=json", "200 OK", json),
        ("format=bibtex", "200 OK", bibtex),
        (
            "format=json",
            "200 OK",
            format!(
                r#"{{"hits":{{"hits":[{}]}}}}"#,
                json_record(42, "Provider:42", "Provider wording", "2401.00042")
            ),
        ),
        (
            "format=bibtex",
            "200 OK",
            entry("Provider:42", "Provider wording", "eprint={2401.00042},"),
        ),
    ]);
    let output = success(bibi_with_server(directory.path(), &["sync"], &server.base));
    assert!(output.contains("resolved 1 entries"), "{output}");
    assert!(
        output.contains("refreshed 0 of 1 managed entries"),
        "{output}"
    );

    let bibliography = read_bib(directory.path());
    assert!(
        bibliography.contains("x-bibi-inspire-id = {42}"),
        "{bibliography}"
    );
    // Adoption answers "what is this", not "replace it": the wording is mine.
    assert!(bibliography.contains("My own wording"), "{bibliography}");
    assert!(!bibliography.contains("Provider wording"), "{bibliography}");
}

#[test]
fn sync_reports_entries_inspire_does_not_know_without_failing() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Textbook", "A textbook", "doi={10.1/TEXTBOOK},"),
    ));
    let before = read_bib(directory.path());
    let server = server(vec![("", "404 Not Found", String::new())]);
    let output = success(bibi_with_server(
        directory.path(),
        &["sync", "--verbose"],
        &server.base,
    ));
    assert!(output.contains("1 entries not on INSPIRE"), "{output}");
    assert!(output.contains("not on INSPIRE: Textbook"), "{output}");
    assert_eq!(read_bib(directory.path()), before);
}

/// The marker covers both halves of sync: a frozen entry is neither refreshed
/// nor looked up, which is what makes it usable for a hand-corrected entry and
/// for a book INSPIRE will never have.
#[test]
fn sync_leaves_frozen_entries_completely_alone() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    fs::write(
        directory.path().join("references.bib"),
        format!(
            "{}\n",
            entry(
                "Frozen",
                "Mine forever",
                "eprint={2401.00042},\n  x-bibi-frozen = {true},"
            )
        ),
    )
    .unwrap();
    let before = read_bib(directory.path());
    let output = success(bibi_with_server(
        directory.path(),
        &["sync"],
        "http://127.0.0.1:1/",
    ));
    assert!(output.contains("1 frozen entries left alone"), "{output}");
    assert_eq!(read_bib(directory.path()), before);
}

/// import dedupes on DOI and arXiv id, so it cannot catch a pair that only
/// turns out to be one paper once INSPIRE resolves both.
#[test]
fn sync_refuses_two_entries_that_resolve_to_one_record() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    fs::write(
        directory.path().join("references.bib"),
        format!(
            "{}\n\n{}\n",
            entry("ByArxiv", "One", "eprint={2401.00042},"),
            entry("ByDoi", "Two", "doi={10.1/SAME},"),
        ),
    )
    .unwrap();
    let server = server(vec![
        (
            "format=json",
            "200 OK",
            json_record(42, "P:42", "One", "2401.00042"),
        ),
        (
            "format=bibtex",
            "200 OK",
            entry("P:42", "One", "eprint={2401.00042},"),
        ),
        (
            "format=json",
            "200 OK",
            json_record(42, "P:42", "One", "2401.00042"),
        ),
        (
            "format=bibtex",
            "200 OK",
            entry("P:42", "One", "eprint={2401.00042},"),
        ),
    ]);
    let error = failure(bibi_with_server(directory.path(), &["sync"], &server.base));
    assert!(error.contains("both claim INSPIRE record 42"), "{error}");
}

#[test]
fn sync_dry_run_reports_the_plan_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Mine", "Mine", "eprint={2401.00042},"),
    ));
    let before = read_bib(directory.path());
    let server = server(vec![
        (
            "format=json",
            "200 OK",
            json_record(42, "P:42", "Provider", "2401.00042"),
        ),
        (
            "format=bibtex",
            "200 OK",
            entry("P:42", "Provider", "eprint={2401.00042},"),
        ),
        (
            "format=json",
            "200 OK",
            format!(
                r#"{{"hits":{{"hits":[{}]}}}}"#,
                json_record(42, "P:42", "Provider", "2401.00042")
            ),
        ),
        (
            "format=bibtex",
            "200 OK",
            entry("P:42", "Provider", "eprint={2401.00042},"),
        ),
    ]);
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["sync", "--dry-run"],
        &server.base,
    ));
    assert!(stdout.contains("resolved 1 entries"), "{stdout}");
    assert!(stderr.contains("not written"), "{stderr}");
    assert_eq!(read_bib(directory.path()), before);
}

/// The summary points at --verbose, so --verbose has to account for everything
/// the summary counted — including entries with no identifier to look up.
#[test]
fn sync_verbose_lists_every_entry_the_summary_counted() {
    let directory = tempfile::tempdir().unwrap();
    bib(directory.path());
    let input = format!(
        "{}\n{}",
        entry("NoIds", "A textbook", ""),
        entry("Unknown", "Never indexed", "doi={10.1/UNKNOWN},"),
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &input));
    let server = server(vec![("", "404 Not Found", String::new())]);
    let output = success(bibi_with_server(
        directory.path(),
        &["sync", "--verbose"],
        &server.base,
    ));
    assert!(output.contains("2 entries not on INSPIRE"), "{output}");
    assert!(output.contains("not on INSPIRE: Unknown"), "{output}");
    assert!(
        output.contains("no DOI or arXiv id to look up: NoIds"),
        "{output}"
    );
}
