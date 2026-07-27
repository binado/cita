# Plan: Simplify the core data model after the schema-1 pivot

## Context

The `#15` change ("store authoritative BibTeX with curated INSPIRE identifiers")
changed a core invariant: **every** `SourceSnapshot` now carries authoritative
BibTeX, and the tracked `references.bib` is produced by re-keying that stored
BibTeX (`rename_entry`), never by writing out a projected `Reference`. Three
pieces of the older design — when a source could exist *without* BibTeX and had
to be rendered from the neutral `Reference` — survived the pivot and are now
either unreachable, write-only, or duplicated. This plan makes the code match
the new invariant, removing dead machinery and one verbatim duplication.

All three crates involved (`cita-core`, `cita-bibliography`, `cita-inspire-client`,
`cita-manifest`) are internal workspace crates with no external publication, so
unused `pub` items are dead weight rather than API surface.

Verified during exploration:
- `render_reference` + `set_optional` + `set_chunks` have **zero** production
  callers (only `cita-bibliography`'s own unit test).
- `Reference` is **never** serialized/deserialized anywhere (no `serde_json`, not
  stored to disk — the manifest persists `SourceSnapshot`/BibTeX, not projections).
  Its serde derives are vestigial.
- `Reference` fields actually read in production: `title`, `authors`,
  `collaborations`, `year`, `identifiers` (via `list.rs`, `git.rs`, `fetch.rs`).
  `publication`, `url`, `primary_category` are projected but never read.
- `Publication` is referenced only inside `cita-bibliography` + `cita-core`.
- Dependency direction: `cita-inspire-client` → {`cita-bibliography`, `cita-core`};
  `cita-manifest` → all three. So a shared INSPIRE-projection helper belongs in
  `cita-inspire-client`.

## Fix 1 — Delete the dead `Reference → BibTeX` render path

**`crates/cita-bibliography/src/lib.rs`**
- Remove `render_reference` (~286-342), `set_optional` (~344-348), `set_chunks`
  (~350-355), and the test `generic_reference_uses_biblatex_serialization`
  (~419-435).
- Drop the now-unused `biblatex` imports `Chunk` and `EntryType` from the
  top `use biblatex::{…}` block. **Keep** `Entry as BibEntry`, `ChunksExt`,
  `DateValue`, `PermissiveType`, `Bibliography`, `RawBibliography`.
- **Keep** `format_person` — it is used by `project_bibtex`.

**`AGENTS.md`**
- In the `cita-bibliography` bullet, drop the trailing "and generic
  `biblatex::Entry` rendering" (rendering now always re-keys stored BibTeX).

## Fix 2 — Trim the neutral `Reference` model (incl. dropping dead serde)

**`crates/cita-core/src/reference.rs`**
- Remove the `publication`, `url`, and `primary_category` fields from `Reference`.
- Delete the `Publication` struct entirely (this also resolves the
  `Reference.year` vs `Publication.year` redundancy).
- Change `Reference` and `Identifiers` derives from
  `(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)` to
  `(Clone, Debug, Default, Eq, PartialEq)`, and remove every `#[serde(...)]`
  attribute on their fields.
- Remove `use serde::{Deserialize, Serialize};`. Keep `BTreeMap`
  (`Identifiers.providers`) and `Future` (`MetadataProvider`).

**`crates/cita-core/src/lib.rs`**
- Remove `Publication` from the `pub use reference::{…}` re-export list.

**`crates/cita-bibliography/src/lib.rs`** (`project_bibtex`)
- Remove the `publication` block (~120-134) and the `url` / `primary_category`
  assignments (~153-154). Construct `Reference` with only `title`, `authors`,
  `collaborations`, `year`, `identifiers`.
- Remove `Publication` from the `cita_core::{…}` import (coincides with Fix 1's
  import cleanup). The `year` local stays (feeds the `year` field); `chunks`
  stays (still used for `collaboration`).

**`crates/cita-inspire-client/src/snapshot.rs`** (`SelectedRecord::project`, ~96)
- Remove the `url: Some(format!("https://inspirehep.net/literature/{}", …))`
  line; keep `title`, `identifiers`, and `..Reference::default()`.

## Fix 3 — Share the duplicated INSPIRE projection

`InspireEntry::project` (`cita-manifest/src/lib.rs:69-85`) and
`InspireRecord::project` (`cita-inspire-client/src/snapshot.rs:111-127`) are
verbatim identical: `project_bibtex` → override arxiv → override doi → insert
`inspire` provider. Collapse to one helper.

**`crates/cita-inspire-client/src/snapshot.rs`**
- Add:
  ```rust
  pub fn project_inspire(
      bibtex: &str,
      arxiv: Option<&str>,
      doi: Option<&str>,
      record_id: u64,
  ) -> Result<Reference, ProjectionError> {
      let mut reference = project_bibtex(bibtex)
          .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
      if let Some(arxiv) = arxiv {
          reference.identifiers.arxiv = vec![normalize_arxiv(arxiv)];
      }
      if let Some(doi) = doi {
          reference.identifiers.dois = vec![normalize_doi(doi)];
      }
      reference
          .identifiers
          .providers
          .insert("inspire".to_owned(), vec![record_id.to_string()]);
      Ok(reference)
  }
  ```
- Rewrite `InspireRecord::project` to delegate:
  `project_inspire(&self.bibtex, self.arxiv.as_deref(), self.doi.as_deref(), self.record_id)`.

**`crates/cita-inspire-client/src/lib.rs`**
- Re-export `project_inspire` alongside `InspireRecord`.

**`crates/cita-manifest/src/lib.rs`**
- Rewrite `InspireEntry::project` to delegate:
  `cita_inspire_client::project_inspire(&self.bibtex, self.identifiers.arxiv.as_deref(), self.identifiers.doi.as_deref(), self.record_id)`.
- Leave `project_bibtex`, `normalize_arxiv`, `normalize_doi` imports — still used
  by `validate_references` / `find`.

**Struct shapes stay distinct** (intentional): `InspireEntry` keeps its nested
`HepIdentifiers` TOML table and `InspireRecord` keeps flat `Option`s + `texkey`.
Only the projection *logic* is unified.

## Suggested sequencing

Fixes 1 and 2 both touch `project_bibtex` and its imports, so apply them
together, then Fix 3. All on one branch.

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings` — the authoritative
  check for leftover unused imports/items after the deletions.
- `cargo test --workspace` — existing suites cover the surviving behavior
  (`cita-bibliography` projection tests, `cita-manifest` round-trip/refresh
  tests, `cita-inspire-client` snapshot projection tests including
  `durable_projection_derives_content_from_bibtex_and_overrides_identity`,
  which now exercises the shared `project_inspire`).
- Optional (network): `cargo test --test e2e -- --ignored`.

Expected net effect: ~120+ lines removed, one duplicated `project()` body
eliminated, and the neutral `Reference` narrowed to exactly the fields the code
consumes — with no behavior change (bibliography rendering, identity checks,
list/fetch output all unchanged).
