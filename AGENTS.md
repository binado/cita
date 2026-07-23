# AGENTS.md

## What this is

cita is a Git-friendly bibliography CLI. Authoritative BibTeX (plus curated
INSPIRE identifiers) lives in schema-1 `cita.toml`; `references.bib` is a
deterministic, tracked generated artifact. A schema-1 `cita-library.toml` can
register multiple independent shelf projects by stable name and safe relative
path. Rust edition 2024, MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p cita-bibliography
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cita -- add 1207.7214
cargo run -p cita -- import local.bib
cargo run -p cita -- generate
cargo run -p cita -- export
cargo run -p cita -- library shelves
cargo test --test e2e -- --ignored  # live INSPIRE, network required
```

CI runs format, clippy-as-errors, test, and build on 1.88 and stable. Tests are
hermetic; INSPIRE and arXiv suites use local `TcpListener`s. The exception is
`crates/cita/tests/e2e.rs`, a single `#[ignore]`d end-to-end test that hits the
real INSPIRE API; CI runs it on every push and pull request, plus a weekly
schedule as a standalone liveness check.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
cita-core
   ↑                ↑                      ↑
cita-bibliography ← cita-inspire-client   cita-documents
   └──────────┬────────┘                   │
        cita-manifest ─────────────────────┤
              └──────── cita ──────────────┘
```

- `cita-core`: `Reference`, provider traits, locators, and normalization.
- `cita-bibliography`: strict standalone BibTeX snapshots, projections,
  and raw-entry re-keying and field insertion.
- `cita-inspire-client`: lean `InspireRecord`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; cross-checks its BibTeX against the
  selected JSON through `cita-bibliography`.
- `cita-manifest`: schema-1 shelf authority, library registry/path validation,
  identity indexes, deterministic TOML, generated bibliography verification,
  and coordinated writes.
- `cita-documents`: accepts a validated arXiv ID and atomically caches PDFs and
  safely extracts gzip-compressed TeX source packages beneath `.cita/files/arxiv`.
- `cita`: CLI, parent discovery, sync reconciliation, derived-export policy, and
  scoped Git commits.

## Key decisions

### Source snapshots and raw entries

`cita.toml` snapshots are authoritative; projections are derived. Every source
stores authoritative standalone BibTeX and its `Reference` (title, authors, year,
publication, arXiv/DOI) is projected from that BibTeX. INSPIRE entries are tagged
`source = "inspire"` and additionally carry the refresh key (`record_id`), an
`updated` timestamp, and a curated `identifiers` block (canonical normalized
arXiv/DOI) that overrides the projected identity; imports are tagged
`source = "import"` and derive identity from their entry. BibTeX parsing uses raw
spans plus semantic `biblatex` parsing. Rendering sorts by local key, changes
only the raw key token, joins entries with one blank line, and appends one
newline. Do not add a handwritten writer.

Only entries and whitespace are allowed. Reject directives, comments/non-entry
content, malformed or duplicate entries, missing titles, texkeys outside
`[A-Za-z0-9._:+-]+`, and duplicate normalized DOI/eprint identities.

### Derived exports

`cita export` writes a separate artifact and never touches `references.bib`.
Layout stays in `cita-manifest::Manifest::render_derived`, so every rendered
bibliography shares one set of rules; only field-level policy lives in the CLI.
Fields are added through `cita-bibliography::insert_field`, which splices after
an entry's last field using scanner-owned spans — inserting before the closing
brace would break entries carrying a trailing inline comment. An entry that
already defines the field is returned unchanged, which keeps authored values and
makes repeated exports byte-stable.

Exports are pure functions of `cita.toml`: no network, no cache probing, no
machine-specific paths. They are untracked, unverified, never read back, and
refuse to run against bibliography drift or to overwrite a managed file. Shelf
exports are named for the registered shelf name, not the shelf directory.

### Atomic mutations

Add, import, remove, and sync validate a complete candidate before writing.
Persist and sync `references.bib` first, then persist and sync `cita.toml` as the
commit point. The old manifest remains authoritative after an interrupted
second write, and `cita generate` repairs detectable drift.

Library registration is a separate atomic write. Shelf initialization completes
before registration, so a failed registry write leaves a valid standalone shelf.
Library-wide operations run shelves in name order and continue after failures;
there is no crash-atomic transaction across shelves.

### Libraries and shelves

Each registered shelf is an independent cita project with its own manifest,
bibliography, identities, cache, and Git commits. `cita-library.toml` only maps
stable names to library-relative paths. Paths cannot escape the root, overlap,
nest, or alias through symlinks. The library root cannot itself contain
`cita.toml` or `references.bib`. There is no aggregate bibliography, shared
cache, cross-shelf uniqueness, or library-wide commit.

### INSPIRE sync

Refresh by stable INSPIRE record ID. Batch at 100 records or a 6 KiB encoded `q`
value. Fetch JSON and BibTeX searches sequentially and match each raw entry to
exactly one JSON record through returned texkeys. Retry 429 three times using
`Retry-After` capped at sixty seconds, otherwise five seconds, and report each
retry on stderr.

Every managed record and returned result must be explained. Local citation keys
are independent from provider texkeys and never change during refresh. Imported
BibTeX snapshots cause no network request.

### Selectors and documents

Exact local key wins, then provider ID, normalized DOI, and normalized arXiv ID.
Transient `fetch` resolves INSPIRE JSON only unless `--save` also fetches the
authoritative BibTeX and stores the record. It returns either an absolute cached
PDF path, an absolute extracted source directory with `--source`, or, with
`--url`, the arXiv PDF URL; `--source` and `--url` are mutually exclusive;
`--open` launches that target. Documents use the projected arXiv ID; stored IDs
are versionless and cache paths retain legacy arXiv archive directories.

### Git

`cita commit` validates consistency and stages only `cita.toml` and
`references.bib`. Commit messages derive added, removed, and modified local keys
and projected titles. If the HEAD manifest exists but is unreadable, warn on
stderr and use `references: update bibliography`. A path missing from HEAD is
expected absence; any other git failure is an error, never a first commit.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
