//! The compiled binary: exit status, exact stdout, and files on disk.
//!
//! Provider-backed commands run against a local listener through the test-only
//! base-URL override, so the suite never depends on the real network.

mod support;

use support::{TestServer, hits, record};

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

/// Run `bibi` in a temporary project.
///
/// The working directory is the whole target: with no global manifest there is
/// nothing outside this directory a command could reach for.
fn bibi(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("running bibi")
}

fn bibi_with_stdin(directory: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning bibi");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("writing bibi stdin");
    child.wait_with_output().expect("waiting for bibi")
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
fn adding_a_file_writes_keys_to_stdout_and_a_summary_to_stderr() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    let output = bibi(&path, &["add", "-f", "library.bib"]);

    assert_eq!(code(&output), 0);
    // stdout is one local key per line, so the output can be piped into other commands.
    assert_eq!(stdout(&output), "astropy:2022\nnotes:2026\n");
    assert!(stderr(&output).contains("added 2"));
    assert!(path.join("bibi.toml").exists());
}

#[test]
fn empty_redirected_batches_are_successful_without_a_manifest() {
    for command in ["add", "fetch", "remove"] {
        let (_directory, path) = project();
        let output = bibi_with_stdin(&path, &[command], "\n \n");
        assert_eq!(code(&output), 0, "{command}: {}", stderr(&output));
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).is_empty());
        assert!(!path.join("bibi.toml").exists());
    }
}

#[cfg(unix)]
#[test]
fn omitted_interactive_inputs_are_clap_style_usage_errors() {
    use std::{fs::File, os::fd::FromRawFd};

    for command in ["add", "fetch", "remove", "show"] {
        let (_directory, path) = project();
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: `openpty` initializes both descriptors on success. Each is
        // then given exactly one owner below.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0, "opening pseudo-terminal");
        // SAFETY: `slave` is a fresh owned descriptor from `openpty`.
        let terminal_stdin = unsafe { File::from_raw_fd(slave) };
        // SAFETY: `master` is the other fresh descriptor and is not used again.
        unsafe {
            libc::close(master);
        }

        let output = Command::new(env!("CARGO_BIN_EXE_bibi"))
            .current_dir(&path)
            .arg(command)
            .stdin(Stdio::from(terminal_stdin))
            .output()
            .expect("running bibi with terminal stdin");
        assert_eq!(code(&output), 2, "{command}: {}", stderr(&output));
        assert!(stderr(&output).contains("Usage:"), "{command}");
        assert!(!path.join("bibi.toml").exists());
    }
}

#[test]
fn piped_remove_and_show_use_trimmed_nonblank_selectors() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let shown = bibi_with_stdin(&path, &["show"], "\n  notes:2026 \n");
    assert_eq!(code(&shown), 0, "{}", stderr(&shown));
    assert!(stdout(&shown).starts_with("@unpublished{notes:2026,"));

    let too_many = bibi_with_stdin(&path, &["show"], "notes:2026\nastropy:2022\n");
    assert_eq!(code(&too_many), 2);
    assert!(stderr(&too_many).contains("exactly one selector"));

    let removed = bibi_with_stdin(&path, &["remove"], " notes:2026\n\nastropy:2022 ");
    assert_eq!(code(&removed), 0, "{}", stderr(&removed));
    assert_eq!(stdout(&removed), "notes:2026\nastropy:2022\n");
}

#[test]
fn explicit_positionals_win_over_standard_input() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let shown = bibi_with_stdin(&path, &["show", "notes:2026"], "astropy:2022\nextra\n");
    assert_eq!(code(&shown), 0, "{}", stderr(&shown));
    assert!(stdout(&shown).starts_with("@unpublished{notes:2026,"));
}

#[test]
fn a_literal_dash_is_an_ordinary_selector() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    for command in ["show", "remove", "fetch"] {
        let output = bibi_with_stdin(&path, &[command, "-"], "notes:2026\n");
        assert_eq!(code(&output), 1, "{command}: {}", stderr(&output));
        assert!(stderr(&output).contains("no record matches"));
    }
}

#[test]
fn adding_the_same_file_twice_skips_and_still_exits_zero() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let output = bibi(&path, &["add", "-f", "library.bib"]);

    // The requested end state already holds, so a build script may proceed.
    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "astropy:2022\nnotes:2026\n");
    assert!(stderr(&output).contains("skipped"));
}

#[test]
fn listing_outputs_are_exactly_what_a_pipeline_expects() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let keys = bibi(&path, &["list", "--fields", "key"]);
    assert_eq!(stdout(&keys), "astropy:2022\nnotes:2026\n");

    let bibtex = bibi(&path, &["list", "--format", "bibtex"]);
    assert_eq!(stdout(&bibtex), LIBRARY.trim_end().to_owned() + "\n");

    let json = bibi(&path, &["list", "--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
    assert_eq!(parsed[0]["key"], "astropy:2022");
    assert!(parsed[0].get("bibtex").is_none());

    let filtered = bibi(&path, &["list", "--fields", "key", "--author", "roe"]);
    assert_eq!(stdout(&filtered), "notes:2026\n");
}

#[test]
fn the_default_listing_is_a_headed_table_and_never_styles_a_pipe() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let listed = bibi(&path, &["list"]);
    assert_eq!(code(&listed), 0);
    let rendered = stdout(&listed);
    for header in ["KEY", "AUTHOR", "YEAR", "ARXIV", "TITLE"] {
        assert!(
            rendered.contains(header),
            "{header} missing from {rendered}"
        );
    }
    assert!(rendered.contains("astropy:2022"), "{rendered}");
    assert!(rendered.contains("Price-Whelan"), "{rendered}");
    // Column widths are ambient here, but this is not: stdout is a pipe under
    // the test harness, so nothing may style it.
    assert!(!rendered.contains('\u{1b}'), "styling leaked into a pipe");
}

#[test]
fn fields_are_tab_separated_and_an_absent_value_keeps_its_column() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);
    // A local entry carries no arXiv id, so its derived columns are empty.
    std::fs::write(path.join("mine.bib"), "@misc{Mine,title={Mine}}\n").unwrap();
    bibi(&path, &["add", "-f", "mine.bib", "--force-local"]);

    let listed = bibi(&path, &["list", "--fields", "key,provider,arxiv,arxiv-url"]);
    assert_eq!(code(&listed), 0);
    assert_eq!(
        stdout(&listed),
        "Aad:2012tfa\tinspire\t1207.7214\thttps://arxiv.org/pdf/1207.7214\n\
         Mine\tlocal\t\t\n"
    );

    // A repeated field is a repeated column, and order is the order given.
    let reordered = bibi(&path, &["list", "--fields", "provider,key", "--local"]);
    assert_eq!(stdout(&reordered), "local\tMine\n");
}

/// A manifest naming a provider this build does not carry.
///
/// Hand-written rather than produced by `add`, because there is no way to make
/// this build create an `ads` record — which is the point: a manifest written
/// by a later build has to remain readable by this one.
const FOREIGN: &str = "schema = 1\n\n\
    [[records]]\n\
    id = \"7641d991-6a03-4795-b302-82b2c0cb3adc\"\n\
    key = \"Someone:2030abc\"\n\
    provider = \"ads\"\n\
    provider_id = \"2030ApJ...900..1X\"\n\
    title = \"A later build wrote this\"\n\
    year = 2030\n\
    bibtex = \"@article{Someone:2030abc,\\n  title = {A later build wrote this}\\n}\"\n";

#[test]
fn an_unknown_provider_names_the_ones_that_would_have_worked() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    // Wrong case is the common mistake, and the grammar that rejects it is not
    // the user's problem — the names that would work are.
    let shouted = bibi(&path, &["list", "--provider", "TEST"]);
    assert_eq!(code(&shouted), 1);
    assert_eq!(
        stderr(&shouted).trim_end(),
        "bibi: provider `TEST` not found. Known providers: inspire, local"
    );
    assert!(!stderr(&shouted).contains("[a-z]"), "no regex is shown");

    // A well-formed name nothing knows is the same mistake, not an empty list.
    let absent = bibi(&path, &["list", "--provider", "ads"]);
    assert_eq!(code(&absent), 1);
    assert!(stdout(&absent).is_empty());
    assert!(stderr(&absent).contains("provider `ads` not found"));

    // `check` shares the filter, so it shares the check.
    let checked = bibi(&path, &["check", "--provider", "ads"]);
    assert_eq!(code(&checked), 1);
    assert!(stderr(&checked).contains("provider `ads` not found"));
}

#[test]
fn a_provider_only_the_manifest_knows_is_still_filterable() {
    let (_directory, path) = project();
    std::fs::write(path.join("bibi.toml"), FOREIGN).unwrap();

    // This build carries no `ads` provider and cannot refresh the record, but
    // the record is here, so filtering to it is a question with an answer.
    let listed = bibi(&path, &["list", "--provider", "ads", "--fields", "key"]);
    assert_eq!(code(&listed), 0);
    assert_eq!(stdout(&listed), "Someone:2030abc\n");

    // Naming it where it must actually be called still fails, and says why.
    let synced = bibi(&path, &["sync", "--provider", "ads"]);
    assert_eq!(code(&synced), 1);
    assert_eq!(
        stderr(&synced).trim_end(),
        "bibi: provider `ads` not found. Installed providers: inspire, local"
    );

    // `add` refuses before it resolves anything: no base URL is configured
    // here, so reaching the network at all would hang or fail differently.
    let added = bibi(&path, &["add", "--provider", "ads", "1207.7214"]);
    assert_eq!(code(&added), 1);
    assert!(stderr(&added).contains("Installed providers: inspire, local"));
}

#[test]
fn fields_and_format_are_mutually_exclusive() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    // Clap owns this one: a format says how to encode, a field says what to
    // include, and asking for both at once has no answer.
    let both = bibi(&path, &["list", "--fields", "key", "--format", "json"]);
    assert_eq!(code(&both), 2);

    // The default format must not count as having been given.
    let alone = bibi(&path, &["list", "--fields", "key"]);
    assert_eq!(code(&alone), 0);

    let unknown = bibi(&path, &["list", "--fields", "titel"]);
    assert_eq!(code(&unknown), 2);
    assert!(stderr(&unknown).contains("possible values"));
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
fn rename_rewrites_only_the_key_and_remove_emits_its_key() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let renamed = bibi(&path, &["rename", "notes:2026", "Roe:2026"]);
    assert_eq!(code(&renamed), 0);
    assert!(stdout(&renamed).starts_with("@unpublished{Roe:2026,"));
    assert!(stderr(&renamed).contains("\\cite{}"));

    let removed = bibi(&path, &["remove", "Roe:2026"]);
    assert_eq!(code(&removed), 0);
    assert_eq!(stdout(&removed), "Roe:2026\n");
    assert_eq!(
        stdout(&bibi(&path, &["list", "--fields", "key"])),
        "astropy:2022\n"
    );
}

#[test]
fn removing_the_same_record_twice_is_a_successful_skip() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let removed = bibi(&path, &["remove", "notes:2026", "notes:2026"]);
    assert_eq!(code(&removed), 0);
    assert_eq!(stdout(&removed), "notes:2026\n");
    assert!(stderr(&removed).contains("already removed"));
    assert_eq!(
        stdout(&bibi(&path, &["list", "--fields", "key"])),
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
    assert_eq!(stdout(&output), "astropy:2022\n");
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
    let explicit = bibi(&nested, &["list", "--fields", "key", "-p", "../bibi.toml"]);
    assert_eq!(code(&explicit), 0);
    assert_eq!(stdout(&explicit), "astropy:2022\nnotes:2026\n");
}

#[test]
fn there_is_no_user_level_manifest_to_select() {
    let (_directory, path) = project();
    // `-g` was the one way to act on a manifest outside the working directory.
    // With it gone the flag is unknown, which clap reports as a usage error.
    let output = bibi(&path, &["init", "-g"]);
    assert_eq!(code(&output), 2);
    assert!(!path.join("bibi.toml").exists());
}

/// Run `bibi` with INSPIRE pointed at a local listener.
fn bibi_against(directory: &Path, server: &TestServer, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .env("BIBI_INSPIRE_BASE_URL", &server.base_url)
        .args(args)
        .output()
        .expect("running bibi")
}

fn bibi_against_with_stdin(
    directory: &Path,
    server: &TestServer,
    args: &[&str],
    input: &str,
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .env("BIBI_INSPIRE_BASE_URL", &server.base_url)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning bibi");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("writing bibi stdin");
    child.wait_with_output().expect("waiting for bibi")
}

#[test]
fn adding_a_locator_adopts_the_providers_texkey_and_stores_its_bibtex() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(
            1124337,
            "Aad:2012tfa",
            "1207.7214",
            "Observation of a new particle",
        )]),
        "@article{Aad:2012tfa,\n  title = {Observation of a new particle}\n}\n".to_owned(),
    ]);

    let output = bibi_against(&path, &server, &["add", "1207.7214"]);
    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "Aad:2012tfa\n");

    // The record is provider-owned, with a handle and a token to refresh by.
    let json = bibi(&path, &["list", "--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(parsed[0]["key"], "Aad:2012tfa");
    assert_eq!(parsed[0]["provider"], "inspire");
    assert_eq!(parsed[0]["provider_id"], "1124337");
    assert_eq!(parsed[0]["revision"], "2026-01-01T00:00:00+00:00");
    assert_eq!(parsed[0]["arxiv"], "1207.7214");
    assert_eq!(parsed[0]["collaborations"][0], "ATLAS");
    // Two requests: one structured search, one BibTeX search.
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn piped_add_reads_newline_delimited_locators() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);

    let output = bibi_against_with_stdin(&path, &server, &["add"], "\n 1207.7214 \n\n");
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), "Aad:2012tfa\n");
    assert!(path.join("bibi.toml").exists());
}

#[test]
fn a_locator_no_provider_holds_is_an_item_failure() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![hits(&[])]);
    let output = bibi_against(&path, &server, &["add", "2401.99999"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no provider holds a record"));
    assert!(!path.join("bibi.toml").exists());
}

#[test]
fn an_imported_entry_a_provider_holds_is_upgraded_but_keeps_its_key() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(
            1124337,
            "Aad:2012tfa",
            "1207.7214",
            "Observation of a new particle",
        )]),
        "@article{Aad:2012tfa,\n  title = {Provider formatting}\n}\n".to_owned(),
    ]);
    std::fs::write(
        path.join("colleague.bib"),
        "@article{TheirKey:2012,\n  title = {Their formatting},\n  eprint = {1207.7214}\n}\n",
    )
    .unwrap();

    let output = bibi_against(&path, &server, &["add", "-f", "colleague.bib"]);
    assert_eq!(code(&output), 0);
    // The colleague's citation key survives; the provider's bytes replace theirs.
    assert_eq!(stdout(&output), "TheirKey:2012\n");
    let manifest = std::fs::read_to_string(path.join("bibi.toml")).unwrap();
    assert!(manifest.contains("provider = \"inspire\""));
    assert!(manifest.contains("@article{Aad:2012tfa,"));
}

#[test]
fn structural_flag_conflicts_are_clap_usage_errors() {
    let (_directory, path) = project();
    for args in [
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

/// What a shell does with `bibi list --format bibtex > <name>`.
///
/// bibi writes no bibliography itself, so every materialized `.bib` in these
/// tests is produced the way a user produces one: by redirecting stdout.
fn render_to(path: &Path, name: &str, filters: &[&str]) -> Output {
    let mut args = vec!["list", "--format", "bibtex"];
    args.extend_from_slice(filters);
    let output = bibi(path, &args);
    std::fs::write(path.join(name), stdout(&output)).unwrap();
    output
}

#[test]
fn a_rendered_bibliography_is_what_check_verifies() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    // A bibliography exists only once a shell writes one: adding wrote no .bib,
    // and neither does anything else bibi offers.
    assert!(!path.join("references.bib").exists());

    let rendered = render_to(&path, "references.bib", &[]);
    assert_eq!(code(&rendered), 0);
    assert_eq!(
        std::fs::read_to_string(path.join("references.bib")).unwrap(),
        LIBRARY.trim_end().to_owned() + "\n"
    );

    let matched = bibi(&path, &["check"]);
    assert_eq!(code(&matched), 0);
    assert!(stderr(&matched).contains("matches the manifest"));

    // Drift is a nonzero exit, and check repairs nothing.
    std::fs::write(
        path.join("references.bib"),
        "@misc{edited,title={By hand}}\n",
    )
    .unwrap();
    let drifted = bibi(&path, &["check", "--diff"]);
    assert_eq!(code(&drifted), 1);
    assert!(stderr(&drifted).contains("drifted"));
    assert!(stderr(&drifted).contains("first difference at line 1"));
    assert_eq!(
        std::fs::read_to_string(path.join("references.bib")).unwrap(),
        "@misc{edited,title={By hand}}\n"
    );

    // A bibliography that is not there is missing, not drift.
    std::fs::remove_file(path.join("references.bib")).unwrap();
    let missing = bibi(&path, &["check"]);
    assert_eq!(code(&missing), 1);
    assert!(stderr(&missing).contains("no bibliography at"));
}

#[test]
fn there_is_no_command_that_writes_a_bibliography() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let before = std::fs::read_to_string(path.join("bibi.toml")).unwrap();

    // `export` chose a destination, which is the shell's job now.
    let output = bibi(&path, &["export"]);
    assert_eq!(code(&output), 2);
    assert!(!path.join("references.bib").exists());
    // And with no destination to choose, nothing can be aimed at the manifest.
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        before
    );
}

#[test]
fn filters_select_what_is_rendered_and_what_is_checked() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);

    let output = render_to(&path, "subset.bib", &["--year", "2026"]);
    assert_eq!(code(&output), 0);
    let written = std::fs::read_to_string(path.join("subset.bib")).unwrap();
    assert!(written.contains("notes:2026"));
    assert!(!written.contains("astropy:2022"));

    // The same options make the check agree; without them it drifts.
    assert_eq!(
        code(&bibi(&path, &["check", "subset.bib", "--year", "2026"])),
        0
    );
    assert_eq!(code(&bibi(&path, &["check", "subset.bib"])), 1);
}

#[test]
fn rendering_and_check_honor_local() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);
    std::fs::write(path.join("mine.bib"), "@misc{Mine,title={Mine}}\n").unwrap();
    bibi(&path, &["add", "-f", "mine.bib", "--force-local"]);

    let listed = bibi(&path, &["list", "--fields", "key", "--local"]);
    assert_eq!(code(&listed), 0);
    assert_eq!(stdout(&listed), "Mine\n");

    let rendered = render_to(&path, "local.bib", &["--local"]);
    assert_eq!(code(&rendered), 0);
    let written = std::fs::read_to_string(path.join("local.bib")).unwrap();
    assert!(written.contains("@misc{Mine"));
    assert!(!written.contains("Aad:2012tfa"));

    assert_eq!(code(&bibi(&path, &["check", "local.bib", "--local"])), 0);
    assert_eq!(code(&bibi(&path, &["check", "local.bib"])), 1);
}

#[test]
fn check_takes_a_provider_filter_like_list_does() {
    let (_directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);
    std::fs::write(path.join("mine.bib"), "@misc{Mine,title={Mine}}\n").unwrap();
    bibi(&path, &["add", "-f", "mine.bib", "--force-local"]);

    render_to(&path, "inspire.bib", &["--provider", "inspire"]);
    // `--provider` was excluded from the rendering filters only because on
    // `export` it named the provider to sync. It is a plain filter now.
    assert_eq!(
        code(&bibi(
            &path,
            &["check", "inspire.bib", "--provider", "inspire"]
        )),
        0
    );
    assert_eq!(code(&bibi(&path, &["check", "inspire.bib"])), 1);
}

#[test]
fn a_sync_with_no_managed_records_reports_and_writes_nothing() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let before = std::fs::read_to_string(path.join("bibi.toml")).unwrap();

    // Both records are local, so nothing is refreshable and no request is made.
    let output = bibi(&path, &["sync"]);
    assert_eq!(code(&output), 0);
    assert!(stderr(&output).contains("2 unrefreshable"));
    assert_eq!(
        std::fs::read_to_string(path.join("bibi.toml")).unwrap(),
        before
    );
}

/// Add one INSPIRE-owned record carrying an arXiv identifier.
fn project_with_an_arxiv_record() -> (tempfile::TempDir, PathBuf) {
    let (directory, path) = project();
    let server = TestServer::new(vec![
        hits(&[record(1124337, "Aad:2012tfa", "1207.7214", "Observation")]),
        "@article{Aad:2012tfa,\n  title = {Observation}\n}\n".to_owned(),
    ]);
    bibi_against(&path, &server, &["add", "1207.7214"]);
    (directory, path)
}

fn project_with_two_arxiv_records() -> (tempfile::TempDir, PathBuf) {
    let (directory, path) = project();
    std::fs::write(
        path.join("arxiv.bib"),
        "@article{First,title={First},eprint={1207.7214}}\n\n\
         @article{Second,title={Second},eprint={2401.00001}}\n",
    )
    .unwrap();
    let added = bibi(&path, &["add", "-f", "arxiv.bib", "--force-local"]);
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    (directory, path)
}

/// Run `bibi` with arXiv pointed at a local listener serving one PDF.
fn bibi_fetching(directory: &Path, body: &[u8], args: &[&str]) -> Output {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binding a test server");
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let body = body.to_vec();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        use std::io::{BufRead, Write};
        while reader.read_line(&mut line).is_ok() {
            if line.trim().is_empty() || line.is_empty() {
                break;
            }
            line.clear();
        }
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        let _ = stream.write_all(&response);
        let _ = stream.flush();
    });
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(directory)
        .env("BIBI_ARXIV_BASE_URL", &base_url)
        .args(args)
        .output()
        .expect("running bibi")
}

#[test]
fn fetch_reports_a_url_without_downloading_anything() {
    let (_directory, path) = project_with_an_arxiv_record();

    let output = bibi(&path, &["fetch", "Aad:2012tfa", "--url"]);
    assert_eq!(code(&output), 0);
    // One line, so `open $(bibi fetch <selector> --url)` works.
    assert_eq!(stdout(&output), "https://arxiv.org/pdf/1207.7214\n");
    // A question, not a download: nothing landed in the working directory.
    assert!(!path.join("1207.7214.pdf").exists());

    // `--source` asks the same question about the other artifact.
    let source = bibi(&path, &["fetch", "Aad:2012tfa", "--url", "--source"]);
    assert_eq!(code(&source), 0);
    assert_eq!(stdout(&source), "https://arxiv.org/e-print/1207.7214\n");
}

#[test]
fn piped_fetch_accepts_a_selector_batch() {
    let (_directory, path) = project_with_an_arxiv_record();

    let output = bibi_with_stdin(&path, &["fetch", "--url"], "\n Aad:2012tfa\n1207.7214 \n");
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), "https://arxiv.org/pdf/1207.7214\n");
    assert!(stderr(&output).contains("same artifact as earlier selector"));
}

#[test]
fn a_fetch_url_batch_preserves_success_order_across_partial_failures() {
    let (_directory, path) = project_with_two_arxiv_records();

    let output = bibi(&path, &["fetch", "Second", "missing", "First", "--url"]);
    assert_eq!(code(&output), 1);
    assert_eq!(
        stdout(&output),
        "https://arxiv.org/pdf/2401.00001\nhttps://arxiv.org/pdf/1207.7214\n"
    );
    assert!(stderr(&output).contains("`missing`"));
}

#[test]
fn several_fetches_reject_a_non_directory_output_before_downloading() {
    let (_directory, path) = project_with_two_arxiv_records();

    let output = bibi(&path, &["fetch", "First", "Second", "-o", "combined.pdf"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("must be an existing directory"));
    assert!(!path.join("combined.pdf").exists());
    assert!(!path.join("1207.7214.pdf").exists());
    assert!(!path.join("2401.00001.pdf").exists());
}

#[test]
fn a_fetch_downloads_into_the_working_directory_under_the_arxiv_name() {
    let (_directory, path) = project_with_an_arxiv_record();

    let output = bibi_fetching(&path, b"%PDF-1.7\nbody", &["fetch", "Aad:2012tfa"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let destination = path.join("1207.7214.pdf");
    assert!(destination.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"%PDF-1.7\nbody");
    // stdout is the path alone, so `open $(bibi fetch k)` works. It is compared
    // by suffix because the working directory may canonicalize (on macOS
    // `/var` is a symlink to `/private/var`).
    let reported = stdout(&output);
    assert_eq!(reported.lines().count(), 1, "the result is one path");
    assert!(
        reported.trim_end().ends_with("/1207.7214.pdf"),
        "unexpected path {reported:?}"
    );
}

#[test]
fn a_fetch_onto_an_existing_file_is_a_collision_rather_than_a_replacement() {
    let (_directory, path) = project_with_an_arxiv_record();
    let destination = path.join("1207.7214.pdf");
    std::fs::write(&destination, "mine").unwrap();

    let output = bibi_fetching(&path, b"%PDF-1.7\nnew", &["fetch", "Aad:2012tfa"]);
    // The user asked for a download and did not get one, so this fails.
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("already exists"));
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "mine");
}

#[test]
fn an_output_path_names_an_exact_file() {
    let (_directory, path) = project_with_an_arxiv_record();

    let output = bibi_fetching(
        &path,
        b"%PDF-1.7\nbody",
        &["fetch", "Aad:2012tfa", "-o", "paper.pdf"],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(path.join("paper.pdf").exists());
    assert!(!path.join("1207.7214.pdf").exists());
}

#[test]
fn a_response_that_is_not_the_artifact_leaves_nothing_behind() {
    let (_directory, path) = project_with_an_arxiv_record();

    // arXiv serves an HTML holding page for a withdrawn work.
    let output = bibi_fetching(&path, b"<!DOCTYPE html>", &["fetch", "Aad:2012tfa"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("not a PDF"));
    assert!(!path.join("1207.7214.pdf").exists());
    // No temporary sibling survived the refusal.
    assert_eq!(
        std::fs::read_dir(&path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name() != "bibi.toml")
            .count(),
        0
    );
}

#[test]
fn fetching_a_record_without_an_arxiv_id_explains_why_it_cannot() {
    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let output = bibi(&path, &["fetch", "notes:2026", "--url"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no arXiv identifier"));
}

#[test]
fn a_forced_fetch_replaces_an_existing_file() {
    let (_directory, path) = project_with_an_arxiv_record();
    let destination = path.join("1207.7214.pdf");
    std::fs::write(&destination, "mine").unwrap();

    let output = bibi_fetching(
        &path,
        b"%PDF-1.7\nnew",
        &["fetch", "Aad:2012tfa", "--force"],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(std::fs::read(&destination).unwrap(), b"%PDF-1.7\nnew");
}

#[test]
fn there_is_no_document_cache_to_maintain() {
    let (_directory, path) = project();
    // `cache clean` managed a global collection, which is a library concern.
    assert_eq!(code(&bibi(&path, &["cache", "clean", "--all"])), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn a_non_broken_stdout_error_exits_nonzero() {
    use std::{fs::OpenOptions, process::Stdio};

    let (_directory, path) = project();
    std::fs::write(path.join("library.bib"), LIBRARY).unwrap();
    bibi(&path, &["add", "-f", "library.bib"]);
    let sink = OpenOptions::new().write(true).open("/dev/full").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(&path)
        .env("BIBI_CACHE_ROOT", path.join("cache/bibi"))
        .args(["show", "notes:2026"])
        .stdout(Stdio::from(sink))
        .output()
        .expect("running bibi");

    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("writing stdout"));
}

#[cfg(unix)]
#[test]
fn a_closed_stdout_pipe_still_exits_zero() {
    use std::process::Stdio;

    let (_directory, path) = project();
    let large = format!("@misc{{Large,title={{{}}}}}\n", "x".repeat(1_000_000));
    std::fs::write(path.join("large.bib"), large).unwrap();
    bibi(&path, &["add", "-f", "large.bib", "--force-local"]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(&path)
        .env("BIBI_CACHE_ROOT", path.join("cache/bibi"))
        .args(["show", "Large"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running bibi");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("waiting for bibi");

    assert_eq!(code(&output), 0);
}
