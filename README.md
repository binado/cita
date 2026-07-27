# bibi

bibi is a Git-friendly bibliography CLI. It keeps authoritative source snapshots
in `cita.toml` and deterministically generates the tracked `references.bib`.
INSPIRE is the managed metadata provider: its records retain canonical
identifiers selected from and cross-checked against INSPIRE's exact BibTeX and
can be refreshed by stable record ID. Generic BibTeX ingestion is available
through `bibi import`, which preserves each standalone entry's exact source
bytes.

Requires Rust 1.88 or newer.

```bash
cargo install bibi
```

## Quick start

```bash
bibi add https://arxiv.org/abs/1207.7214 doi:10.1016/j.physletb.2012.08.020
bibi import local-references.bib
bibi list
bibi sync
bibi fetch 1207.7214
```

Supported locators are bare arXiv IDs; explicit `arxiv:`, `doi:`, or `inspire:`
locators; and canonical `https://arxiv.org`, `https://inspirehep.net`, or
`https://doi.org` URLs. Selectors first match an exact local citation key, then
a provider ID, normalized DOI, or normalized arXiv ID. The same canonical URLs
work as selectors.

## Commands

- `bibi import <path|->` atomically imports all standalone entries from a file
  or stdin.
- `bibi add [--key K] <locator>...` resolves INSPIRE JSON and authoritative
  BibTeX. `--key` keeps an independent local key and accepts one locator.
- `bibi sync` refreshes only INSPIRE snapshots by stable record ID and leaves
  imported entries byte-for-byte unchanged.
- `bibi remove <selector>...` removes a batch atomically.
- `bibi list [--sort-by key|title|author|year] [--order asc|desc]` displays
  source-neutral projections.
- `bibi export [-o/--output <file>]` writes a derived BibTeX file for tools that
  want a resolvable link, such as Zotero. It renders the same entries as
  `references.bib` and adds `url = {https://arxiv.org/pdf/<id>}` to each entry
  with an arXiv ID, leaving entries that already define a `url` untouched. The
  default file is named for the project directory; a relative `--output` is
  relative to the directory where bibi was invoked. The export never overwrites
  a managed file — not this project's, and not a `cita.toml` or `references.bib`
  belonging to any other project — and it refuses to run while `references.bib`
  has drifted.
- `bibi fetch [--force | --cache-only | --url] [--source] [--open] [--save]
  <selector>` returns an absolute cached PDF path by default, the arXiv PDF URL
  with `-u/--url`, or an absolute extracted source directory with `--source`.
  `--source` and `--url` are mutually exclusive. `--open` launches the returned
  target with the system default application. Without `--save`, an unmatched
  locator uses INSPIRE JSON only.
- `bibi completions <bash|elvish|fish|powershell|zsh>` prints a shell completion
  script to stdout, e.g. `bibi completions zsh > ~/.zfunc/_bibi`.

Successful `fetch` output is suitable for command substitution; status messages
are written to stderr. For example, choose a specific PDF viewer on macOS with
`open -a Skim "$(bibi fetch <selector>)"`.

## Storage rules

`cita.toml` is the sole authority. Each sorted local key contains one tagged
source snapshot (`inspire` or `import`). Snapshots are strictly validated and
duplicate normalized DOI, arXiv, or provider identities are rejected across all
sources.

The projected `Reference` intentionally contains only the fields Bibi needs for
selection and display: title, authors, collaborations, year, and DOI/arXiv/
provider identifiers. The authoritative BibTeX remains available in the source
snapshot for all other bibliographic data.

`references.bib` behaves like a lockfile: entries are sorted by local key,
separated by one blank line, and end with one newline. Preserved field bytes are
unchanged; only the citation-key token may be re-keyed. Every normal command
checks its exact bytes against the manifest and reports drift. Use `bibi
generate` to repair it.

The `bibi export` output is a derived, one-way convenience artifact. It is never
authoritative, is not tracked or verified, and is not read back by any command;
regenerate it instead of editing it, and add it to `.gitignore` if you do not
want it tracked. Re-importing an export into Zotero adds items again rather than
updating the previous import.

Mutations validate and render the complete candidate in memory, atomically
persist `references.bib` first, and persist `cita.toml` as the commit point.
Downloaded PDFs and extracted source packages live under `.bibi/files`, and
`/.bibi/files/` is added to the project root's `.gitignore` so the cache is not
tracked.

## Workspace

- `bibi-core`: locators, neutral `Reference` vocabulary, and provider traits.
- `bibi-bibliography`: standalone BibTeX snapshot projection, raw-entry
  preservation, re-keying, and `biblatex`-based generic rendering.
- `bibi-inspire-client`: typed INSPIRE JSON metadata, authoritative BibTeX
  snapshots, and stable-ID refreshes.
- `bibi-manifest`: schema-1 project validation, identity indexes, deterministic
  TOML, output verification, and coordinated writes.
- `bibi-documents`: validated arXiv PDF/source downloads, safe source
  extraction, and atomic caching.
- `bibi`: CLI wiring, discovery, and selectors.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Provider and document tests are hermetic and use local TCP listeners.

## Publishing

All six crates share one version and their APIs are intentionally unstable
throughout 0.x. Releases are automated with
[release-plz](https://release-plz.dev) (`release-plz.toml`,
`.github/workflows/release-plz.yml`):

1. Merge Conventional-Commit PRs to `main`.
2. release-plz opens (or updates) a "release PR" that bumps the shared version
   and updates every crate's `CHANGELOG.md`.
3. Merging that release PR publishes all six crates in dependency order and tags
   them.

After the first release, smoke-test with `cargo install bibi --locked`.

As an emergency fallback, the crates can still be published by hand in
dependency order: `bibi-core`, then `bibi-bibliography` and `bibi-documents`,
then `bibi-inspire-client`, then `bibi-manifest`, and finally `bibi`.
