# Changelog

All notable changes to `cita-store` will be documented in this file.

## [Unreleased]

### Added

- add a SQLite-backed global bibliography with shared shelf memberships
- add deterministic lossless JSON and TOML interchange documents

### Changed

- store typed INSPIRE refresh records instead of generic provider records
- replace the `rusqlite` implementation with a private Diesel adapter while
  preserving schema version 1 and existing database compatibility
- split library persistence into connection, schema, model, read, and write
  modules and batch reference hydration in bounded ID chunks
- replace `LibraryError::Sql` with typed `Connection` and `Database` variants
