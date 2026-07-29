//! What an artifact is called, and where arXiv publishes it.

use bibi_core::ArxivId;

/// Which artifact of a work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactKind {
    /// The typeset PDF.
    Pdf,
    /// The original source package, as arXiv serves it.
    Source,
}

impl ArtifactKind {
    /// A word for diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Source => "source",
        }
    }

    /// The file extension a downloaded artifact of this kind carries.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Source => "tar.gz",
        }
    }

    /// The path segment arXiv publishes this kind under.
    pub(crate) fn endpoint(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Source => "e-print",
        }
    }
}

/// What a downloaded artifact is called when the caller names no path.
///
/// The identifier and nothing else, which is arXiv's own naming rather than a
/// scheme of bibi's. A descriptive name would need a portable Unicode slugging
/// rule, a decision about whether a collaboration counts as an author, and a
/// rule for what to drop when a value is absent — three decisions in exchange
/// for a filename the user can rename with `--output` anyway.
///
/// A legacy identifier carries a `/`, which is not a filename character, so it
/// becomes `-`. arXiv cannot avoid that either: it publishes the archive as a
/// URL path component, so a browser saving `/pdf/hep-th/9901001` keeps only
/// `9901001` and loses which archive it came from.
///
/// The name carries no version. [`ArxivId`] is versionless by design, because
/// a record names a work rather than one revision of it.
pub fn default_filename(id: &ArxivId, kind: ArtifactKind) -> String {
    format!("{}.{}", id.as_str().replace('/', "-"), kind.extension())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arxiv(value: &str) -> ArxivId {
        ArxivId::new(value).unwrap()
    }

    #[test]
    fn a_modern_identifier_is_the_whole_name() {
        assert_eq!(
            default_filename(&arxiv("1207.7214v2"), ArtifactKind::Pdf),
            "1207.7214.pdf"
        );
        assert_eq!(
            default_filename(&arxiv("2401.00001"), ArtifactKind::Source),
            "2401.00001.tar.gz"
        );
    }

    #[test]
    fn a_legacy_identifier_keeps_its_archive_as_a_prefix() {
        assert_eq!(
            default_filename(&arxiv("hep-th/9901001"), ArtifactKind::Pdf),
            "hep-th-9901001.pdf"
        );
        // Normalization has already lowercased and stripped the version.
        assert_eq!(
            default_filename(&arxiv("HEP-TH/9901001v3"), ArtifactKind::Source),
            "hep-th-9901001.tar.gz"
        );
    }

    #[test]
    fn a_default_name_is_always_a_single_path_component() {
        for id in ["1207.7214", "hep-th/9901001", "math.GT/0309136"] {
            for kind in [ArtifactKind::Pdf, ArtifactKind::Source] {
                let name = default_filename(&arxiv(id), kind);
                assert_eq!(
                    std::path::Path::new(&name).components().count(),
                    1,
                    "{name} is not one component"
                );
            }
        }
    }
}
