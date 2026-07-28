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

/// An empty bibliography, which is all a bibi project is.
fn bib(directory: &Path) {
    fs::write(directory.join("references.bib"), "").unwrap();
}

fn read_bib(directory: &Path) -> String {
    fs::read_to_string(directory.join("references.bib")).unwrap()
}

/// Slice the bibliography down to the one entry keyed `key`, so assertions
/// about one paper cannot accidentally match a neighbour.
fn section<'a>(bibliography: &'a str, key: &str) -> &'a str {
    let header = format!("{{{key},");
    let start = bibliography
        .find(&header)
        .and_then(|at| bibliography[..at].rfind('@'))
        .unwrap_or_else(|| panic!("{key} missing from:\n{bibliography}"));
    let end = bibliography[start..]
        .find("\n@")
        .map_or(bibliography.len(), |offset| start + offset);
    &bibliography[start..end]
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

    // 1. An empty bibliography is the whole of a fresh project.
    bib(directory.path());
    assert_eq!(read_bib(directory.path()), "");

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
    // An import prints the resulting bibliography, which is exactly the file.
    let output = success(bibi_stdin(directory.path(), &["import", "-"], seed));
    assert_eq!(output, read_bib(directory.path()));
    assert!(output.contains("{SeedAlpha,"), "{output}");
    assert!(output.contains("{SeedBeta,"), "{output}");
    let bibliography = read_bib(directory.path());
    let seed_alpha_before = section(&bibliography, "SeedAlpha").to_owned();
    let seed_beta_before = section(&bibliography, "SeedBeta").to_owned();
    // Imported entries carry no bibi bookkeeping, which is exactly what makes
    // them unmanaged.
    assert!(
        !seed_alpha_before.contains("x-bibi-"),
        "{seed_alpha_before}"
    );
    assert!(!seed_beta_before.contains("x-bibi-"), "{seed_beta_before}");

    // 3. Progressively `add` each handpicked paper by its stable INSPIRE
    //    record id, checking the bibliography after every command.
    for paper in &PAPERS {
        let locator = format!("inspire:{}", paper.record_id);
        // An add prints the stored entry's BibTeX.
        let output = success(bibi(
            directory.path(),
            &["add", "--key", paper.key, &locator],
        ));
        assert!(output.contains(&format!("{{{},", paper.key)), "{output}");
        assert!(
            output.contains(&format!("x-bibi-inspire-id = {{{}}}", paper.record_id)),
            "{output}"
        );

        let bibliography = read_bib(directory.path());
        let entry = section(&bibliography, paper.key);
        assert!(
            entry.contains(&format!("x-bibi-inspire-id = {{{}}}", paper.record_id)),
            "{entry}"
        );
        assert!(entry.contains("x-bibi-inspire-updated"), "{entry}");
        assert!(entry.contains(paper.title), "{entry}");
        match paper.arxiv {
            Some(id) => assert!(
                entry.contains(&format!("x-bibi-arxiv = {{{id}}}")),
                "{entry}"
            ),
            None => assert!(!entry.contains("x-bibi-arxiv"), "{entry}"),
        }
    }

    // 4. Re-adding an already-managed record is idempotent: same outcome,
    //    byte-identical bibliography (INSPIRE record identity wins over the
    //    request, regardless of the key it was requested under).
    let before = read_bib(directory.path());
    // Re-adding prints the kept entry again and warns about the skip on stderr.
    let (stdout, stderr) = success_streams(bibi(
        directory.path(),
        &["add", "--key", "Maldacena", "inspire:451647"],
    ));
    assert!(stdout.contains("{Maldacena,"), "{stdout}");
    assert!(stderr.contains("skipped Maldacena"), "{stderr}");
    assert_eq!(read_bib(directory.path()), before);

    // 5. Every added paper is listed.
    let listed = success(bibi(directory.path(), &["list"]));
    for paper in &PAPERS {
        assert!(listed.contains(paper.key), "{listed}");
    }

    // 6. `sync` refreshes every INSPIRE-managed record by stable id and must
    //    leave the two imported seed entries byte-identical. Nothing changed
    //    upstream since step 3, so the timestamps match and nothing is written.
    let before = read_bib(directory.path());
    let output = success(bibi(directory.path(), &["sync"]));
    assert!(
        output.contains("refreshed 0 of 4 managed entries"),
        "{output}"
    );
    assert!(output.contains("2 entries not on INSPIRE"), "{output}");
    assert_eq!(read_bib(directory.path()), before);
    let bibliography = read_bib(directory.path());
    assert_eq!(section(&bibliography, "SeedAlpha"), seed_alpha_before);
    assert_eq!(section(&bibliography, "SeedBeta"), seed_beta_before);

    // 7. `check` accepts what bibi itself wrote.
    assert!(success(bibi(directory.path(), &["check"])).contains("is valid: 6 references"));

    // 8. `remove` drops a managed record by provider id, not its local key.
    let output = success(bibi(directory.path(), &["remove", "inspire:51188"]));
    assert_eq!(output, "Removed Weinberg\n");
    let bibliography = read_bib(directory.path());
    assert!(!bibliography.contains("{Weinberg,"), "{bibliography}");
    assert!(
        !bibliography.contains("A Model of Leptons"),
        "{bibliography}"
    );

    // 9. `export` hands out a copy with bibi's bookkeeping taken back out.
    let exported = success(bibi(directory.path(), &["export", "-o", "-"]));
    assert!(!exported.contains("x-bibi-"), "{exported}");
    assert!(exported.contains("Large $N$ limit"), "{exported}");
}
