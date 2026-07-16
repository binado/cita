//! Locator parsing and identifier normalization shared by Cita crates.

mod locator;

pub use locator::{Error, Locator, normalize_arxiv, normalize_doi, strip_arxiv_version};
