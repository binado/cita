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

struct Paper {
    key: &'static str,
    record_id: u64,
    title: &'static str,
}

const PAPERS: [Paper; 4] = [
    Paper {
        key: "Maldacena",
        record_id: 451647,
        title: "Large $N$ limit of superconformal field theories",
    },
    Paper {
        key: "Choptuik",
        record_id: 33714,
        title: "Universality and scaling in gravitational collapse",
    },
    Paper {
        key: "Ligo",
        record_id: 1421100,
        title: "Observation of Gravitational Waves from a Binary Black Hole Merger",
    },
    Paper {
        key: "Weinberg",
        record_id: 51188,
        title: "A Model of Leptons",
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
    assert!(home.join("library.sqlite3").is_file());

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
    for paper in &PAPERS {
        let locator = format!("inspire:{}", paper.record_id);
        assert_eq!(
            success(cita(&home, &work, &["add", "--key", paper.key, &locator],)),
            format!("added {}\n", paper.key)
        );
    }

    let listed = success(cita(&home, &work, &["list"]));
    for paper in &PAPERS {
        assert!(
            listed.contains(paper.key) && listed.contains(paper.title),
            "{listed}"
        );
    }
    success(cita(&home, &work, &["export", "references.bib"]));
    let bibliography = fs::read_to_string(work.join("references.bib")).unwrap();
    assert!(bibliography.contains("@misc{Maldacena,"));

    let output = success(cita(&home, &work, &["sync"]));
    assert!(
        output.contains("4 managed") && output.contains("2 imported"),
        "{output}"
    );
    assert_eq!(
        success(cita(&home, &work, &["remove", "inspire:51188"])),
        "Removed Weinberg\n"
    );
    assert!(!success(cita(&home, &work, &["list"])).contains("Weinberg"));
}
