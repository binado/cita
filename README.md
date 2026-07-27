# cita

cita is a personal bibliography CLI. It keeps authoritative source snapshots in
named shelves inside one user-global store, resolves managed records through
INSPIRE, and materializes URL-enriched BibTeX files on demand.

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
- `cita shelf new <name>` (alias `create`) creates a named shelf.
- `cita shelf list` (alias `ls`) lists shelves and marks the fixed default.
- `cita add [--key K] [--overwrite] <locator>...` resolves INSPIRE JSON and
  authoritative BibTeX.
- `cita import [--overwrite] <path|->` atomically imports standalone BibTeX
  entries from a caller-relative path or stdin.
- `cita remove <selector>...` removes a batch atomically.
- `cita list [--sort-by key|title|author|year] [--order asc|desc]` displays
  source-neutral projections.
- `cita sync` refreshes INSPIRE snapshots by stable record ID and leaves imports
  unchanged.
- `cita export [OUTPUT]` renders URL-enriched BibTeX. Without `OUTPUT`, it writes
  `<shelf>.bib` in the current directory. Relative destinations use the caller's
  directory, and destinations inside the global store are rejected.
- `cita export --all-shelves [DIRECTORY]` writes `<name>.bib` for every shelf
  into an existing directory, defaulting to the current directory.
- `cita fetch [--force | --cache-only | --url] [--source] [--open] [--save]
  <selector>` returns a cached arXiv PDF/source path or URL.
- `cita completions <shell>` prints shell completions.

Reference commands accept `-s/--shelf <name>` and otherwise use `main`.
`sync` and `export` additionally accept `--all-shelves`; batches run in name
order, continue after shelf-specific failures, and fail overall if any shelf
fails. An unknown shelf is never created implicitly.

## Storage

`CITA_HOME` may override the store with an absolute path. Otherwise the layout
is:

```text
$HOME/.cita/
├── library.toml
├── shelves/
│   ├── main/shelf.toml
│   └── <name>/shelf.toml
├── files/
└── locks/
```

Each `shelf.toml` is the sole authority for that shelf. Its sorted local keys
hold either an exact imported BibTeX snapshot or an INSPIRE snapshot with its
stable record ID, timestamp, and curated identifiers. DOI, arXiv, and provider
identities must be unique within a shelf.

Exports are deterministic, one-way artifacts. Entries are sorted and re-keyed
by local key, separated by one blank line, and terminated by one newline. Cita
adds an arXiv PDF `url` when an entry has an arXiv identifier and no authored
URL. Exports are never read back or verified.

Shelf mutations validate complete candidates and atomically replace only
`shelf.toml`. Cross-process advisory locks prevent lost updates. PDFs and source
packages use the shared `files/` cache, so the same arXiv artifact is reused
across shelves.

### Migrating pre-global projects

Legacy local `cita.toml`, `cita-library.toml`, and `references.bib` files are
ignored. Import a legacy bibliography explicitly:

```bash
cita shelf new paper
cita import -s paper /old/project/references.bib
```

This preserves BibTeX entries and local keys as imported snapshots, but does not
preserve INSPIRE record IDs or managed-refresh metadata.

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
