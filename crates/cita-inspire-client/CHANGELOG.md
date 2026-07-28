# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/binado/cita/compare/cita-inspire-client-v0.4.0...cita-inspire-client-v0.5.0) - 2026-07-28

### Other

- simplify INSPIRE record pipeline ([#40](https://github.com/binado/cita/pull/40))

### Added

- expose the supported `ApiLiteratureRecord` JSON model and
  `Client::resolve_api_record`

### Changed

- [**breaking**] replace `InspireRecord` with `InspireSnapshot` and attach
  authoritative BibTeX directly to API records

## [0.2.0](https://github.com/binado/cita/compare/cita-inspire-client-v0.1.0...cita-inspire-client-v0.2.0) - 2026-07-21

### Other

- release v0.1.0 ([#24](https://github.com/binado/cita/pull/24))

## [0.1.0](https://github.com/binado/cita/releases/tag/cita-inspire-client-v0.1.0) - 2026-07-21

### Added

- [**breaking**] store authoritative BibTeX with curated INSPIRE identifiers ([#15](https://github.com/binado/cita/pull/15))
- use canonical INSPIRE bibliography with sync ([#14](https://github.com/binado/cita/pull/14))

### Fixed

- drop publish = [\"crates-io\"] allowlists ([#25](https://github.com/binado/cita/pull/25))

### Other

- prepare cita v0.1.0 for release ([#22](https://github.com/binado/cita/pull/22))
- simplify Reference model after schema-1 ([#21](https://github.com/binado/cita/pull/21))
- rename project and crates to cita ([#9](https://github.com/binado/cita/pull/9))
