//! Compare derived BibTeX with an ordinary file.

use crate::{Error, RenderOptions, render_bibliography};
use bibi_core::Bibliography;
use std::path::{Path, PathBuf};

/// Check result.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum CheckOutcome {
    /// Exact match.
    Match { path: PathBuf },
    /// Existing file differs.
    Drift { path: PathBuf, summary: String },
    /// File is absent.
    Missing { path: PathBuf },
}

impl CheckOutcome {
    /// Whether the check failed.
    pub fn failed(&self) -> bool {
        !matches!(self, Self::Match { .. })
    }
}

/// Compare without writing.
pub fn check(
    bibliography: &Bibliography,
    path: &Path,
    options: &RenderOptions,
) -> Result<CheckOutcome, Error> {
    let expected = render_bibliography(bibliography, options);
    let found = match std::fs::read_to_string(path) {
        Ok(found) => found,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CheckOutcome::Missing {
                path: path.to_owned(),
            });
        }
        Err(error) => return Err(Error::io(path, error)),
    };
    if expected == found {
        Ok(CheckOutcome::Match {
            path: path.to_owned(),
        })
    } else {
        Ok(CheckOutcome::Drift {
            path: path.to_owned(),
            summary: format!("expected {} byte(s), found {}", expected.len(), found.len()),
        })
    }
}
