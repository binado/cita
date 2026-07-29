//! The global, derived, disposable document cache.
//!
//! Artifacts are addressed by normalized arXiv identifier and kind, never by
//! record identity, so projects citing one paper share one copy. Downloads are
//! published atomically and source archives are extracted under hard bounds.
#![warn(missing_docs)]
