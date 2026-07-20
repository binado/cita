# AGENTS.md

## What this is

Cita is a Git-friendly bibliography CLI. Authoritative BibTeX (plus curated
INSPIRE identifiers) lives in schema-1 `cita.toml`; `references.bib` is a
deterministic, tracked generated artifact. Rust edition 2024, MSRV 1.88.

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
```

CI runs format, clippy-as-errors, test, and build on 1.88 and stable. Tests are
hermetic; INSPIRE and arXiv suites use local `TcpListener`s.

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
  raw-entry re-keying, and generic `biblatex::Entry` rendering.
- `cita-inspire-client`: lean `InspireRecord`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; cross-checks its BibTeX against the
  selected JSON through `cita-bibliography`.
- `cita-manifest`: schema-1 authority, identity indexes, deterministic TOML,
  generated bibliography verification, and coordinated writes.
- `cita-documents`: accepts a validated arXiv ID and atomically caches PDFs
  beneath `.cita/files/arxiv`.
- `cita`: CLI, parent discovery, sync reconciliation, and scoped Git commits.

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

### Atomic mutations

Add, import, remove, and sync validate a complete candidate before writing.
Persist and sync `references.bib` first, then persist and sync `cita.toml` as the
commit point. The old manifest remains authoritative after an interrupted
second write, and `cita generate` repairs detectable drift.

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
Transient `fetch` and `open` resolve INSPIRE JSON only unless `--save` also
fetches the authoritative BibTeX and stores the record. Documents use the
projected arXiv ID; stored IDs are versionless and cache paths retain legacy
arXiv archive directories.

### Git

`cita commit` validates consistency and stages only `cita.toml` and
`references.bib`. Commit messages derive added, removed, and modified local keys
and projected titles. If the HEAD manifest exists but is unreadable, warn on
stderr and use `references: update bibliography`. A path missing from HEAD is
expected absence; any other git failure is an error, never a first commit.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
