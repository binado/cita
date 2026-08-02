//! Exact BibTeX payloads and their derived citation keys.

/// An exact BibTeX payload paired with its derived citation key.
///
/// The core model deliberately does not validate BibTeX grammar. That is the
/// responsibility of the parser at the input boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bibtex {
    source: String,
    texkey: String,
}

impl Bibtex {
    /// Construct a payload from its exact source and parser-derived key.
    pub fn new(source: impl Into<String>, texkey: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            texkey: texkey.into(),
        }
    }

    /// The exact BibTeX source bytes.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The citation key derived from the source.
    pub fn texkey(&self) -> &str {
        &self.texkey
    }
}
