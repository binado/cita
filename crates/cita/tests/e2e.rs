//! End-to-end test against the real INSPIRE API.
//!
//! Every other suite in this workspace is hermetic (INSPIRE traffic is served
//! by a local `TcpListener`, see `cli.rs` and `cita-inspire-client/tests`).
//! This test is the one exception: it omits `CITA_INSPIRE_BASE_URL` entirely,
//! so `cita` falls back to `https://inspirehep.net/` and makes real requests
//! for a handful of handpicked papers. It is `#[ignore]`d so `cargo test
//! --workspace` never touches the network; run it explicitly with:
//!
//!     cargo test --test e2e -- --ignored

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn cita(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
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

fn git(directory: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap()
}

fn git_success(directory: &Path, args: &[&str]) -> String {
    let output = git(directory, args);
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn init_git(directory: &Path) {
    git_success(directory, &["init", "-q"]);
    git_success(directory, &["config", "user.email", "cita@example.test"]);
    git_success(directory, &["config", "user.name", "cita Test"]);
}

/// Slice `cita.toml` text down to one `[references.<key>]` entry, including
/// any of its own nested subtables (e.g. `.inspire`), so assertions about
/// one paper cannot accidentally match a sibling entry or the next section.
fn section<'a>(manifest: &'a str, key: &str) -> &'a str {
    let header = format!("[references.{key}]");
    let start = manifest
        .find(&header)
        .unwrap_or_else(|| panic!("{header} missing from:\n{manifest}"));
    let own_subtable = format!("\n[references.{key}.");
    // Walk every following `[references.…]` header; skip this entry's own
    // nested subtables and stop at the first sibling entry (or end of file).
    let end = manifest[start..]
        .match_indices("\n[references.")
        .map(|(offset, _)| start + offset)
        .find(|&pos| !manifest[pos..].starts_with(&own_subtable))
        .unwrap_or(manifest.len());
    &manifest[start..end]
}

struct Paper {
    key: &'static str,
    record_id: u64,
    title: &'static str,
    arxiv: Option<&'static str>,
}

/// The papers handpicked in `E2E_TEST.md`, addressed by their stable INSPIRE
/// record id (a `cita add` locator, not the `inspirehep.net/literature/<id>`
/// URL they're documented as).
const PAPERS: [Paper; 4] = [
    // Legacy arXiv identifier (has a v2 on arXiv), math in the title.
    Paper {
        key: "Maldacena",
        record_id: 451647,
        title: "Large $N$ limit of superconformal field theories",
        arxiv: Some("hep-th/9711200"),
    },
    // No arXiv preprint.
    Paper {
        key: "Choptuik",
        record_id: 33714,
        title: "Universality and scaling in gravitational collapse",
        arxiv: None,
    },
    // Collaboration paper with 1000+ authors.
    Paper {
        key: "Ligo",
        record_id: 1421100,
        title: "Observation of Gravitational Waves from a Binary Black Hole Merger",
        arxiv: Some("1602.03837"),
    },
    // No arXiv preprint.
    Paper {
        key: "Weinberg",
        record_id: 51188,
        title: "A Model of Leptons",
        arxiv: None,
    },
];

/// Insert user-owned lines at the top of one entry's table, the way an editor
/// session would. Tags and notes never change the stored BibTeX, so the
/// generated bibliography stays in sync without regenerating it.
fn annotate(directory: &Path, key: &str, lines: &str) {
    let path = directory.join("cita.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    let header = format!("[references.{key}]\n");
    let at = manifest.find(&header).unwrap() + header.len();
    let (head, tail) = manifest.split_at(at);
    fs::write(&path, format!("{head}{lines}{tail}")).unwrap();
}

#[test]
#[ignore = "hits the live INSPIRE API; run with `cargo test --test e2e -- --ignored`"]
fn e2e_seed_then_add_handpicked_inspire_papers() {
    let directory = tempfile::tempdir().unwrap();
    init_git(directory.path());

    // 1. `init` creates a fresh schema-2 manifest and an empty bibliography.
    assert!(success(cita(directory.path(), &["init"])).contains("Initialized"));
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.starts_with("schema = 2"), "{manifest}");
    assert_eq!(
        fs::read_to_string(directory.path().join("references.bib")).unwrap(),
        ""
    );

    // 2. Seed a pre-existing library through `import`, simulating a repo that
    //    already tracks standalone BibTeX before adopting INSPIRE sync. Fed
    //    over stdin rather than a fixture file, since this workspace's
    //    `.gitignore` blanket-ignores `*.bib` (only `references.bib` is
    //    excepted) and every other suite avoids that friction the same way.
    let seed = concat!(
        "@misc{SeedAlpha,\n",
        "  title = {A seeded import entry},\n",
        "  author = {Doe, Jane},\n",
        "  year = {2020}\n",
        "}\n",
        "@misc{SeedBeta,\n",
        "  title = {Another seeded import entry},\n",
        "  doi = {10.1000/e2e.seed}\n",
        "}\n",
    );
    let output = success(cita_stdin(directory.path(), &["import", "-"], seed));
    assert_eq!(output, "added SeedAlpha\nadded SeedBeta\n");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    let seed_alpha_before = section(&manifest, "SeedAlpha").to_owned();
    let seed_beta_before = section(&manifest, "SeedBeta").to_owned();
    // An import is unmanaged: no provider sub-table, so `cita sync` never
    // touches it.
    assert!(!seed_alpha_before.contains("inspire"), "{manifest}");
    assert!(!seed_beta_before.contains("inspire"), "{manifest}");
    assert!(seed_alpha_before.contains("type = \"misc\""), "{manifest}");

    // 3. Progressively `add` each handpicked paper by its stable INSPIRE
    //    record id, checking the manifest after every command.
    for paper in &PAPERS {
        let locator = format!("inspire:{}", paper.record_id);
        let output = success(cita(
            directory.path(),
            &["add", "--key", paper.key, &locator],
        ));
        assert_eq!(output, format!("added {}\n", paper.key));

        let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
        let entry = section(&manifest, paper.key);
        assert!(
            entry.contains(&format!("record_id = {}", paper.record_id)),
            "{entry}"
        );
        // Provenance is the presence of the provider sub-table.
        assert!(
            entry.contains(&format!("[references.{}.inspire]", paper.key)),
            "{entry}"
        );
        assert!(entry.contains(paper.title), "{entry}");
        match paper.arxiv {
            Some(id) => assert!(entry.contains(&format!("arxiv = \"{id}\"")), "{entry}"),
            None => assert!(!entry.contains("arxiv = "), "{entry}"),
        }
    }

    // 4. Re-adding an already-managed record is idempotent: same outcome,
    //    byte-identical manifest (INSPIRE record identity wins over the
    //    request, regardless of the key it was requested under).
    let before = fs::read(directory.path().join("cita.toml")).unwrap();
    let output = success(cita(
        directory.path(),
        &["add", "--key", "Maldacena", "inspire:451647"],
    ));
    assert_eq!(output, "skipped Maldacena\n");
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        before
    );

    // 5. `generate` must be a no-op here: `add` already left references.bib
    //    in sync, so re-rendering it byte-for-byte proves no drift.
    let bib_before = fs::read(directory.path().join("references.bib")).unwrap();
    assert!(success(cita(directory.path(), &["generate"])).starts_with("Generated"));
    assert_eq!(
        fs::read(directory.path().join("references.bib")).unwrap(),
        bib_before
    );
    let listed = success(cita(directory.path(), &["list"]));
    for paper in &PAPERS {
        assert!(listed.contains(paper.key), "{listed}");
    }

    // 6. `sync` refreshes every INSPIRE-managed record by stable id. It must
    //    leave the two imported seed entries byte-identical, and must carry
    //    user-owned tags and notes on a managed entry through untouched.
    annotate(
        directory.path(),
        "Ligo",
        "tags = [\"gw\", \"reading-list\"]\nnotes = [\"Check the noise budget.\"]\n",
    );
    let output = success(cita(directory.path(), &["sync"]));
    assert!(
        output.contains("4 managed") && output.contains("2 unmanaged"),
        "{output}"
    );
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert_eq!(section(&manifest, "SeedAlpha"), seed_alpha_before);
    assert_eq!(section(&manifest, "SeedBeta"), seed_beta_before);
    let ligo = section(&manifest, "Ligo");
    assert!(ligo.contains("\"reading-list\""), "{ligo}");
    assert!(ligo.contains("Check the noise budget."), "{ligo}");
    assert!(
        success(cita(directory.path(), &["list", "--tag", "gw"])).contains("Ligo"),
        "{manifest}"
    );
    for paper in &PAPERS {
        let entry = section(&manifest, paper.key);
        assert!(
            entry.contains(&format!("record_id = {}", paper.record_id)),
            "{entry}"
        );
    }

    // 7. `remove` drops a managed record by provider id (not its local key),
    //    and `commit` stages only the two generated artifacts.
    let output = success(cita(directory.path(), &["remove", "inspire:51188"]));
    assert_eq!(output, "Removed Weinberg\n");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(!manifest.contains("[references.Weinberg]"), "{manifest}");
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(
        !bibliography.contains("A Model of Leptons"),
        "{bibliography}"
    );

    success(cita(directory.path(), &["commit"]));
    let committed = git_success(
        directory.path(),
        &["show", "--pretty=format:", "--name-only", "HEAD"],
    );
    assert!(committed.contains("cita.toml"), "{committed}");
    assert!(committed.contains("references.bib"), "{committed}");
}
