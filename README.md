# bibi

bibi is a Git-friendly bibliography CLI whose source of truth is your
`references.bib` file. Entries are authoritative BibTeX and stay exactly as you
or your provider wrote them; bibi's own bookkeeping rides along in `x-bibi-*`
fields on the entries, where LaTeX ignores it. There is no manifest, no
generated artifact, and nothing to keep in step.

INSPIRE is the managed metadata provider: its records retain canonical
identifiers cross-checked against INSPIRE's exact BibTeX and are refreshed by
stable record ID.

Requires Rust 1.88 or newer.

```bash
cargo install bibi
```

## Quick start

```bash
touch references.bib
bibi add https://arxiv.org/abs/1207.7214 doi:10.1016/j.physletb.2012.08.020
bibi import colleague.bib
bibi list
bibi sync
bibi fetch 1207.7214
```

bibi operates on `./references.bib` unless you point it elsewhere with
`-p/--path` (a file or a directory) or `$BIBI_BIB`. It never searches parent
directories, and it never creates the file for you — if you work in
subdirectories, set `BIBI_BIB` in a `.envrc`.

Supported locators are bare arXiv IDs; explicit `arxiv:`, `doi:`, or `inspire:`
locators; and canonical `https://arxiv.org`, `https://inspirehep.net`, or
`https://doi.org` URLs. Selectors first match an exact local citation key, then
a provider ID, normalized DOI, or normalized arXiv ID. The same canonical URLs
work as selectors.

## Commands

- `bibi add [--key K] [--overwrite] <locator>...` resolves INSPIRE JSON and
  authoritative BibTeX. `--key` keeps an independent local key and accepts one
  locator. A failed lookup writes nothing.
- `bibi import [--overwrite] <path|->...` folds entries from other BibTeX files
  into yours. Sources are only ever read. Comments and directives in a source
  are tolerated, and any `x-bibi-*` fields on incoming entries are stripped —
  someone else's bookkeeping is not evidence about your bibliography, and
  `bibi sync` re-establishes it from each entry's own identity.
- `bibi sync [--dry-run] [--verbose]` reconciles with INSPIRE. Managed entries
  refresh by record ID; entries INSPIRE recognizes but bibi does not yet track
  are adopted, which attaches bookkeeping while leaving your wording alone.
  Entries INSPIRE does not know are reported, not treated as failures.
- `bibi remove <selector>...` removes a batch atomically.
- `bibi rekey <selector> <new-key>` changes one entry's citation key, touching
  only that token.
- `bibi list [--sort-by key|title|author|year] [--order asc|desc]` displays
  source-neutral projections.
- `bibi check` validates the file and reports every problem at once.
- `bibi export [-o/--output <file>] [--keep-metadata]` writes a copy for
  somebody else to read: `x-bibi-*` fields removed, and
  `url = {https://arxiv.org/pdf/<id>}` added to each entry with an arXiv ID,
  leaving authored `url` values untouched. `-o -` writes to stdout. The default
  file is named for the bibliography's directory. The only target it refuses is
  the bibliography itself.
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

Git is yours to drive. bibi writes one tracked text file in the format you read,
so `git diff` already shows what changed.

## Storage rules

`references.bib` is the sole authority. Bibliographic content is the BibTeX
itself; bibi adds only these fields:

| Field | Meaning |
|---|---|
| `x-bibi-inspire-id` | stable INSPIRE record id; its presence is what makes an entry managed |
| `x-bibi-inspire-updated` | provider timestamp, so a sync that learns nothing writes nothing |
| `x-bibi-arxiv` | curated normalized arXiv id |
| `x-bibi-doi` | curated normalized DOI |
| `x-bibi-frozen` | never refreshed, never resolved — for entries you have corrected by hand, and for work INSPIRE will never have |

Because the file is yours to edit, **a mutation rewrites only the entries it
touches**. Your comments, `@string` directives, indentation, and entry ordering
survive every command; new entries are appended rather than sorted in. Loading
and writing back without changing anything reproduces the file byte for byte.

Duplicate normalized DOI, arXiv, or provider identities are rejected across the
whole file, as are missing titles and unsafe citation keys.

The projected `Reference` intentionally contains only the fields bibi needs for
selection and display: title, authors, collaborations, year, and DOI/arXiv/
provider identifiers. The BibTeX entry remains available for everything else.

The `bibi export` output is a derived, one-way convenience artifact. It is never
authoritative, is not tracked or verified, and is not read back by any command;
regenerate it instead of editing it. Re-importing an export into Zotero adds
items again rather than updating the previous import.

Mutations validate a complete candidate in memory, then replace the file in one
atomic write. Downloaded PDFs and extracted source packages live under
`.bibi/files` beside the bibliography, and `/.bibi/files/` is added to the
directory's `.gitignore` so the cache is not tracked.

## Workspace

- `bibi-core`: locators, neutral `Reference` vocabulary, and provider traits.
- `bibi-bibliography`: BibTeX projection, whole-file span scanning, raw-entry
  preservation, re-keying, and field insertion and removal.
- `bibi-inspire-client`: typed INSPIRE JSON metadata, authoritative BibTeX
  snapshots, and stable-ID refreshes.
- `bibi-bibfile`: the `.bib` file as the store — byte-preserving mutation,
  identity uniqueness, validation, and path resolution.
- `bibi-documents`: validated arXiv PDF/source downloads, safe source
  extraction, and atomic caching.
- `bibi`: CLI wiring, sync reconciliation, and export policy.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Provider and document tests are hermetic and use local TCP listeners.

## Publishing

All six crates share one version. Releases are automated with
[release-plz](https://release-plz.dev) (`release-plz.toml`,
`.github/workflows/release-plz.yml`):

1. Merge Conventional-Commit PRs to `main`.
2. release-plz opens (or updates) a "release PR" that bumps the shared version
   and updates every crate's `CHANGELOG.md`.
3. Merging that release PR publishes all six crates in dependency order and
   tags them.

After a release, smoke-test with `cargo install bibi --locked`.

As an emergency fallback, the crates can still be published by hand in
dependency order: `bibi-core`, then `bibi-bibliography` and `bibi-documents`,
then `bibi-inspire-client`, then `bibi-bibfile`, and finally `bibi`.
