# AGENTS.md

## What this is

cita is a personal bibliography CLI backed by one user-global library.
Authoritative source snapshots live in `$CITA_HOME/library.sqlite3`;
`$CITA_HOME` defaults to `$HOME/.cita`. References are global and shelves hold
memberships with local citation keys. BibTeX is materialized only through
`cita export`. Rust edition 2024, MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p cita-bibliography
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cita -- init
cargo run -p cita -- shelf new paper
cargo run -p cita -- add -s paper 1207.7214
cargo run -p cita -- import -s paper local.bib
cargo run -p cita -- export -s paper references.bib
cargo run -p cita -- sync --all-shelves
cargo test --test e2e -- --ignored  # live INSPIRE, network required
```

CI runs format, Clippy-as-errors, tests, and builds on 1.88 and stable. Regular
tests are hermetic and assign temporary `CITA_HOME` roots. INSPIRE and arXiv
suites use local `TcpListener`s. `crates/cita/tests/e2e.rs` is the ignored live
INSPIRE liveness test.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
cita-core
   ↑                ↑                      ↑
cita-bibliography ← cita-inspire-client   cita-documents
   └──────────┬────────┘                   │
          cita-store ──────────────────────┤
              └──────── cita ──────────────┘
```

- `cita-core`: `Reference`, provider traits, locators, and normalization.
- `cita-bibliography`: strict BibTeX snapshots, semantic projection, raw-entry
  re-keying, and field insertion.
- `cita-inspire-client`: lean `InspireSnapshot`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; attaches BibTeX only after
  cross-checking it against the slim API JSON model through `cita-bibliography`.
- `cita-store`: SQLite authority, global identity indexes, shelf memberships,
  transactions, and deterministic lossless interchange.
- `cita-documents`: validated arXiv PDF/source downloads and safe extraction.
- `cita`: global path resolution, command routing, sync, export policy, and
  shared-cache selection.

## Key decisions

### Global library and shelves

There is one library at `$CITA_HOME`, defaulting to `$HOME/.cita`.
`library.sqlite3` stores global references, shelves, and memberships; there are
no configurable paths, project manifests, or ancestor discovery. Every data
command lazily creates the database and fixed `main` shelf. `cita init` is an
optional idempotent eager initializer, and `init --from-file` accepts lossless
JSON/TOML exports. An explicit unknown shelf is always an error.

Shelf names remain validated `ShelfName` values and are unique
case-insensitively. SQLite WAL transactions replace advisory lock files. Network
calls happen before `BEGIN IMMEDIATE`; identities and source preconditions are
revalidated inside the transaction.

### Source snapshots and raw entries

The database is the sole authority. Every global reference stores standalone
BibTeX as exact UTF-8 `TEXT` plus a structured projection of title, year,
contributors, and normalized identities. INSPIRE entries additionally store
their stable `record_id`, `updated` timestamp, and curated canonical arXiv/DOI
identifiers. Shelves hold memberships and local citation keys.

BibTeX parsing uses raw spans plus semantic `biblatex` parsing. Rendering sorts
by local key, changes only the raw key token, joins entries with one blank line,
and appends one newline. Do not add a handwritten writer.

Only entries and whitespace are accepted. Reject directives, comments or other
non-entry content, malformed or duplicate entries, missing titles, invalid
texkeys, and globally duplicate normalized DOI/eprint/provider identities.

### Exports

There is no managed `references.bib` and no `generate` command. BibTeX export is
a pure database projection that adds an arXiv PDF URL only when the entry has no
authored URL. JSON/TOML exports are versioned lossless interchange documents.
Single-shelf output defaults to `<shelf>.<format>`; all-shelf BibTeX writes one
file per shelf, while all-shelf JSON/TOML writes one library document. Exports
cannot target `$CITA_HOME`.

### INSPIRE sync

Refresh by stable INSPIRE record ID. Batch at 100 records or a 6 KiB encoded
query. Fetch JSON and BibTeX sequentially, match raw entries exactly through
returned texkeys, and explain every requested and returned record. Retry 429
three times using capped `Retry-After`. Sync also retries canonicalizing imports
through arXiv then DOI identities. Fetch first and apply the selected unique
global reference set atomically; shared shelves observe the same update.

### Selectors and documents

Exact local key wins, followed by provider ID, normalized DOI, and normalized
arXiv ID. `fetch` resolves transient INSPIRE JSON unless `--save` also stores
authoritative BibTeX. It returns an absolute shared-cache path or URL;
`--source` and `--url` conflict, and `--open` launches the result.

Documents live under `$CITA_HOME/files/arxiv` and are shared across shelves.
Stored arXiv IDs are versionless and cache paths retain legacy archive
directories.

### Compatibility

Legacy manifests are ignored and there is no automatic migration or downgrade
support. Git hooks, project links, store Git tooling, shelf rename/delete, and
configurable defaults are deferred.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new persistence failure modes.
