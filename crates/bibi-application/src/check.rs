//! Verifying a rendered bibliography.
//!
//! bibi neither writes a `.bib` nor keeps one in step with the manifest.
//! Mutations write the manifest and nothing else; a bibliography is rendered to
//! stdout when the user asks for it, and the shell decides whether and where
//! that becomes a file. That is what makes it output rather than maintained
//! state, and what leaves `check` with one job — comparing a file someone else
//! placed against what the manifest says it should be.

use crate::{
    error::Error,
    render::{RenderOptions, render_manifest},
};
use bibi_manifest::ManifestStore;
use std::path::{Path, PathBuf};

/// Whether a rendered bibliography still matches the manifest.
#[derive(Debug)]
pub enum CheckOutcome {
    /// The file is byte-identical to what would be rendered now.
    Match {
        /// The file that was checked.
        path: PathBuf,
    },
    /// The file exists and differs.
    Drift {
        /// The file that was checked.
        path: PathBuf,
        /// A short human-readable account of the first difference.
        summary: String,
    },
    /// There is no such file.
    ///
    /// Reported as its own outcome rather than as drift: rendering a diff of
    /// the expected bytes against nothing would bury the actual answer, which
    /// is that the file the user expected to check is not there.
    Missing {
        /// Where one was expected.
        path: PathBuf,
    },
}

impl CheckOutcome {
    /// True when the check did not pass.
    pub fn failed(&self) -> bool {
        !matches!(self, Self::Match { .. })
    }
}

/// Compare a rendered bibliography with what the manifest says it should be.
///
/// Always offline, and it never writes either file: a check that could mutate
/// would not be a check.
pub fn check(
    store: &ManifestStore,
    path: &Path,
    options: &RenderOptions,
) -> Result<CheckOutcome, Error> {
    let manifest = store.load()?.manifest;
    let expected = render_manifest(&manifest, options)?;
    let found = match std::fs::read_to_string(path) {
        Ok(found) => found,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CheckOutcome::Missing {
                path: path.to_path_buf(),
            });
        }
        Err(error) => return Err(Error::io(path, error)),
    };
    if expected == found {
        Ok(CheckOutcome::Match {
            path: path.to_path_buf(),
        })
    } else {
        Ok(CheckOutcome::Drift {
            path: path.to_path_buf(),
            summary: describe_drift(&expected, &found),
        })
    }
}

/// Name the first difference, without pulling in a diff library.
fn describe_drift(expected: &str, found: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let found_lines = found.lines().collect::<Vec<_>>();
    let at = expected_lines
        .iter()
        .zip(&found_lines)
        .position(|(left, right)| left != right);
    let mut summary = format!(
        "expected {} line(s), found {}",
        expected_lines.len(),
        found_lines.len()
    );
    if let Some(at) = at {
        summary.push_str(&format!(
            "; first difference at line {}:\n  expected: {}\n  found:    {}",
            at + 1,
            expected_lines[at],
            found_lines[at]
        ));
    } else if expected_lines.len() != found_lines.len() {
        let (label, extra) = if expected_lines.len() > found_lines.len() {
            ("missing", &expected_lines[found_lines.len()..])
        } else {
            ("unexpected", &found_lines[expected_lines.len()..])
        };
        summary.push_str(&format!(
            "; {label} from line {}:\n  {}",
            expected_lines.len().min(found_lines.len()) + 1,
            extra.first().copied().unwrap_or_default()
        ));
    }
    summary
}
