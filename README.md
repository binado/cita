# cita

cita is a Git-friendly bibliography CLI. It keeps authoritative source snapshots
in `cita.toml` and deterministically generates the tracked `references.bib`.
INSPIRE is the managed metadata provider: its records retain curated identifiers
plus INSPIRE's exact BibTeX and can be refreshed by stable record ID. Generic
BibTeX ingestion is available through `cita import`, which preserves each
standalone entry's exact source bytes.

Requires Rust 1.88 or newer.

```bash
cargo install cita
```

## Quick start

```bash
cita init
cita add 1207.7214 doi:10.1016/j.physletb.2012.08.020
cita import local-references.bib
cita list
cita sync
cita fetch 1207.7214
```

Supported locators are bare arXiv IDs and explicit `arxiv:`, `doi:`, or
`inspire:` locators. Selectors first match an exact local citation key, then a
provider ID, normalized DOI, or normalized arXiv ID.

## Commands

- `cita init [--here]` creates an empty schema-1 project. By default it
  initializes at the enclosing Git repository root; outside Git it uses the
  current directory. `--here` always uses the current directory, allowing a
  nested Cita project. Commands run inside a nested project discover its
  nearest `cita.toml`. If only `references.bib` exists at the chosen location,
  initialization imports every standalone entry. Existing schema-1 projects
  are validated; any other schema is explicitly unsupported.
- `cita import <path|->` atomically imports all standalone entries from a file
  or stdin.
- `cita add [--key K] <locator>...` resolves INSPIRE JSON and authoritative
  BibTeX. `--key` keeps an independent local key and accepts one locator.
- `cita sync` refreshes only INSPIRE snapshots by stable record ID and leaves
  imported entries byte-for-byte unchanged.
- `cita remove <selector>...` removes a batch atomically.
- `cita list [--sort-by key|title|author|year] [--order asc|desc]` displays
  source-neutral projections.
- `cita generate` repairs a missing or edited `references.bib` from the
  authoritative manifest.
- `cita fetch [--force | --cache-only | --url] [--open] [--save] <selector>`
  returns an absolute cached PDF path, or the arXiv PDF URL with `-u/--url`.
  `--open` launches the returned target with the system default application.
  Without `--save`, an unmatched locator uses INSPIRE JSON only.
- `cita commit` is an optional Git helper. It validates consistency and commits
  only `cita.toml` and `references.bib`, leaving unrelated staged changes
  intact. It refuses to run if either managed file is already staged.

Successful `fetch` output is suitable for command substitution; status messages
are written to stderr. For example, choose a specific PDF viewer on macOS with
`open -a Skim "$(cita fetch <selector>)"`.

## Storage rules

`cita.toml` is the sole authority. Each sorted local key contains one tagged
source snapshot (`inspire` or `import`). Snapshots are strictly validated and
duplicate normalized DOI, arXiv, or provider identities are rejected across all
sources.

The projected `Reference` intentionally contains only the fields Cita needs for
selection and display: title, authors, collaborations, year, and DOI/arXiv/
provider identifiers. The authoritative BibTeX remains available in the source
snapshot for all other bibliographic data.

`references.bib` behaves like a lockfile: entries are sorted by local key,
separated by one blank line, and end with one newline. Preserved field bytes are
unchanged; only the citation-key token may be re-keyed. Every normal command
checks its exact bytes against the manifest and reports drift. Use `cita
generate` to repair it.

Mutations validate and render the complete candidate in memory, atomically
persist `references.bib` first, and persist `cita.toml` as the commit point.
Downloaded documents live under `.cita/files`; initialization adds
`/.cita/files/` to the project root's `.gitignore` so the cache is not tracked.

## Workspace

- `cita-core`: locators, neutral `Reference` vocabulary, and provider traits.
- `cita-bibliography`: standalone BibTeX snapshot projection, raw-entry
  preservation, re-keying, and `biblatex`-based generic rendering.
- `cita-inspire-client`: typed INSPIRE JSON/BibTeX snapshots and stable-ID
  refreshes.
- `cita-manifest`: schema-1 validation, identity indexes, deterministic TOML,
  output verification, and coordinated writes.
- `cita-documents`: validated arXiv PDF downloads and atomic caching.
- `cita`: CLI wiring, discovery, selectors, and scoped Git commits.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Provider and document tests are hermetic and use local TCP listeners.

## Publishing

All six crates share version 0.1.0. The five library APIs are intentionally
unstable throughout 0.x. After the release checks pass, publish manually in
dependency order:

1. `cita-core`
2. `cita-bibliography` and `cita-documents`
3. `cita-inspire-client`
4. `cita-manifest`
5. `cita`

Immediately before publishing, recheck that all six crate names are available.
After publishing, smoke-test with
`cargo install cita --version 0.1.0 --locked`.
