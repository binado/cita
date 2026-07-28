# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
