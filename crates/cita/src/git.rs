use anyhow::{Context, Result, bail};
use cita_bibliography::{Bibliography, Entry};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn repository_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", path.to_str()?, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

pub fn commit(bibliography_path: &Path) -> Result<()> {
    let root = repository_root(bibliography_path.parent().unwrap_or_else(|| Path::new(".")))
        .context("references.bib is not inside a Git repository")?;
    let relative = bibliography_path
        .strip_prefix(&root)
        .context("references.bib is not inside the discovered Git repository")?;
    let relative_text = relative.to_string_lossy();
    let current_bytes = fs::read(bibliography_path)?;
    let old_bytes = git_output_allow_failure(&root, &["show", &format!("HEAD:{relative_text}")]);
    if old_bytes.as_deref() == Some(current_bytes.as_slice()) {
        println!("references.bib has no Git changes");
        return Ok(());
    }

    let current = Bibliography::load(bibliography_path)?;
    let (subject, body) = if let Some(bytes) = &old_bytes {
        match parse_committed_bibliography(bytes)? {
            Some(old) => commit_message(Some(old.entries()), current.entries()),
            None => ("references: update bibliography".into(), String::new()),
        }
    } else {
        commit_message(None, current.entries())
    };

    run_git(&root, &["add", "--", &relative_text])?;
    let mut args = vec!["commit", "--only", "-m", &subject];
    if !body.is_empty() {
        args.extend(["-m", &body]);
    }
    args.extend(["--", &relative_text]);
    run_git(&root, &args)?;
    println!("Committed references.bib: {subject}");
    Ok(())
}

/// Parse the bibliography bytes committed at HEAD; `None` means the blob is
/// not a readable current-format bibliography.
fn parse_committed_bibliography(bytes: &[u8]) -> Result<Option<Bibliography>> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("references.bib");
    fs::write(&path, bytes)?;
    Ok(Bibliography::load(path).ok())
}

fn commit_message(
    old: Option<&BTreeMap<String, Entry>>,
    new: &BTreeMap<String, Entry>,
) -> (String, String) {
    let Some(old) = old else {
        return (
            "references: initialize cita".into(),
            body_lines([], new, []),
        );
    };
    let added = new
        .iter()
        .filter(|(key, _)| !old.contains_key(*key))
        .collect::<Vec<_>>();
    let removed = old
        .iter()
        .filter(|(key, _)| !new.contains_key(*key))
        .collect::<Vec<_>>();
    let modified = new
        .iter()
        .filter(|(key, record)| old.get(*key).is_some_and(|old| old != *record))
        .collect::<Vec<_>>();
    let subject = match (added.len(), removed.len(), modified.len()) {
        (count, 0, 0) if count > 0 => format!("references: add {count} {}", plural(count)),
        (0, count, 0) if count > 0 => format!("references: remove {count} {}", plural(count)),
        (0, 0, 0) => "references: update bibliography".into(),
        _ => "references: update bibliography".into(),
    };
    (subject, body_lines(removed, added, modified))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "paper" } else { "papers" }
}

fn body_lines<'a>(
    removed: impl IntoIterator<Item = (&'a String, &'a Entry)>,
    added: impl IntoIterator<Item = (&'a String, &'a Entry)>,
    modified: impl IntoIterator<Item = (&'a String, &'a Entry)>,
) -> String {
    removed
        .into_iter()
        .map(|(key, entry)| format!("- {key} — {}", entry.title()))
        .chain(
            added
                .into_iter()
                .map(|(key, entry)| format!("+ {key} — {}", entry.title())),
        )
        .chain(
            modified
                .into_iter()
                .map(|(key, entry)| format!("~ {key} — {}", entry.title())),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

fn git_output_allow_failure(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn run_git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_specific_messages() {
        fn entries(items: &[(&str, &str)]) -> BTreeMap<String, Entry> {
            let source = items
                .iter()
                .map(|(key, title)| format!("@article{{{key},title={{{title}}}}}"))
                .collect::<Vec<_>>()
                .join("\n");
            cita_bibliography::parse(&source).unwrap()
        }
        let empty = entries(&[]);
        let just_a = entries(&[("A", "Alpha")]);
        let a_and_b = entries(&[("A", "Alpha"), ("B", "Beta")]);
        assert_eq!(
            commit_message(None, &empty).0,
            "references: initialize cita"
        );
        assert_eq!(
            commit_message(Some(&empty), &a_and_b).0,
            "references: add 2 papers"
        );
        assert_eq!(
            commit_message(Some(&just_a), &empty).0,
            "references: remove 1 paper"
        );
        assert_eq!(
            commit_message(Some(&just_a), &just_a).0,
            "references: update bibliography"
        );
    }
}
