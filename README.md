# Cita

Cita is a Git-friendly bibliography CLI for high-energy physics. INSPIRE supplies
complete BibTeX entries and Cita stores them in a canonical, committed
`references.bib`. Cita rearranges and validates entries, but never rewrites their
bibliographic fields.

Requires Rust 1.88 or newer.

## Quick start

```bash
cita init
cita add 1207.7214 doi:10.1016/j.physletb.2012.08.020
cita list
cita sync
cita fetch 1207.7214
cita commit
```

Supported locators are bare arXiv IDs and explicit `arxiv:`, `doi:`, or
`inspire:` locators. Selectors used by `remove`, `fetch`, and `open` first match
an exact INSPIRE texkey, then a normalized DOI or arXiv eprint. An unmatched
locator is resolved transiently through INSPIRE; pass `--save` to `fetch` or
`open` to retain it.

## Commands

- `cita init` creates or validates `references.bib` at the Git root and prepares
  the ignored `.cita/files` PDF cache. Legacy `cita.toml` files are rejected;
  there is no automatic migration.
- `cita add <locator>...` fetches one INSPIRE BibTeX entry per locator and adds
  the entire batch atomically.
- `cita sync` refreshes every entry in sequential batched INSPIRE searches.
  Obsolete local texkeys are preserved after a confirming single-key lookup.
- `cita remove <selector>...` removes a batch atomically.
- `cita list [--sort-by key|title|author|year] [--order asc|desc]` displays
  semantic projections parsed from BibTeX.
- `cita fetch [--force] [--dry-run | --save] <selector>` manages the arXiv PDF
  cache.
- `cita open [--force | --no-download | --browser] [--save] <selector>` opens a
  cached/downloaded PDF or its arXiv URL.
- `cita commit` commits only `references.bib`, leaving unrelated staged changes
  intact.

`references.bib` is already the export; the old `cita export --bibtex`, `add
--key`, and `add --force` interfaces no longer exist.

## Storage rules

Only complete BibTeX entries and whitespace are accepted. Comments, string or
preamble directives, arbitrary text, malformed or duplicate entries, missing
titles, unsafe texkeys, and duplicate normalized DOI/eprint identities are
rejected. Entries are sorted by texkey, separated by one blank line, and the
file ends with one newline. Writes use a same-directory temporary file and an
atomic rename.

## Workspace

- `cita-core`: locators and identifier normalization.
- `cita-bibliography`: canonical raw
  BibTeX storage and semantic projections using `biblatex`.
- `cita-inspire-client`: direct single and batched INSPIRE BibTeX requests.
- `cita-documents`: validated arXiv PDF downloads and atomic caching.
- `cita`: CLI wiring, discovery, reconciliation, and scoped Git commits.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The test suite is hermetic and uses local TCP listeners instead of INSPIRE or
arXiv.
