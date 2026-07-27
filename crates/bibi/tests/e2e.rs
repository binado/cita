//! End-to-end test against the real INSPIRE API.
//!
//! Every other suite in this workspace is hermetic (INSPIRE traffic is served
//! by a local `TcpListener`, see `cli.rs` and `bibi-inspire-client/tests`).
//! This test is the one exception: it omits `BIBI_INSPIRE_BASE_URL` entirely,
//! so `bibi` falls back to `https://inspirehep.net/` and makes real requests
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

fn bibi(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bibi"))
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
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
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// An empty schema-1 project, written directly now that `init` is gone.
fn project(directory: &Path) {
    fs::write(directory.join("cita.toml"), "schema = 1\n").unwrap();
    fs::write(directory.join("references.bib"), "").unwrap();
}

/// Slice `cita.toml` text down to one `[references.<key>]` entry, including
/// any of its own nested subtables (e.g. `.identifiers`), so assertions about
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
/// record id (a `bibi add` locator, not the `inspirehep.net/literature/<id>`
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

#[test]
#[ignore = "hits the live INSPIRE API; run with `cargo test --test e2e -- --ignored`"]
fn e2e_seed_then_add_handpicked_inspire_papers() {
    let directory = tempfile::tempdir().unwrap();

    // 1. A fresh schema-1 manifest and an empty bibliography.
    project(directory.path());
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(manifest.starts_with("schema = 1"), "{manifest}");
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
    let output = success(bibi_stdin(directory.path(), &["import", "-"], seed));
    assert_eq!(output, "added SeedAlpha\nadded SeedBeta\n");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    let seed_alpha_before = section(&manifest, "SeedAlpha").to_owned();
    let seed_beta_before = section(&manifest, "SeedBeta").to_owned();
    assert!(
        seed_alpha_before.contains("source = \"import\""),
        "{manifest}"
    );
    assert!(
        seed_beta_before.contains("source = \"import\""),
        "{manifest}"
    );

    // 3. Progressively `add` each handpicked paper by its stable INSPIRE
    //    record id, checking the manifest after every command.
    for paper in &PAPERS {
        let locator = format!("inspire:{}", paper.record_id);
        let output = success(bibi(
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
        assert!(entry.contains("source = \"inspire\""), "{entry}");
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
    let output = success(bibi(
        directory.path(),
        &["add", "--key", "Maldacena", "inspire:451647"],
    ));
    assert_eq!(output, "skipped Maldacena\n");
    assert_eq!(
        fs::read(directory.path().join("cita.toml")).unwrap(),
        before
    );

    // 5. Every added paper is listed.
    let listed = success(bibi(directory.path(), &["list"]));
    for paper in &PAPERS {
        assert!(listed.contains(paper.key), "{listed}");
    }

    // 6. `sync` refreshes every INSPIRE-managed record by stable id and must
    //    leave the two imported seed entries byte-identical.
    let output = success(bibi(directory.path(), &["sync"]));
    assert!(
        output.contains("4 managed") && output.contains("2 imported"),
        "{output}"
    );
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert_eq!(section(&manifest, "SeedAlpha"), seed_alpha_before);
    assert_eq!(section(&manifest, "SeedBeta"), seed_beta_before);
    for paper in &PAPERS {
        let entry = section(&manifest, paper.key);
        assert!(
            entry.contains(&format!("record_id = {}", paper.record_id)),
            "{entry}"
        );
    }

    // 7. `remove` drops a managed record by provider id, not its local key.
    let output = success(bibi(directory.path(), &["remove", "inspire:51188"]));
    assert_eq!(output, "Removed Weinberg\n");
    let manifest = fs::read_to_string(directory.path().join("cita.toml")).unwrap();
    assert!(!manifest.contains("[references.Weinberg]"), "{manifest}");
    let bibliography = fs::read_to_string(directory.path().join("references.bib")).unwrap();
    assert!(
        !bibliography.contains("A Model of Leptons"),
        "{bibliography}"
    );
}
