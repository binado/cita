# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- add a SQLite-backed global library with shared references and shelf-local keys
- add lossless JSON/TOML exports and `init --from-file`
- add import canonicalization, sync-time promotion, and `import --skip-errors`

### Changed

- **breaking**: replace `library.toml` and shelf manifests with
  `$CITA_HOME/library.sqlite3`
- **breaking**: rename the persistence crate from `cita-manifest` to `cita-store`
- sync deduplicates shared references and applies the selected set atomically
- `--overwrite` replaces only local shelf membership collisions
- **breaking**: `cita export` takes its destination as a positional argument;
  the `-o`/`--output` flag was removed
- **breaking**: `cita init` no longer accepts `--path`; the store location is
  set with `CITA_HOME`
- all-shelf JSON/TOML export writes one lossless library document
- `cita init` reports whether it created or found the store intact
- `cita shelf list` pads its columns so they stay aligned for longer shelf names

### Removed

- remove filesystem manifests and advisory lock files
- remove local project discovery, generated `references.bib`, `generate`,
  `commit`, and the path-based `library` command tree

## [0.4.0](https://github.com/binado/cita/compare/cita-v0.3.2...cita-v0.4.0) - 2026-07-25

### Added

- [**breaking**] select shelves with --shelf instead of a nested command tree ([#36](https://github.com/binado/cita/pull/36))

## [0.3.2](https://github.com/binado/cita/compare/cita-v0.3.1...cita-v0.3.2) - 2026-07-25

### Added

- add derived BibTeX export for reference managers ([#34](https://github.com/binado/cita/pull/34))

## [0.3.1](https://github.com/binado/cita/compare/cita-v0.3.0...cita-v0.3.1) - 2026-07-23

### Added

- add library and shelf support ([#32](https://github.com/binado/cita/pull/32))

## [0.3.0](https://github.com/binado/cita/compare/cita-v0.2.1...cita-v0.3.0) - 2026-07-23

### Added

- add arXiv source fetching ([#30](https://github.com/binado/cita/pull/30))

## [0.2.1](https://github.com/binado/cita/compare/cita-v0.2.0...cita-v0.2.1) - 2026-07-22

### Added

- accept canonical reference URLs ([#28](https://github.com/binado/cita/pull/28))

## [0.2.0](https://github.com/binado/cita/compare/cita-v0.1.0...cita-v0.2.0) - 2026-07-21

### Added

- collision-tolerant import/add with skip-by-default and --overwrite ([#26](https://github.com/binado/cita/pull/26))

### Other

- release v0.1.0 ([#24](https://github.com/binado/cita/pull/24))

## [0.1.0](https://github.com/binado/cita/releases/tag/cita-v0.1.0) - 2026-07-21

### Added

- merge document opening into fetch ([#16](https://github.com/binado/cita/pull/16))
- [**breaking**] store authoritative BibTeX with curated INSPIRE identifiers ([#15](https://github.com/binado/cita/pull/15))
- use canonical INSPIRE bibliography with sync ([#14](https://github.com/binado/cita/pull/14))
- resolve transient papers for fetch and open ([#13](https://github.com/binado/cita/pull/13))
- overhaul cita list with sorting, color, and title wrapping ([#12](https://github.com/binado/cita/pull/12))
- add PDF fetching and opening ([#11](https://github.com/binado/cita/pull/11))
- bootstrap PaperDB v0

### Fixed

- drop publish = [\"crates-io\"] allowlists ([#25](https://github.com/binado/cita/pull/25))

### Other

- prepare cita v0.1.0 for release ([#22](https://github.com/binado/cita/pull/22))
- simplify e2e test helpers and paper table ([#20](https://github.com/binado/cita/pull/20))
- add live INSPIRE E2E test and CI job ([#19](https://github.com/binado/cita/pull/19))
- streamline docs ([#18](https://github.com/binado/cita/pull/18))
- split cita/src/main.rs into one module per subcommand ([#17](https://github.com/binado/cita/pull/17))
- expand README with installation, usage, testing, and license ([#10](https://github.com/binado/cita/pull/10))
- rename project and crates to cita ([#9](https://github.com/binado/cita/pull/9))
- split manifest engine out of paperdb-core, move InspireProvider into client ([#4](https://github.com/binado/cita/pull/4))
