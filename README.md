# cita

cita is a Git-friendly bibliography CLI. It keeps authoritative source snapshots
in `cita.toml` and deterministically generates the tracked `references.bib`.
INSPIRE records retain typed JSON metadata plus INSPIRE's exact BibTeX; imported
standalone BibTeX entries retain their exact source bytes.

Requires Rust 1.88 or newer.

## Quick start

```bash
cita init
cita add 1207.7214 doi:10.1016/j.physletb.2012.08.020
cita import local-references.bib
cita list
cita sync
cita fetch 1207.7214
cita commit
```

Supported locators are bare arXiv IDs and explicit `arxiv:`, `doi:`, or
`inspire:` locators. Selectors first match an exact local citation key, then a
provider ID, normalized DOI, or normalized arXiv ID.

## Commands

- `cita init` creates an empty schema-1 project. If only `references.bib`
  exists, it imports every standalone entry. Existing schema-1 projects are
  validated; any other schema is explicitly unsupported.
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
- `cita commit` validates consistency and commits only `cita.toml` and
  `references.bib`, leaving unrelated staged changes intact.

Successful `fetch` output is suitable for command substitution; status messages
are written to stderr. For example, choose a specific PDF viewer on macOS with
`open -a Skim "$(cita fetch <selector>)"`.

## Storage rules

`cita.toml` is the sole authority. Each sorted local key contains one tagged
source snapshot (`inspire` or `bibtex`). Snapshots are strictly validated and
duplicate normalized DOI, arXiv, or provider identities are rejected across all
sources.

`references.bib` behaves like a lockfile: entries are sorted by local key,
separated by one blank line, and end with one newline. Preserved field bytes are
unchanged; only the citation-key token may be re-keyed. Every normal command
checks its exact bytes against the manifest and reports drift. Use `cita
generate` to repair it.

Mutations validate and render the complete candidate in memory, atomically
persist `references.bib` first, and persist `cita.toml` as the commit point.

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
