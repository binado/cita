# cita

cita is a personal bibliography CLI. It keeps authoritative source snapshots in
one user-global SQLite library, shares references across named shelves, resolves
managed records through INSPIRE, and materializes exports on demand.

Requires Rust 1.88 or newer.

```bash
cargo install cita
```

## Quick start

The first data command initializes `$HOME/.cita` and its default `main` shelf.
`cita init` can initialize it explicitly.

```bash
cita add https://arxiv.org/abs/1207.7214
cita import local-references.bib
cita list
cita sync
cita export references.bib
```

Use additional shelves for independent collections:

```bash
cita shelf new paper
cita add -s paper 1207.7214
cita list -s paper
cita export -s paper references.bib
cita sync --all-shelves
```

Supported locators are bare arXiv IDs; explicit `arxiv:`, `doi:`, or `inspire:`
locators; and canonical arXiv, INSPIRE, or DOI URLs. Selectors match an exact
local key first, then a provider ID, normalized DOI, or normalized arXiv ID.

## Commands

- `cita init` idempotently initializes the global store and `main` shelf.
- `cita init --from-file <library.{json,toml}> [--overwrite]` initializes or
  replaces the database from a lossless export.
- `cita shelf new <name>` (alias `create`) creates a named shelf.
- `cita shelf list` (alias `ls`) lists shelves and marks the fixed default.
- `cita add [--key K] [--overwrite] <locator>...` resolves INSPIRE JSON and
  authoritative BibTeX.
- `cita import [--overwrite] [--skip-errors] <path|->` atomically imports
  standalone BibTeX entries, canonicalizing DOI/arXiv matches through INSPIRE.
- `cita remove <selector>...` removes a batch atomically.
- `cita list [--sort-by key|title|author|year] [--order asc|desc]` displays
  source-neutral projections.
- `cita sync` refreshes INSPIRE snapshots by stable record ID and retries
  canonicalizing imported references.
- `cita export [--format bib|json|toml] [OUTPUT]` renders BibTeX or a lossless
  shelf document. Without `OUTPUT`, it writes `<shelf>.<format>`.
- `cita export --all-shelves [DIRECTORY]` writes `<name>.bib` for every shelf
  into an existing directory. With JSON or TOML it writes one complete-library
  document, defaulting to `cita.<format>`.
- `cita fetch [--force | --cache-only | --url] [--source] [--open] [--save]
  <selector>` returns a cached arXiv PDF/source path or URL.
- `cita completions <shell>` prints shell completions.

Reference commands accept `-s/--shelf <name>` and otherwise use `main`.
`sync` and `export` additionally accept `--all-shelves`. Sync deduplicates
shared references and applies the selected result set atomically. An unknown
shelf is never created implicitly.

## Storage

`CITA_HOME` may override the store with an absolute path. Otherwise the layout
is:

```text
$HOME/.cita/
├── library.sqlite3
└── files/
```

`library.sqlite3` is the sole authority. References are global and shelves hold
memberships with shelf-local citation keys, so syncing a shared reference affects
every shelf that contains it. DOI, arXiv, and provider identities are globally
unique. Exact UTF-8 BibTeX is stored alongside its structured title, contributor,
year, identity, and provider projection.

BibTeX exports are deterministic citation projections. Entries are sorted and
re-keyed by local key, separated by one blank line, and terminated by one
newline. Cita adds an arXiv PDF `url` when an entry has an arXiv identifier and
no authored URL. JSON and TOML exports are versioned, lossless interchange
documents accepted by `cita init --from-file`.

SQLite transactions and WAL coordination prevent lost updates across cita
processes. Network access occurs before write transactions, and sync results are
revalidated before commit. PDFs and source packages remain in the shared
`files/` cache.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

All regular tests are hermetic. The ignored `e2e` test reaches the live INSPIRE
API.

All six crates share one version and their 0.x APIs may change between minor
releases. Publishing is automated with release-plz.
