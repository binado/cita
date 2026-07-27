//! End-to-end test against the real INSPIRE API.
//!
//! Every other suite is hermetic. Run this ignored liveness test explicitly:
//!
//!     cargo test --test e2e -- --ignored

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn cita(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("CITA_HOME", home)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn cita_stdin(home: &Path, cwd: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cita"))
        .current_dir(cwd)
        .args(args)
        .env("CITA_HOME", home)
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

fn section<'a>(manifest: &'a str, key: &str) -> &'a str {
    let header = format!("[references.{key}]");
    let start = manifest
        .find(&header)
        .unwrap_or_else(|| panic!("{header} missing from:\n{manifest}"));
    let own_subtable = format!("\n[references.{key}.");
    let end = manifest[start..]
        .match_indices("\n[references.")
        .map(|(offset, _)| start + offset)
        .find(|&position| !manifest[position..].starts_with(&own_subtable))
        .unwrap_or(manifest.len());
    &manifest[start..end]
}

struct Paper {
    key: &'static str,
    record_id: u64,
    title: &'static str,
    arxiv: Option<&'static str>,
}

const PAPERS: [Paper; 4] = [
    Paper {
        key: "Maldacena",
        record_id: 451647,
        title: "Large $N$ limit of superconformal field theories",
        arxiv: Some("hep-th/9711200"),
    },
    Paper {
        key: "Choptuik",
        record_id: 33714,
        title: "Universality and scaling in gravitational collapse",
        arxiv: None,
    },
    Paper {
        key: "Ligo",
        record_id: 1421100,
        title: "Observation of Gravitational Waves from a Binary Black Hole Merger",
        arxiv: Some("1602.03837"),
    },
    Paper {
        key: "Weinberg",
        record_id: 51188,
        title: "A Model of Leptons",
        arxiv: None,
    },
];

#[test]
#[ignore = "hits the live INSPIRE API; run with `cargo test --test e2e -- --ignored`"]
fn global_store_seed_add_sync_export_and_remove() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("cita-home");
    let work = directory.path().join("work");
    fs::create_dir(&work).unwrap();

    assert!(success(cita(&home, &work, &["init"])).contains("Initialized"));
    let manifest_path = home.join("shelves/main/shelf.toml");
    assert!(
        fs::read_to_string(&manifest_path)
            .unwrap()
            .starts_with("schema = 1")
    );

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
    assert_eq!(
        success(cita_stdin(&home, &work, &["import", "-"], seed)),
        "added SeedAlpha\nadded SeedBeta\n"
    );
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let seed_alpha_before = section(&manifest, "SeedAlpha").to_owned();
    let seed_beta_before = section(&manifest, "SeedBeta").to_owned();

    for paper in &PAPERS {
        let locator = format!("inspire:{}", paper.record_id);
        assert_eq!(
            success(cita(&home, &work, &["add", "--key", paper.key, &locator],)),
            format!("added {}\n", paper.key)
        );
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let entry = section(&manifest, paper.key);
        assert!(entry.contains(&format!("record_id = {}", paper.record_id)));
        assert!(entry.contains(paper.title));
        match paper.arxiv {
            Some(id) => assert!(entry.contains(&format!("arxiv = \"{id}\""))),
            None => assert!(!entry.contains("arxiv = ")),
        }
    }

    let listed = success(cita(&home, &work, &["list"]));
    for paper in &PAPERS {
        assert!(listed.contains(paper.key), "{listed}");
    }
    success(cita(&home, &work, &["export", "references.bib"]));
    let bibliography = fs::read_to_string(work.join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{Maldacena,"));

    let output = success(cita(&home, &work, &["sync"]));
    assert!(
        output.contains("4 managed") && output.contains("2 imported"),
        "{output}"
    );
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(section(&manifest, "SeedAlpha"), seed_alpha_before);
    assert_eq!(section(&manifest, "SeedBeta"), seed_beta_before);

    assert_eq!(
        success(cita(&home, &work, &["remove", "inspire:51188"])),
        "Removed Weinberg\n"
    );
    assert!(
        !fs::read_to_string(manifest_path)
            .unwrap()
            .contains("[references.Weinberg]")
    );
}
