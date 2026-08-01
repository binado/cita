use std::{
    io::Write,
    process::{Command, Stdio},
};

fn bibi(directory: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bibi"));
    command.current_dir(directory);
    command
}

#[test]
fn import_file_list_show_and_remove_form_a_pipeable_local_workflow() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("local.bib"),
        "@misc{LocalKey, title={Local title}, eprint={1207.7214}}",
    )
    .unwrap();

    let imported = bibi(directory.path())
        .args(["import", "local.bib"])
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&imported.stdout), "LocalKey\n");

    let fields = bibi(directory.path())
        .args(["list", "--fields", "key,source,arxiv-url"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&fields.stdout),
        "LocalKey\tlocal\thttps://arxiv.org/pdf/1207.7214\n"
    );

    let shown = bibi(directory.path())
        .args(["show", "k:LocalKey"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout),
        "@misc{LocalKey, title={Local title}, eprint={1207.7214}}\n"
    );

    let removed = bibi(directory.path())
        .args(["remove", "k:LocalKey"])
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert_eq!(String::from_utf8_lossy(&removed.stdout), "LocalKey\n");
}

#[test]
fn import_reads_the_complete_redirected_stream() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = bibi(directory.path())
        .arg("import")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"@misc{Piped,title={Piped}}")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Piped\n");
}

#[test]
fn removed_commands_and_options_are_usage_errors() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec!["rename", "k:Old", "New"],
        vec!["fetch", "k:Old"],
        vec!["add", "--key", "Mine", "1207.7214"],
        vec!["sync", "--force"],
    ] {
        assert_eq!(
            bibi(directory.path()).args(args).status().unwrap().code(),
            Some(2)
        );
    }
}
