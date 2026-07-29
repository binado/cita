//! Writing a bibliography, and verifying one.
//!
//! bibi keeps no `.bib` continuously in step with the manifest. Mutations write
//! the manifest and nothing else; a bibliography is rendered when the user asks
//! for it. That is what makes it output rather than maintained state, and what
//! lets it be a function of the manifest *and* its options.

use crate::{
    error::Error,
    render::{RenderOptions, render_manifest},
    services::Services,
    sync::{SyncReport, SyncRequest, sync},
    target,
};
use bibi_core::ProviderName;
use bibi_manifest::{ManifestStore, ParentPolicy, atomic_replace};
use std::path::{Path, PathBuf};

/// The conventional name of a rendered bibliography.
pub const DEFAULT_OUTPUT: &str = "references.bib";

/// Options for `export`.
#[derive(Clone, Debug, Default)]
pub struct ExportRequest {
    /// Refresh this provider's records before rendering.
    pub provider: Option<ProviderName>,
    /// Force that preliminary sync. Meaningless without `provider`.
    pub force: bool,
    /// Where to write. Relative paths resolve against the manifest directory.
    pub output: Option<PathBuf>,
    /// Which records to include.
    pub options: RenderOptions,
}

/// What an export did.
#[derive(Debug)]
pub struct ExportReport {
    /// Where the bibliography was written.
    pub path: PathBuf,
    /// How many records it contains.
    pub records: usize,
    /// The preliminary sync, if one ran.
    pub sync: Option<SyncReport>,
}

/// Render the selected bibliography, optionally syncing one provider first.
pub async fn export(
    services: &Services,
    store: &ManifestStore,
    request: &ExportRequest,
) -> Result<ExportReport, Error> {
    if request.force && request.provider.is_none() {
        return Err(Error::usage(
            "`--force` forces the preliminary sync, so it needs `--provider <name>`",
        ));
    }

    let mut report = None;
    if let Some(provider) = &request.provider {
        let synced = sync(
            services,
            store,
            &SyncRequest {
                provider: Some(provider.clone()),
                force: request.force,
                dry_run: false,
            },
        )
        .await?;
        if synced.has_failures() {
            // Validated updates have already been committed under the ordinary
            // partial-batch rule, but no bibliography is written: a rendered
            // file that silently omits a record's update is worse than none.
            return Err(Error::SyncFailed {
                failures: synced.failures.len(),
            });
        }
        report = Some(synced);
    }

    // Reload after the sync so the output is a function of bytes actually
    // published as authority, rather than of an uncommitted candidate.
    let manifest = store.load()?.manifest;
    let rendered = render_manifest(&manifest, &request.options)?;
    let path = output_path(store, request.output.as_deref())?;
    atomic_replace(&path, rendered.as_bytes(), ParentPolicy::Require)?;
    Ok(ExportReport {
        path,
        records: manifest.filter(&request.options.filter).count(),
        sync: report,
    })
}

/// Where a bibliography should be written, refusing to overwrite the manifest.
///
/// The comparison is over canonicalized paths, not path strings, so a symlink
/// or an alternate spelling of the same file is caught. The output usually does
/// not exist yet, so its parent is canonicalized and the file name joined on.
fn output_path(store: &ManifestStore, requested: Option<&Path>) -> Result<PathBuf, Error> {
    let path = target::output(store, requested.unwrap_or(Path::new(DEFAULT_OUTPUT)));
    let manifest = store
        .path()
        .canonicalize()
        .unwrap_or_else(|_| store.path().to_path_buf());
    // An existing output canonicalizes directly, which follows a symlink to
    // whatever it really points at. One that does not exist yet cannot, so its
    // parent is canonicalized and the file name joined on instead.
    let resolved =
        path.canonicalize()
            .unwrap_or_else(|_| match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) => parent
                    .canonicalize()
                    .map(|parent| parent.join(name))
                    .unwrap_or_else(|_| path.clone()),
                _ => path.clone(),
            });
    if resolved == manifest {
        return Err(Error::usage(format!(
            "`{}` is the manifest bibi renders from; choose another output path",
            path.display()
        )));
    }
    Ok(path)
}

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
/// Always offline, and it never writes either file. It accepts no provider or
/// force option: a check that could mutate would not be a check.
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
