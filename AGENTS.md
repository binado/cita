# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

PaperDB is a Git-friendly bibliography database CLI. It resolves literature through
INSPIRE, stores citation metadata in a human-editable `paperdb.toml` manifest, and
exports deterministic BibTeX. Rust edition 2024, MSRV **1.88** (the `let ... && ...`
let-chains used throughout require it).

## Commands

```bash
cargo build --workspace
cargo test --workspace                     # unit + integration tests
cargo test -p paperdb-manifest manifest    # one crate, filter by name substring
cargo test --test cli                       # the CLI integration suite only
cargo fmt --all -- --check                  # CI runs this exact form
cargo clippy --workspace --all-targets -- -D warnings   # warnings are hard errors in CI
cargo run -p paperdb -- add 1207.7214       # run the binary
```

CI (`.github/workflows/ci.yml`) runs fmt-check, clippy-as-errors, test, and build on
both `1.88` and `stable`. Match it before pushing. Tests are hermetic: the
inspire-client suite spins up a local `TcpListener` instead of hitting the network,
and CLI/manifest tests use `tempfile` dirs — no test contacts inspirehep.net.

## Commit messages

Commits must follow the [Conventional Commits](https://www.conventionalcommits.org)
format: `<type>: <description>` (e.g. `fix: correct MSRV to 1.88`, `refactor: split
manifest engine out of paperdb-core`). Common types used in this repo: `fix`,
`feat`, `refactor`, `chore`, `ci`, `docs`, `test`.

## Workspace architecture

Four crates. `paperdb-core` is the light, provider-neutral vocabulary everything else
shares; the two middle crates never depend on each other; the binary is wiring only.

```
paperdb-core (light: serde, thiserror, async-trait, unicode-normalization)
   ↑                    ↑
paperdb-manifest     paperdb-inspire-client (+ reqwest, serde_json, url)
(+ toml, toml_edit,     ↑
   tempfile)            │
   ↑                    │
   └──── paperdb ───────┘   (binary)
```

- **`paperdb-core`** — models (`PaperRecord`, `ResolvedPaper`, `Publication`),
  `Locator` parsing, identifier normalization (`strip_arxiv_version`,
  `normalize_arxiv`, `normalize_doi`), citation-key helpers, the `MetadataProvider`
  trait, and the `INSPIRE_SOURCE` constant. No I/O, no provider knowledge beyond that
  constant.
- **`paperdb-manifest`** — the `paperdb.toml` storage engine (`Manifest`,
  `AddOutcome`, `Error`) and `export_bibtex`. The only crate that touches
  `toml`/`toml_edit`/`tempfile`.
- **`paperdb-inspire-client`** — INSPIRE metadata provider built on a reusable async
  literature API client. `src/provider.rs` implements core's `MetadataProvider`,
  mapping raw INSPIRE records into `ResolvedPaper`. Knows nothing about manifests.
- **`paperdb`** (binary) — CLI, manifest discovery, and scoped Git commits. Depends on
  all three; contains no domain logic of its own.

## Key design decisions

**Map-based paper storage (`crates/paperdb-core/src/model.rs`,
`crates/paperdb-manifest/src/manifest.rs`).** Papers are stored as
`[papers.<citation-key>]` TOML tables and held in memory as
`BTreeMap<String, PaperRecord>`. `PaperRecord` is the single stored form (no key
field); `ResolvedPaper` is what a provider returns — a `record` plus an advisory
`suggested_key` that is consumed at key-choice time and never stored. Key uniqueness
is structural: a duplicate `[papers.X]` table in a hand-written file is a TOML parse
error, and the map cannot hold two entries under one key. The one place the map is
weaker by default is guarded explicitly: `insert_papers` refuses to
`BTreeMap::insert` over an occupied key when the identities differ
(`Error::KeyConflict`) rather than silently replacing the existing paper.

**Manifest holds two views of the same file.** A typed
`BTreeMap<String, PaperRecord>` (parsed via `toml` + serde, used for all logic) and a
`toml_edit::DocumentMut` (used for edits). The `DocumentMut` path is what preserves
user comments and unknown TOML fields across `add`/`remove`. Any mutation must update
**both** in lockstep. Manifest reads reject `schema != 1`.

**Ordering rule.** `save()` always renders `[papers.<key>]` tables key-sorted
(`sort_paper_tables` assigns `Table::set_position` in key order before serializing).
Hand-edited order is normalized on the next write; comments travel with their tables.
`list` and `export_bibtex` iterate the sorted map directly, so all output is
stable/diff-friendly with no per-call sorting.

**Batch operations are all-or-nothing.** `insert_papers` / `remove_batch` snapshot
`self.papers` into `original` and restore it on any error before returning, and only
call `save()` after all validation passes. `save()` itself is atomic: write to a
`NamedTempFile` in the same directory, `sync_all`, then `persist` (rename). Tests
`preserves_comments_unknown_fields_and_is_atomic_on_conflict`,
`removals_are_all_or_nothing`, `key_collision_with_different_identity_is_a_conflict`
(all in `paperdb-manifest`), and `multi_remove_is_atomic` (in `tests/cli.rs`) guard
this — keep them green.

**Identity & de-duplication.** Papers are considered the same when their
`(source, source_id)` pair, any arXiv id, or any DOI overlaps *after normalization*
(arXiv: strip trailing `vN`, lowercase; DOI: trim + lowercase). Adding a paper that
overlaps an existing one is a no-op (`AddOutcome::Existing`); overlapping identifiers
that map to *different* records of the same source is an `IdentifierConflict` error.
`validate_papers` re-checks global uniqueness after every mutation.
`Locator::Inspire(id)` selectors match `source == INSPIRE_SOURCE && source_id ==
Some(id.to_string())`.

**Locators (`crates/paperdb-core/src/locator.rs`).** User input parses into
`Locator::{Inspire,Arxiv,Doi}`. A bare token is only accepted as arXiv; DOI and
INSPIRE require explicit `doi:` / `inspire:` prefixes. `--key` is only valid when
adding exactly one locator.

**Citation keys (`crates/paperdb-core/src/key.rs`).** Keys appear in BibTeX entries
and as quoted TOML table keys, so `validate_key` rejects whitespace, commas, braces,
quotes, and backslashes. Keys like `Aad:2012tfa` round-trip as quoted table keys
(`[papers."Aad:2012tfa"]`).

**Determinism.** BibTeX collapses internal whitespace but otherwise passes TeX
through verbatim; `@article` vs `@misc` is chosen by whether a journal is present.
For `@article` the year prefers `publication.year` (the journal year) and falls back
to the record's citation-display year; `@misc` uses the record year.

**Scoped Git commits (`crates/paperdb/src/git.rs`).** `paperdb commit` stages and
commits *only* `paperdb.toml` (via `git commit --only -- paperdb.toml`), leaving any
other staged changes intact. Commit messages are generated by diffing the old
committed manifest against the current one (added/removed/modified keys → semantic
subject + body). If the HEAD blob is not a readable current-format manifest, the
commit proceeds with the generic subject `references: update bibliography`. Git is
invoked as a subprocess; there is no libgit2 dependency.

**Forward-compatible API parsing (`paperdb-inspire-client`).** Model structs capture
unknown JSON fields into `extra` maps so new INSPIRE fields don't break
deserialization. Preserve this pattern when editing
`crates/paperdb-inspire-client/src/model.rs`.

## Error handling convention

Library crates (`paperdb-core`, `paperdb-manifest`, `paperdb-inspire-client`) use
`thiserror` enums for typed, matchable errors. The binary uses `anyhow` for
context-rich propagation; `main` prints `error: {:#}` and exits non-zero. When adding
a failure mode to a library, add a typed variant rather than a stringly-typed error.
