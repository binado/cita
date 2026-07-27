use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
};

fn bibi(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn bibi_with_server(cwd: &Path, args: &[&str], base: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        .env("BIBI_INSPIRE_BASE_URL", base)
        .output()
        .unwrap()
}

fn bibi_stdin(cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
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

/// An empty schema-1 project, written directly now that `init` is gone.
///
/// An empty reference set renders to an empty bibliography, so these two files
/// are exactly what the removed command produced and they verify against each
/// other.
fn project(directory: &Path) {
    fs::write(directory.join("cita.toml"), "schema = 1\n").unwrap();
    fs::write(directory.join("references.bib"), "").unwrap();
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

fn sortable_library(directory: &Path) {
    project(directory);
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
    success(bibi_stdin(directory, &["import", "-"], &input));
}

fn key_positions(output: &str, keys: [&str; 3]) -> [usize; 3] {
    keys.map(|key| {
        output
            .find(key)
            .unwrap_or_else(|| panic!("{key} missing from:\n{output}"))
    })
}

/// Whether `directory` is on a case-insensitive filesystem, probed so the
/// regression test means something on both macOS and Linux CI.
fn case_insensitive(directory: &Path) -> bool {
    let probe = directory.join("case-probe");
    fs::write(&probe, b"probe").unwrap();
    let insensitive = directory.join("CASE-PROBE").exists();
    fs::remove_file(&probe).unwrap();
    insensitive
}

fn arxiv_library(directory: &Path) {
    project(directory);
    let input = format!(
        "{}\n{}",
        entry("Zed", "Cached reference", "eprint={2001.00001},"),
        entry("Alpha", "No eprint", "doi={10.1000/alpha},")
    );
    success(bibi_stdin(directory, &["import", "-"], &input));
}

fn cached_pdf(directory: &Path, bytes: &[u8]) {
    cached_pdf_for(directory, "2001.00001", bytes);
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

#[test]
fn nested_projects_discover_the_nearest_manifest() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();
    project(&nested);
    success(bibi_stdin(
        &nested,
        &["import", "-"],
        &entry("Nested", "Nested project", ""),
    ));
    let child = nested.join("child");
    fs::create_dir(&child).unwrap();

    assert!(success(bibi(&child, &["list"])).contains("Nested project"));
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
    let error = failure(bibi(directory.path(), &["list"]));
    assert!(error.contains("unsupported cita.toml schema 2"), "{error}");
    assert!(!directory.path().join("references.bib").exists());
}

#[test]
fn import_is_source_preserving_and_skips_duplicates_by_default() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let input = entry("B", "Beta", "doi={10.1/B},");
    assert_eq!(
        success(bibi_stdin(directory.path(), &["import", "-"], &input)),
        "added B\n"
    );
    // An import mixing a fresh entry with a different-key duplicate of an
    // existing DOI adds the fresh one and skips the duplicate, exiting 0.
    let mixed = format!(
        "{}\n{}",
        entry("C", "Gamma", "doi={10.1/C},"),
        entry("D", "Delta", "doi={10.1/b},")
    );
    let output = success(bibi_stdin(directory.path(), &["import", "-"], &mixed));
    assert!(output.contains("added C"), "{output}");
    assert!(
        output.contains("skipped D: already present as B"),
        "{output}"
    );
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("[references.C]"), "{manifest}");
    assert!(!manifest.contains("[references.D]"), "{manifest}");
}

#[test]
fn import_reports_identical_duplicates_as_skipped() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("A", "Alpha", "doi={10.1/A},"),
    ));
    // A batch re-listing an identical existing entry alongside a fresh one adds
    // the new entry and reports the identical one as an idempotent "already
    // present" no-op, printed per-line as `skipped A` with no aggregate summary.
    let mixed = format!(
        "{}\n{}",
        entry("A", "Alpha", "doi={10.1/A},"),
        entry("B", "Beta", "doi={10.1/B},")
    );
    let output = success(bibi_stdin(directory.path(), &["import", "-"], &mixed));
    assert!(output.contains("added B"), "{output}");
    assert!(output.contains("skipped A"), "{output}");
    assert!(!output.contains(" added,"), "{output}");
}

#[test]
fn import_overwrite_rekeys_a_duplicate_and_leaves_others_untouched() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("foo", "Foo", "eprint={2107.00001},"),
    ));

    // bar duplicates foo's arXiv identity; baz is fresh.
    let second = format!(
        "{}\n{}",
        entry("bar", "Bar", "eprint={2107.00001},"),
        entry("baz", "Baz", "eprint={2202.00002},")
    );
    let output = success(bibi_stdin(directory.path(), &["import", "-"], &second));
    assert!(
        output.contains("skipped bar: already present as foo"),
        "{output}"
    );
    assert!(output.contains("added baz"), "{output}");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("[references.foo]"), "{manifest}");
    assert!(manifest.contains("[references.baz]"), "{manifest}");
    assert!(!manifest.contains("[references.bar]"), "{manifest}");

    // Re-import with --overwrite rekeys foo -> bar and leaves baz as-is.
    let output = success(bibi_stdin(
        directory.path(),
        &["import", "--overwrite", "-"],
        &second,
    ));
    assert!(output.contains("overwrote foo -> bar"), "{output}");
    assert!(output.contains("skipped baz"), "{output}");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("[references.bar]"), "{manifest}");
    assert!(manifest.contains("[references.baz]"), "{manifest}");
    assert!(!manifest.contains("[references.foo]"), "{manifest}");

    // The generated bibliography follows the rekey with no drift.
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{bar,"), "{bibliography}");
    assert!(!bibliography.contains("@misc{foo,"), "{bibliography}");
}

#[test]
fn import_overwrite_reports_every_removed_collision() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let existing = format!(
        "{}\n{}",
        entry("DoiOwner", "DOI owner", "doi={10.1/BRIDGE},"),
        entry("ArxivOwner", "arXiv owner", "eprint={2401.00042},")
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &existing));

    let incoming = entry(
        "Combined",
        "Combined",
        "doi={10.1/bridge}, eprint={2401.00042},",
    );
    assert_eq!(
        success(bibi_stdin(
            directory.path(),
            &["import", "--overwrite", "-"],
            &incoming,
        )),
        "overwrote DoiOwner, ArxivOwner -> Combined\n"
    );

    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("[references.Combined]"), "{manifest}");
    assert!(!manifest.contains("[references.DoiOwner]"), "{manifest}");
    assert!(!manifest.contains("[references.ArxivOwner]"), "{manifest}");
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{Combined,"), "{bibliography}");
    assert!(!bibliography.contains("@misc{DoiOwner,"), "{bibliography}");
    assert!(
        !bibliography.contains("@misc{ArxivOwner,"),
        "{bibliography}"
    );
}

#[test]
fn add_overwrite_replaces_a_colliding_local_key() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Provider:42", "Imported", "doi={10.1000/imported},"),
    ));
    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);

    let output = success(bibi_with_server(
        directory.path(),
        &["add", "--overwrite", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(output.contains("overwrote Provider:42"), "{output}");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("record_id = 42"), "{manifest}");
    assert!(!manifest.contains("10.1000/imported"), "{manifest}");
}

#[test]
fn add_exact_overwrite_reports_and_removes_all_collisions() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());

    let old_json = json_record(42, "Provider:Old", "Old provider", "2301.00042");
    let old_bib = entry("Provider:Old", "Old provider", "eprint={2301.00042},");
    let (base, handle) = server(vec![("200 OK", old_json), ("200 OK", old_bib)]);
    success(bibi_with_server(
        directory.path(),
        &["add", "--key", "Local", "inspire:42"],
        &base,
    ));
    handle.join().unwrap();

    let occupied = format!(
        "{}\n{}",
        entry("Renamed", "Requested key occupant", "doi={10.1/FREED},"),
        entry("ArxivOwner", "arXiv owner", "eprint={2401.00042},")
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &occupied));

    let new_json = json_record(42, "Provider:New", "New provider", "2401.00042");
    let new_bib = entry("Provider:New", "New provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", new_json), ("200 OK", new_bib)]);
    assert_eq!(
        success(bibi_with_server(
            directory.path(),
            &["add", "--overwrite", "--key", "Renamed", "inspire:42",],
            &base,
        )),
        "overwrote Renamed, Local, ArxivOwner -> Renamed\n"
    );
    handle.join().unwrap();

    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("[references.Renamed]"), "{manifest}");
    assert!(manifest.contains("record_id = 42"), "{manifest}");
    assert!(!manifest.contains("[references.Local]"), "{manifest}");
    assert!(!manifest.contains("[references.ArxivOwner]"), "{manifest}");
    assert!(!manifest.contains("10.1/FREED"), "{manifest}");
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{Renamed,"), "{bibliography}");
    assert!(!bibliography.contains("@misc{Local,"), "{bibliography}");
    assert!(
        !bibliography.contains("@misc{ArxivOwner,"),
        "{bibliography}"
    );
}

#[test]
fn add_accepts_an_arxiv_url_and_preserves_an_explicit_local_key() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bib = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    assert_eq!(
        success(bibi_with_server(
            directory.path(),
            &[
                "add",
                "--key",
                "Local:42",
                "https://arxiv.org/abs/2401.00042",
            ],
            &base
        )),
        "added Local:42\n"
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
fn add_skips_a_suggested_key_that_collides_with_different_content() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Provider:42", "Imported", "doi={10.1000/imported},"),
    ));
    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);

    // The suggested texkey already holds unrelated imported content: the add is
    // skipped (not fatal) and the existing entry is left untouched.
    let output = success(bibi_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(
        output.contains("skipped Provider:42: local key already holds different content"),
        "{output}"
    );
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("10.1000/imported"), "{manifest}");
    assert!(!manifest.contains("record_id = 42"), "{manifest}");
}

#[test]
fn cli_prints_every_inspire_retry_to_stderr() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let json = json_record(42, "Provider:42", "Provider title", "2401.00042");
    let bib = entry("Provider:42", "Provider title", "eprint={2401.00042},");
    let (base, handle) = server(vec![
        ("429 Too Many Requests", String::new()),
        ("429 Too Many Requests", String::new()),
        ("429 Too Many Requests", String::new()),
        ("200 OK", json),
        ("200 OK", bib),
    ]);
    let output = bibi_with_server(directory.path(), &["add", "2401.00042"], &base);
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
fn sync_refreshes_managed_records_by_id_and_leaves_imports_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let initial_json = json_record(42, "Provider:42", "Old", "2401.00042");
    let initial_bib = entry("Provider:42", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", initial_json), ("200 OK", initial_bib)]);
    success(bibi_with_server(
        directory.path(),
        &["add", "--key", "Local", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Imported", "Untouched", "eprint={2401.00999},"),
    ));

    let fresh = json_record(42, "Current:42", "Fresh", "2401.00042");
    let search_json = format!(r#"{{"hits":{{"hits":[{fresh}]}}}}"#);
    let fresh_bib = entry("Current:42", "Fresh", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", search_json), ("200 OK", fresh_bib)]);
    let output = success(bibi_with_server(directory.path(), &["sync"], &base));
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
    project(directory.path());
    let initial_json = json_record(42, "Provider:Old", "Old", "2401.00042");
    let initial_bib = entry("Provider:Old", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", initial_json), ("200 OK", initial_bib)]);
    success(bibi_with_server(
        directory.path(),
        &["add", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();

    let fresh = json_record(42, "Provider:New", "Fresh", "2401.00042");
    let search_json = format!(r#"{{"hits":{{"hits":[{fresh}]}}}}"#);
    let fresh_bib = entry("Provider:New", "Fresh", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", search_json), ("200 OK", fresh_bib)]);
    success(bibi_with_server(directory.path(), &["sync"], &base));
    handle.join().unwrap();

    cached_pdf_for(directory.path(), "2401.00042", b"%PDF-cached");
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--save", "https://inspirehep.net/literature/42"],
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
                .join(".bibi/files/arxiv/2401.00042.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        concat!(
            "skipped Provider:Old\n",
            "Already fetched Provider:Old: https://arxiv.org/pdf/2401.00042\n"
        )
    );
    assert_eq!(
        success(bibi(directory.path(), &["remove", "inspire:42"])),
        "Removed Provider:Old\n"
    );
}

#[test]
fn imported_only_sync_performs_no_network_work() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
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
        "Already in sync: 0 managed, 1 imported\n"
    );
}

#[test]
fn remove_is_atomic_and_resolves_identity_selectors() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let input = format!(
        "{}\n{}",
        entry("Alpha", "First", "doi={10.1000/example},"),
        entry("Zed", "Second", "")
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &input));
    let manifest_path = directory.path().join("cita.toml");
    let before = fs::read(&manifest_path).unwrap();
    let error = failure(bibi(directory.path(), &["remove", "Alpha", "missing"]));
    assert!(
        error.contains("reference `missing` was not found"),
        "{error}"
    );
    assert_eq!(fs::read(&manifest_path).unwrap(), before);
    assert_eq!(
        success(bibi(
            directory.path(),
            &["remove", "https://doi.org/10.1000/example"]
        )),
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
    let ascending = success(bibi(directory.path(), &["list"]));
    let [early, later, undated] = key_positions(&ascending, ["K.early", "K.later", "K.undated"]);
    assert!(early < later && later < undated, "{ascending}");
    let descending = success(bibi(directory.path(), &["list", "--order", "desc"]));
    let [early, later, undated] = key_positions(&descending, ["K.early", "K.later", "K.undated"]);
    assert!(undated < later && later < early, "{descending}");
}

#[test]
fn list_sorts_by_title_and_author() {
    let directory = tempfile::tempdir().unwrap();
    sortable_library(directory.path());
    let by_title = success(bibi(directory.path(), &["list", "--sort-by", "title"]));
    let [early, later, undated] = key_positions(&by_title, ["K.early", "K.later", "K.undated"]);
    assert!(undated < later && later < early, "{by_title}");
    let by_author = success(bibi(directory.path(), &["list", "--sort-by", "author"]));
    let [early, later, undated] = key_positions(&by_author, ["K.early", "K.later", "K.undated"]);
    assert!(early < undated && undated < later, "{by_author}");
}

#[test]
fn list_displays_authors_then_collaboration_then_placeholder() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let input = [
        entry("One", "One", "author={Alice},"),
        entry("Many", "Many", "author={Alice and Bob},"),
        entry("Team", "Team", "collaboration={ATLAS Collaboration},"),
        entry("Nobody", "Nobody", "note={none},"),
    ]
    .join("\n");
    success(bibi_stdin(directory.path(), &["import", "-"], &input));
    let output = success(bibi(directory.path(), &["list"]));
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
    let ascending = success(bibi(directory.path(), &["list", "--sort-by", "year"]));
    let [early, later, undated] = key_positions(&ascending, ["K.early", "K.later", "K.undated"]);
    assert!(early < later && later < undated, "{ascending}");
    let descending = success(bibi(
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
    let (stdout, stderr) = success_streams(bibi(&nested, &["fetch", "Zed"]));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".bibi/files/arxiv/2001.00001.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        "Already fetched Zed: https://arxiv.org/pdf/2001.00001\n"
    );
    let (stdout, stderr) = success_streams(bibi(&nested, &["fetch", "--cache-only", "Zed"]));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".bibi/files/arxiv/2001.00001.pdf")
                .display()
        )
    );
    assert_eq!(
        stderr,
        "Already fetched Zed: https://arxiv.org/pdf/2001.00001\n"
    );
    let error = failure(bibi(&nested, &["fetch", "--force", "Alpha"]));
    assert!(
        error.contains("reference `Alpha` has no arXiv eprint"),
        "{error}"
    );
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
fn fetch_source_reuses_the_cached_directory_from_nested_working_directories() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_source_for(
        directory.path(),
        "2001.00001",
        "figures/plot.tex",
        b"cached",
    );
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();

    let (stdout, stderr) =
        success_streams(bibi(&nested, &["fetch", "--source", "--cache-only", "Zed"]));
    assert_eq!(
        stdout,
        format!(
            "{}\n",
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join(".bibi/files/arxiv/2001.00001/source")
                .display()
        )
    );
    assert_eq!(stderr, "Already fetched source for Zed\n");
}

#[test]
fn fetch_source_cache_miss_has_guidance_and_creates_no_cache_state() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());

    let error = failure(bibi(
        directory.path(),
        &["fetch", "--source", "--cache-only", "Zed"],
    ));
    assert!(error.contains("source is not cached"), "{error}");
    assert!(error.contains("rerun without --cache-only"), "{error}");
    assert!(!directory.path().join(".bibi").exists());
}

#[test]
fn fetch_source_cache_only_suggests_force_for_an_invalid_cached_source() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    fs::create_dir_all(directory.path().join(".bibi/files/arxiv/2001.00001/source")).unwrap();

    let error = failure(bibi(
        directory.path(),
        &["fetch", "--source", "--cache-only", "Zed"],
    ));
    assert!(error.contains("cached source directory"), "{error}");
    assert!(error.contains("retry with --force"), "{error}");
}

#[test]
fn fetch_source_conflicts_with_url_and_still_checks_for_arxiv_ids() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let conflict = failure(bibi(
        directory.path(),
        &["fetch", "--source", "--url", "Zed"],
    ));
    assert!(conflict.contains("cannot be used with"), "{conflict}");

    let missing = failure(bibi(
        directory.path(),
        &["fetch", "--source", "--cache-only", "Alpha"],
    ));
    assert!(
        missing.contains("reference `Alpha` has no arXiv eprint"),
        "{missing}"
    );
}

#[test]
fn fetch_source_save_uses_the_selected_local_key() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    cached_source_for(directory.path(), "2401.00042", "main.tex", b"cached source");
    let record = json_record(42, "Provider:42", "Saved", "2401.00042");
    let bibtex = entry("Provider:42", "Saved", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", record), ("200 OK", bibtex)]);

    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--source", "--cache-only", "--save", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(stdout.ends_with(".bibi/files/arxiv/2401.00042/source\n"));
    assert_eq!(
        stderr,
        "added Provider:42\nAlready fetched source for Provider:42\n"
    );
}

#[test]
fn fetch_url_can_save_metadata_without_creating_the_pdf_cache() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let record = json_record(42, "Provider:42", "Saved", "2401.00042");
    let bibtex = entry("Provider:42", "Saved", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", record), ("200 OK", bibtex)]);
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--url", "--save", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert_eq!(stdout, "https://arxiv.org/pdf/2401.00042\n");
    assert_eq!(stderr, "added Provider:42\n");
    assert!(!directory.path().join(".bibi").exists());
    assert!(
        fs::read_to_string(directory.path().join("cita.toml"))
            .unwrap()
            .contains("record_id = 42")
    );
}

#[test]
fn fetch_save_uses_the_actual_existing_local_key() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let old_json = json_record(42, "Provider:Old", "Old", "2401.00042");
    let old_bib = entry("Provider:Old", "Old", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", old_json), ("200 OK", old_bib)]);
    success(bibi_with_server(
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
    let (_, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--save", "doi:10.1000/new"],
        &base,
    ));
    handle.join().unwrap();
    assert_eq!(
        stderr,
        concat!(
            "skipped Local\n",
            "Already fetched Local: https://arxiv.org/pdf/2401.00042\n"
        )
    );
}

#[test]
fn fetch_save_skips_a_local_key_that_holds_different_content() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    success(bibi_stdin(
        directory.path(),
        &["import", "-"],
        &entry("Provider:42", "Imported", "doi={10.1000/imported},"),
    ));
    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    // The suggested texkey already holds unrelated imported content: the save is
    // skipped, the URL for the resolved paper is still returned, and the
    // pre-existing entry is left untouched.
    let (stdout, stderr) = success_streams(bibi_with_server(
        directory.path(),
        &["fetch", "--url", "--save", "2401.00042"],
        &base,
    ));
    handle.join().unwrap();
    assert!(stderr.contains("skipped Provider:42"), "{stderr}");
    assert!(stdout.contains("2401.00042"), "{stdout}");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.contains("10.1000/imported"), "{manifest}");
    assert!(!manifest.contains("record_id = 42"), "{manifest}");
}

#[test]
fn fetch_force_and_url_are_mutually_exclusive() {
    let directory = tempfile::tempdir().unwrap();
    let error = failure(bibi(
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
    let error = failure(bibi(directory.path(), &["fetch", "Zed"]));
    assert!(error.contains("retry with --force"), "{error}");
}

#[test]
fn fetch_cache_only_errors_on_a_cache_miss_without_creating_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let error = failure(bibi(directory.path(), &["fetch", "--cache-only", "Zed"]));
    assert!(error.contains("PDF is not cached"), "{error}");
    assert!(error.contains("rerun without --cache-only"), "{error}");
    assert!(!directory.path().join(".bibi").exists());
}

#[test]
fn fetch_cache_only_suggests_force_for_an_invalid_cached_pdf() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    cached_pdf(directory.path(), b"not a PDF");
    let error = failure(bibi(directory.path(), &["fetch", "--cache-only", "Zed"]));
    assert!(error.contains("retry with --force"), "{error}");
}

#[test]
fn export_injects_arxiv_urls_and_leaves_the_generated_bibliography_untouched() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let generated = fs::read(directory.path().join("references.bib")).unwrap();

    let stdout = success(bibi(directory.path(), &["export"]));
    assert!(stdout.starts_with("Exported "), "{stdout}");

    // The export is a separate artifact; references.bib stays authoritative.
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        generated
    );
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    let exported = fs::read_to_string(directory.path().join(format!("{name}.bib"))).unwrap();
    assert!(
        exported.contains("url = {https://arxiv.org/pdf/2001.00001}"),
        "{exported}"
    );
    // The DOI-only entry has no arXiv ID, so it gets no derived URL.
    let alpha = exported
        .split("\n\n")
        .find(|block| block.starts_with("@misc{Alpha,"))
        .unwrap_or_else(|| panic!("{exported}"));
    assert!(!alpha.contains("url"), "{alpha}");
    assert_eq!(exported.matches("url = {").count(), 1, "{exported}");
}

#[test]
fn export_derives_the_url_from_an_inspire_records_curated_arxiv_id() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());

    let json = json_record(42, "Provider:42", "Provider", "2401.00042");
    let bib = entry("Provider:42", "Provider", "eprint={2401.00042},");
    let (base, handle) = server(vec![("200 OK", json), ("200 OK", bib)]);
    success(bibi_with_server(
        directory.path(),
        &["add", "inspire:42"],
        &base,
    ));
    handle.join().unwrap();

    let stdout = success(bibi(directory.path(), &["export"]));
    assert!(stdout.starts_with("Exported "), "{stdout}");
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    let exported = fs::read_to_string(directory.path().join(format!("{name}.bib"))).unwrap();
    assert!(
        exported.contains("url = {https://arxiv.org/pdf/2401.00042}"),
        "{exported}"
    );
}

#[test]
fn export_honors_output_and_resolves_it_against_the_caller() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let nested = directory.path().join("sub");
    fs::create_dir(&nested).unwrap();

    // The manifest is found by walking ancestors, but a relative --output
    // resolves against the caller's directory, as import paths do.
    success(bibi(&nested, &["export", "-o", "custom.bib"]));
    assert!(nested.join("custom.bib").is_file());
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    assert!(!directory.path().join(format!("{name}.bib")).exists());
}

#[test]
fn export_refuses_to_overwrite_the_managed_files() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let generated = fs::read(directory.path().join("references.bib")).unwrap();
    let manifest = fs::read(directory.path().join("cita.toml")).unwrap();

    for target in ["references.bib", "./sub/../references.bib", "cita.toml"] {
        fs::create_dir_all(directory.path().join("sub")).unwrap();
        let stderr = failure(bibi(directory.path(), &["export", "-o", target]));
        assert!(stderr.contains("managed file"), "{stderr}");
    }
    #[cfg(unix)]
    {
        // A symlinked directory must not let the export alias a managed file
        // through a different path.
        std::os::unix::fs::symlink(".", directory.path().join("alias")).unwrap();
        let stderr = failure(bibi(
            directory.path(),
            &["export", "-o", "alias/references.bib"],
        ));
        assert!(stderr.contains("managed file"), "{stderr}");
    }
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        generated
    );
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        manifest
    );
}

#[test]
fn export_refuses_to_overwrite_another_projects_managed_files() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("one");
    let two = root.path().join("two");
    fs::create_dir(&one).unwrap();
    fs::create_dir(&two).unwrap();
    arxiv_library(&one);
    arxiv_library(&two);
    let bibliography = fs::read(two.join("references.bib")).unwrap();
    let manifest = fs::read(two.join("cita.toml")).unwrap();

    // --output is the only path in the CLI that can leave the discovered
    // project, so the guard has to know about every project, not just this one.
    for target in ["../two/references.bib", "../two/cita.toml"] {
        let stderr = failure(bibi(&one, &["export", "-o", target]));
        assert!(stderr.contains("managed file"), "{stderr}");
    }
    assert_eq!(fs::read(two.join("references.bib")).unwrap(), bibliography);
    assert_eq!(fs::read(two.join("cita.toml")).unwrap(), manifest);

    // A managed name is only managed where a project owns it, so the same file
    // name in a plain directory stays a legal target.
    fs::create_dir(root.path().join("plain")).unwrap();
    success(bibi(&one, &["export", "-o", "../plain/references.bib"]));
    assert!(root.path().join("plain/references.bib").is_file());
}

#[test]
fn export_refuses_a_case_alias_of_a_managed_file_on_a_case_insensitive_filesystem() {
    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    let insensitive = case_insensitive(directory.path());
    let generated = fs::read(directory.path().join("references.bib")).unwrap();
    let manifest = fs::read(directory.path().join("cita.toml")).unwrap();

    for target in ["References.bib", "CITA.toml"] {
        let output = bibi(directory.path(), &["export", "-o", target]);
        if insensitive {
            // On this filesystem `target` names the same inode as the managed
            // file, so it already exists; the assertion below on the managed
            // files' bytes is what proves the export did not touch it.
            let stderr = failure(output);
            assert!(stderr.contains("managed file"), "{stderr}");
        } else {
            success(output);
            assert!(directory.path().join(target).is_file());
            fs::remove_file(directory.path().join(target)).unwrap();
        }
    }

    // Either branch must leave both managed files byte-identical.
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        generated
    );
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        manifest
    );
}

#[test]
fn export_is_byte_stable_and_keeps_an_authored_url() {
    let directory = tempfile::tempdir().unwrap();
    project(directory.path());
    let input = entry(
        "Authored",
        "Has its own url",
        "eprint={2001.00001},\n  url = {https://example.test/paper},",
    );
    success(bibi_stdin(directory.path(), &["import", "-"], &input));

    success(bibi(directory.path(), &["export"]));
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    let exported = directory.path().join(format!("{name}.bib"));
    let first = fs::read(&exported).unwrap();
    success(bibi(directory.path(), &["export"]));
    // Re-running must not append a second url or otherwise churn the bytes.
    assert_eq!(fs::read(&exported).unwrap(), first);
    let text = String::from_utf8(first).unwrap();
    assert!(text.contains("https://example.test/paper"), "{text}");
    assert!(!text.contains("arxiv.org"), "{text}");
}

#[test]
fn export_requires_a_project_and_a_current_bibliography() {
    let bare = tempfile::tempdir().unwrap();
    let stderr = failure(bibi(bare.path(), &["export"]));
    assert!(stderr.contains("no cita.toml found"), "{stderr}");

    let directory = tempfile::tempdir().unwrap();
    arxiv_library(directory.path());
    fs::write(directory.path().join("references.bib"), "drift\n").unwrap();
    // Export claims to hold the same entries as references.bib, so it must
    // refuse to run against drift rather than silently disagree with it.
    let stderr = failure(bibi(directory.path(), &["export"]));
    assert!(stderr.contains("run `bibi generate`"), "{stderr}");
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    assert!(!directory.path().join(format!("{name}.bib")).exists());
}
