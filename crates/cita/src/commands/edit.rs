use super::find_manifest;
use anyhow::{Context, Result, bail};
use cita_manifest::{Entry, MANIFEST_FILE, Manifest};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Prefix of the diagnostic block injected into a rejected buffer.
///
/// TOML treats these lines as comments and the saved manifest is re-rendered
/// from the parsed data, so they can never reach `cita.toml`. The prefix also
/// makes the block recognizable, which is how a second round of errors replaces
/// the first instead of piling up.
const DIAGNOSTIC: &str = "# cita:";

/// Edit the whole manifest in an external editor.
///
/// This is also the only way to create a reference no provider knows about:
/// adding BibTeX by hand is deliberately out of scope, so a hand-written entry
/// is structured fields only.
pub(crate) fn edit(cwd: &Path) -> Result<()> {
    let path = find_manifest(cwd)?;
    // Drift is fatal before editing: an edit that started from a manifest
    // disagreeing with references.bib would silently adopt the disagreement.
    let mut manifest = Manifest::load_verified(&path)?;
    let directory = tempfile::tempdir().context("could not create a temporary directory")?;
    let buffer = directory.path().join(MANIFEST_FILE);
    let mut contents =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    let mut rejected: Option<Vec<String>> = None;
    loop {
        fs::write(&buffer, &contents)
            .with_context(|| format!("could not write {}", buffer.display()))?;
        launch_editor(&buffer)?;
        let edited = fs::read_to_string(&buffer)
            .with_context(|| format!("could not read {}", buffer.display()))?;
        if edited == contents {
            // Saving a rejected buffer untouched is how the user gives up, so
            // the reasons are repeated on stderr rather than left in a
            // temporary file that is about to be deleted.
            if let Some(problems) = rejected {
                bail!(
                    "the edit was rejected and has not been saved:\n  {}",
                    problems.join("\n  ")
                );
            }
            println!("No changes to {}", path.display());
            return Ok(());
        }
        let candidate = strip_diagnostics(&edited).to_owned();
        match review(&candidate, &buffer, manifest.references()) {
            Ok(references) => {
                if manifest.replace_all(references)? {
                    println!("Updated {}", path.display());
                } else {
                    println!("No changes to {}", path.display());
                }
                return Ok(());
            }
            Err(problems) => {
                // The user's own bytes are preserved verbatim beneath the
                // diagnostics, so nothing typed into a rejected buffer is lost.
                contents = format!("{}{candidate}", diagnostics(&problems));
                rejected = Some(problems);
            }
        }
    }
}

/// Parse a candidate buffer and enforce every ownership rule against the
/// manifest it was rendered from, reporting all violations at once.
fn review(
    candidate: &str,
    buffer: &Path,
    current: &BTreeMap<String, Entry>,
) -> Result<BTreeMap<String, Entry>, Vec<String>> {
    let references = Manifest::parse(candidate, buffer).map_err(|error| vec![error.to_string()])?;
    let mut problems = Vec::new();
    for (key, entry) in &references {
        let Some(old) = current.get(key) else {
            // A hand-written reference is structured fields only. BibTeX is
            // opaque provider cargo, so there is nowhere for authored bytes to
            // have come from.
            if entry.bibtex.is_some() {
                problems.push(format!("`{key}.bibtex` cannot be written by hand"));
            }
            continue;
        };
        if old.bibtex != entry.bibtex {
            problems.push(format!("`{key}.bibtex` is immutable"));
        }
        if old.inspire.is_some()
            && let Some(field) = old.provider_field_change(entry)
        {
            problems.push(format!(
                "`{key}.{field}` is owned by INSPIRE; edit tags and notes instead, or run `cita sync`"
            ));
        }
    }
    if problems.is_empty() {
        Ok(references)
    } else {
        Err(problems)
    }
}

/// Render the reasons as a comment block.
///
/// Every line is commented, not just the first of each problem: a TOML parse
/// error spans several lines, and an uncommented continuation would both break
/// the buffer it is describing and survive `strip_diagnostics` into the next
/// round.
fn diagnostics(problems: &[String]) -> String {
    let lines = ["the edit was rejected and has NOT been saved."]
        .into_iter()
        .chain(problems.iter().flat_map(|problem| problem.lines()))
        .chain(["fix the entries below and save again, or save unchanged to abort."]);
    let mut block = String::new();
    for line in lines {
        block.push_str(&format!("{DIAGNOSTIC} {line}\n"));
    }
    block.push('\n');
    block
}

/// Remove a leading diagnostic block, so a second round of errors replaces the
/// first rather than accumulating.
fn strip_diagnostics(buffer: &str) -> &str {
    let mut rest = buffer;
    while rest.starts_with(DIAGNOSTIC) {
        while let Some(tail) = rest.strip_prefix(DIAGNOSTIC) {
            rest = tail.split_once('\n').map_or("", |(_, tail)| tail);
        }
        // Every block ends with the blank line separating it from the manifest.
        rest = rest.strip_prefix('\n').unwrap_or(rest);
    }
    rest
}

fn launch_editor(buffer: &Path) -> Result<()> {
    let (program, arguments) = editor_command();
    let status = Command::new(&program)
        .args(&arguments)
        .arg(buffer)
        .status()
        .with_context(|| format!("could not run the editor `{}`", program.display()))?;
    if !status.success() {
        bail!("editor `{}` exited with {status}", program.display());
    }
    Ok(())
}

/// `$VISUAL`, then `$EDITOR`, then `vi`.
///
/// The setting is split on whitespace so the common `EDITOR="code -w"` form
/// works; a path containing spaces has to be wrapped in a script instead.
fn editor_command() -> (PathBuf, Vec<String>) {
    editor_from(
        ["VISUAL", "EDITOR"]
            .into_iter()
            .find_map(|name| env::var(name).ok()),
    )
}

/// Split one editor setting, taken as an argument so the rule stays testable
/// without mutating the environment of a threaded test runner.
fn editor_from(setting: Option<String>) -> (PathBuf, Vec<String>) {
    let mut words = setting
        .iter()
        .flat_map(|setting| setting.split_whitespace())
        .map(str::to_owned);
    let program = words.next().map_or_else(|| "vi".into(), PathBuf::from);
    (program, words.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_blocks_are_replaced_rather_than_accumulated() {
        let buffer = format!("{}schema = 2\n", diagnostics(&["first".into()]));
        assert_eq!(strip_diagnostics(&buffer), "schema = 2\n");
        let again = format!("{}{}", diagnostics(&["second".into()]), buffer);
        assert_eq!(strip_diagnostics(&again), "schema = 2\n");
        // A buffer the user never let cita annotate is returned untouched.
        assert_eq!(strip_diagnostics("schema = 2\n"), "schema = 2\n");
        // A comment the user wrote themselves is not a diagnostic.
        assert_eq!(
            strip_diagnostics("# mine\nschema = 2\n"),
            "# mine\nschema = 2\n"
        );
    }

    #[test]
    fn editor_settings_split_program_from_arguments() {
        assert_eq!(
            editor_from(Some("code -w --new-window".into())),
            (
                PathBuf::from("code"),
                vec!["-w".into(), "--new-window".into()]
            )
        );
        assert_eq!(
            editor_from(Some("vim".into())),
            (PathBuf::from("vim"), Vec::new())
        );
        // An unset or blank setting falls through to the POSIX default.
        assert_eq!(editor_from(None), (PathBuf::from("vi"), Vec::new()));
        assert_eq!(
            editor_from(Some("   ".into())),
            (PathBuf::from("vi"), Vec::new())
        );
    }
}
