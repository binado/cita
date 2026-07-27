# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- add global library persistence, deterministic shelf paths, and advisory locks
- add `ShelfName`, a validated newtype that makes `shelves/<name>` traversal-safe
  by construction
- add `LibraryLock`, `Library::lock_registry`, `ShelfLock::name`, and
  `ShelfLock::manifest`, which pairs a manifest with the lock protecting it
- add `LibraryError::ShelfMissing` for a registered shelf whose directory is gone,
  and `LibraryError::RelativeHome` for a non-absolute home directory

### Changed

- **breaking**: `MANIFEST_FILE` changed value from `cita.toml` to `shelf.toml`,
  and `LIBRARY_FILE` from `cita-library.toml` to `library.toml`. Code that uses
  these constants keeps compiling but will read and write different filenames
- **breaking**: `Library::shelves` returns `&BTreeSet<ShelfName>`, and
  `shelf_manifest`, `create_shelf`, and `lock_shelf` take `&ShelfName` instead of
  `&str`
- **breaking**: `Manifest::create` always takes the manifest file path; it no
  longer appends `MANIFEST_FILE` when handed an existing directory
- **breaking**: `LibraryError` is now `#[non_exhaustive]`
- **breaking**: lock files are `locks/registry.lock` and `locks/shelf-<name>.lock`;
  a shelf named `library` previously collided with the registry lock
- `LibraryError` and `Error` variants no longer repeat their underlying cause in
  `Display`, which `anyhow`'s `{:#}` was printing twice
- `Library::open_or_create` recreates a missing default shelf without parsing an
  existing manifest, so a corrupt `main` no longer fails unrelated shelves
- `create_shelf` validates before creating, scoping the alias check to the new
  name so one damaged shelf cannot block creation or leave an orphan directory

### Removed

- **breaking**: remove `Shelf`, `validate_shelf_name` (superseded by `ShelfName`),
  `BIBLIOGRAPHY_FILE`, `Error::BibliographyDrift`, and `Manifest::{import_existing,
  load_verified, verify_bibliography, generate, bibliography_path}`
- remove generated-bibliography coordination and arbitrary shelf registration
  paths

## [0.5.0](https://github.com/binado/cita/compare/cita-manifest-v0.4.0...cita-manifest-v0.5.0) - 2026-07-28

### Other

- simplify INSPIRE record pipeline ([#40](https://github.com/binado/cita/pull/40))

### Changed

- [**breaking**] accept the renamed `InspireSnapshot` provider type in public
  manifest conversion and refresh APIs without changing schema-1 serialization

## [0.3.2](https://github.com/binado/cita/compare/cita-manifest-v0.3.1...cita-manifest-v0.3.2) - 2026-07-25

### Added

- add derived BibTeX export for reference managers ([#34](https://github.com/binado/cita/pull/34))

## [0.3.1](https://github.com/binado/cita/compare/cita-manifest-v0.3.0...cita-manifest-v0.3.1) - 2026-07-23

### Added

- add library and shelf support ([#32](https://github.com/binado/cita/pull/32))

## [0.2.0](https://github.com/binado/cita/compare/cita-manifest-v0.1.0...cita-manifest-v0.2.0) - 2026-07-21

### Added

- collision-tolerant import/add with skip-by-default and --overwrite ([#26](https://github.com/binado/cita/pull/26))

### Other

- release v0.1.0 ([#24](https://github.com/binado/cita/pull/24))

## [0.1.0](https://github.com/binado/cita/releases/tag/cita-manifest-v0.1.0) - 2026-07-21

### Added

- [**breaking**] store authoritative BibTeX with curated INSPIRE identifiers ([#15](https://github.com/binado/cita/pull/15))
- use canonical INSPIRE bibliography with sync ([#14](https://github.com/binado/cita/pull/14))
- resolve transient papers for fetch and open ([#13](https://github.com/binado/cita/pull/13))
- add PDF fetching and opening ([#11](https://github.com/binado/cita/pull/11))

### Fixed

- drop publish = [\"crates-io\"] allowlists ([#25](https://github.com/binado/cita/pull/25))

### Other

- prepare cita v0.1.0 for release ([#22](https://github.com/binado/cita/pull/22))
- simplify Reference model after schema-1 ([#21](https://github.com/binado/cita/pull/21))
- rename project and crates to cita ([#9](https://github.com/binado/cita/pull/9))
