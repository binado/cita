# AGENTS.md

## What this is

cita is a personal bibliography CLI backed by one user-global library.
Authoritative source snapshots live in schema-1
`$CITA_HOME/shelves/<name>/shelf.toml`; `$CITA_HOME` defaults to
`$HOME/.cita`. BibTeX is materialized only through `cita export`. Rust edition
2024, MSRV 1.88.

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
        cita-manifest ─────────────────────┤
              └──────── cita ──────────────┘
```

- `cita-core`: `Reference`, provider traits, locators, and normalization.
- `cita-bibliography`: strict BibTeX snapshots, semantic projection, raw-entry
  re-keying, and field insertion.
- `cita-inspire-client`: lean `InspireSnapshot`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; attaches BibTeX only after
  cross-checking it against the slim API JSON model through `cita-bibliography`.
- `cita-manifest`: global schema-1 registry, shelf authority, identity indexes,
  deterministic TOML/rendering, atomic writes, and advisory locks.
- `cita-documents`: validated arXiv PDF/source downloads and safe extraction.
- `cita`: global path resolution, command routing, sync, export policy, and
  shared-cache selection.

## Key decisions

### Global library and shelves

There is one library at `$CITA_HOME`, defaulting to `$HOME/.cita`.
`library.toml` stores sorted shelf names and the fixed `main` default. Shelf
paths are computed as `shelves/<name>/shelf.toml`; there are no configurable
paths, project manifests, or ancestor discovery.

Every data command opens or lazily creates the library and `main`, recreating a
deleted `main` directory or manifest on every open. An existing manifest is never
parsed during that repair, so a corrupt `main` cannot fail unrelated commands.
`cita init` is an optional idempotent eager initializer that reports whether it
created, repaired, or found the store intact. An explicit unknown shelf is an
error and is never created by selection.

`cita shelf new` validates the whole candidate — including the case-alias check,
scoped to the new name only — before creating anything, so a rejection leaves no
orphan directory and a damaged unrelated shelf never blocks creation.

Shelf names are a validated `ShelfName` newtype; the leading-alphanumeric rule is
what makes `shelves/<name>` traversal-safe, and every path built from a name
depends on it.

The registry is protected by `locks/registry.lock`. Add, import, remove, sync,
and `fetch --save` hold an exclusive `locks/shelf-<name>.lock` across load and
atomic manifest replacement; the `shelf-` prefix keeps the two namespaces
disjoint, so no shelf name is reserved. Load manifests through
`ShelfLock::manifest` so each mutation is paired with the lock covering it. Locks
use `flock` and are not reentrant. Batch sync and export run shelves in name
order, continue after failures, report successes on stdout and failures plus an
aggregate summary on stderr, and return failure if any shelf failed.

### Source snapshots and raw entries

`shelf.toml` is the sole authority. Every source stores standalone BibTeX and
projects its `Reference` from that BibTeX. INSPIRE entries additionally store
their stable `record_id`, `updated` timestamp, and curated canonical arXiv/DOI
identifiers. Imports derive identity from their exact entry.

BibTeX parsing uses raw spans plus semantic `biblatex` parsing. Rendering sorts
by local key, changes only the raw key token, joins entries with one blank line,
and appends one newline. Do not add a handwritten writer.

Only entries and whitespace are accepted. Reject directives, comments or other
non-entry content, malformed or duplicate entries, missing titles, invalid
texkeys, and duplicate normalized DOI/eprint/provider identities within a
shelf.

### Exports

There is no managed `references.bib` and no `generate` command. `cita export`
is a pure function of one shelf manifest: no network and no cache probing. It
adds an arXiv PDF URL through `cita-bibliography::insert_field` only when the
entry has no authored URL. Repeated exports are byte-stable.

The optional positional destination is caller-relative and defaults to
`<shelf>.bib`. All-shelf export accepts an existing destination directory and
writes `<name>.bib`. Export files are untracked, unverified, never read back,
and cannot be written inside or through a symlink into `$CITA_HOME`.

### INSPIRE sync

Refresh by stable INSPIRE record ID. Batch at 100 records or a 6 KiB encoded
query. Fetch JSON and BibTeX sequentially, match raw entries exactly through
returned texkeys, and explain every requested and returned record. Retry 429
three times using capped `Retry-After`, reporting retries on stderr. Imported
snapshots cause no network request.

### Selectors and documents

Exact local key wins, followed by provider ID, normalized DOI, and normalized
arXiv ID. `fetch` resolves transient INSPIRE JSON unless `--save` also stores
authoritative BibTeX. It returns an absolute shared-cache path or URL;
`--source` and `--url` conflict, and `--open` launches the result.

Documents live under `$CITA_HOME/files/arxiv` and are shared across shelves.
Stored arXiv IDs are versionless and cache paths retain legacy archive
directories.

### Compatibility

Legacy `cita.toml`, `cita-library.toml`, and `references.bib` files are ignored.
Migration is explicit BibTeX import and loses INSPIRE refresh metadata:

```bash
cita shelf new paper
cita import -s paper /old/project/references.bib
```

Git hooks, project links, store Git tooling, shelf rename/delete, and
configurable defaults are deferred.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new persistence failure modes.
