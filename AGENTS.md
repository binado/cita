# AGENTS.md

## What this is

Cita is a Git-friendly bibliography CLI. INSPIRE supplies complete BibTeX
entries, which Cita validates and stores canonically in `references.bib`. Cita
does not render bibliographic fields. Rust edition 2024, MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p cita-bibliography
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cita -- add 1207.7214
```

CI runs format, clippy-as-errors, test, and build on 1.88 and stable. Tests are
hermetic; INSPIRE and arXiv suites use local `TcpListener`s.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
cita-core
   ↑              ↑                    ↑
cita-bibliography cita-inspire-client  cita-documents
   └──────────────┴──────── cita ──────┘
```

- `cita-core`: `Locator` parsing plus arXiv and DOI normalization.
- `cita-bibliography`: owns
  `references.bib`, raw-entry preservation, semantic projections, identity
  validation, deterministic rendering, and atomic writes.
- `cita-inspire-client`: direct `format=bibtex` single and batched requests,
  bounded query chunks, and 429 retry handling.
- `cita-documents`: accepts a validated arXiv ID and atomically caches PDFs
  beneath `.cita/files/arxiv`.
- `cita`: CLI, parent discovery, sync reconciliation, and scoped Git commits.

## Key decisions

### Raw INSPIRE entries

Storage parses each source twice: `biblatex::RawBibliography` supplies byte spans
for complete raw entry blocks; semantic `biblatex::Bibliography` supplies title,
author, year, DOI, and eprint projections. Rendering sorts the `BTreeMap` by
texkey, joins raw blocks with one blank line, and appends one newline. Do not add
a handwritten BibTeX writer.

Only entries and whitespace are allowed. Reject directives, comments/non-entry
content, malformed or duplicate entries, missing titles, texkeys outside
`[A-Za-z0-9._:+-]+`, and duplicate normalized DOI/eprint identities.

### Atomic mutations

Add, remove, and sync validate a complete candidate map before writing. Writes
use `NamedTempFile` in the destination directory, `sync_all`, and `persist`.
Never partially update `references.bib` after a failed network response,
validation, selector, or reconciliation.

### INSPIRE sync

Batch at 100 texkeys or a 6 KiB encoded `q` value. Queries are direct searches:
`q=texkey:K1 or texkey:K2`, `format=bibtex`, and matching `size`. Batches run
sequentially. Retry 429 three times using `Retry-After`, otherwise five seconds.

Every local key must resolve exactly once and every returned entry must be
explained. If an obsolete key returns a new primary key, confirm it with a
single-key lookup, change only the returned key token back to the local key, and
warn. Multiple local keys resolving to one current record are an error.

### Selectors and documents

Exact texkey wins, then normalized DOI/eprint. Unmatched locators resolve through
INSPIRE. `fetch` and `open` may save transient entries and should direct stale
metadata warnings to `cita sync`. Documents use the parsed `eprint`; stored IDs
are versionless and cache paths retain legacy arXiv archive directories.

### Git

`cita commit` stages and commits only `references.bib`. Commit messages derive
added, removed, and modified texkeys and include parsed titles. If the HEAD blob
is unreadable, use `references: update bibliography`.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
