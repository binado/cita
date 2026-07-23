# cita

cita is a Git-friendly bibliography CLI. It keeps authoritative source snapshots
in `cita.toml` and deterministically generates the tracked `references.bib`.
INSPIRE is the managed metadata provider: its records retain canonical
identifiers selected from and cross-checked against INSPIRE's exact BibTeX and
can be refreshed by stable record ID. Generic BibTeX ingestion is available
through `cita import`, which preserves each standalone entry's exact source
bytes.

Requires Rust 1.88 or newer.

```bash
cargo install cita
```

## Quick start

```bash
cita init
cita add https://arxiv.org/abs/1207.7214 doi:10.1016/j.physletb.2012.08.020
cita import local-references.bib
cita list
cita sync
cita fetch 1207.7214
```

Several independent projects can also be registered as shelves in one library:

```bash
cita library init
cita library shelf paper-one init --path papers/paper-one
cita library shelf paper-one add 1207.7214
cita library shelves
cita library sync
```

Supported locators are bare arXiv IDs; explicit `arxiv:`, `doi:`, or `inspire:`
locators; and canonical `https://arxiv.org`, `https://inspirehep.net`, or
`https://doi.org` URLs. Selectors first match an exact local citation key, then
a provider ID, normalized DOI, or normalized arXiv ID. The same canonical URLs
work as selectors.

## Commands

- `cita init [--path <directory>]` creates an empty schema-1 project in the
  current directory, or in the specified existing directory. Initialization
  does not run Git, so malformed repository metadata or an unavailable Git
  executable cannot prevent it. Commands run inside a nested project discover
  its nearest `cita.toml`. If only `references.bib` exists at the chosen
  location, initialization imports every standalone entry. Existing schema-1
  projects are validated; any other schema is explicitly unsupported. The
  target directory must already exist, and invalid existing content is rejected
  without rewriting the managed files.
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
- `cita export [-o/--output <file>]` writes a derived BibTeX file for tools that
  want a resolvable link, such as Zotero. It renders the same entries as
  `references.bib` and adds `url = {https://arxiv.org/pdf/<id>}` to each entry
  with an arXiv ID, leaving entries that already define a `url` untouched. The
  default file is named for the project directory; a relative `--output` is
  relative to the directory where cita was invoked. The export never overwrites
  a managed file, and it refuses to run while `references.bib` has drifted.
- `cita fetch [--force | --cache-only | --url] [--source] [--open] [--save]
  <selector>` returns an absolute cached PDF path by default, the arXiv PDF URL
  with `-u/--url`, or an absolute extracted source directory with `--source`.
  `--source` and `--url` are mutually exclusive. `--open` launches the returned
  target with the system default application. Without `--save`, an unmatched
  locator uses INSPIRE JSON only.
- `cita commit` is an optional Git helper. It validates consistency and commits
  only `cita.toml` and `references.bib`, leaving unrelated staged changes
  intact. It refuses to run if either managed file is already staged.
- `cita library init [--path <directory>]` creates an idempotent
  `cita-library.toml` registry in an existing directory. A library root cannot
  itself be a cita project.
- `cita library shelves` lists stable shelf names and their library-relative
  paths in deterministic order.
- `cita library shelf <name> init [--path <relative-directory>]` creates and
  registers an independent shelf. It can create an empty project, import an
  existing standalone `references.bib`, or adopt an existing verified cita
  project. Initialization completes before registration, so a registry write
  failure leaves a usable standalone shelf for a safe retry.
- `cita library shelf <name>
  <add|import|remove|list|generate|export|sync|fetch|commit> ...` runs the
  corresponding command in that shelf. Import and export paths remain relative
  to the directory where the user invoked cita, not to the shelf.
- `cita library generate`, `cita library export`, and `cita library sync`
  process every shelf in name order, continue after shelf-specific failures,
  print one result per shelf, and exit unsuccessfully if any shelf failed. A
  shelf export is named for the stable registered shelf name, so importing each
  file into Zotero yields one collection per shelf.
- `cita completions <bash|elvish|fish|powershell|zsh>` prints a shell completion
  script to stdout, e.g. `cita completions zsh > ~/.zfunc/_cita`.

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

The `cita export` output is a derived, one-way convenience artifact. It is never
authoritative, is not tracked or verified, and is not read back by any command;
regenerate it instead of editing it, and add it to `.gitignore` if you do not
want it tracked. Re-importing an export into Zotero adds items again rather than
updating the previous import.

Mutations validate and render the complete candidate in memory, atomically
persist `references.bib` first, and persist `cita.toml` as the commit point.
Downloaded PDFs and extracted source packages live under `.cita/files`;
initialization adds
`/.cita/files/` to the project root's `.gitignore` so the cache is not tracked.

A library is only a sorted registry of shelf names and relative paths. Each
shelf has its own `cita.toml`, `references.bib`, `.cita/files` cache, identities,
and optional Git history. There is no aggregate bibliography, shared cache, or
cross-shelf citation-key/identifier uniqueness. Registered paths cannot escape
the library root, overlap or nest, or alias one another through symlinks.

## Workspace

- `cita-core`: locators, neutral `Reference` vocabulary, and provider traits.
- `cita-bibliography`: standalone BibTeX snapshot projection, raw-entry
  preservation, re-keying, and `biblatex`-based generic rendering.
- `cita-inspire-client`: typed INSPIRE JSON metadata, authoritative BibTeX
  snapshots, and stable-ID refreshes.
- `cita-manifest`: schema-1 shelf and library validation, identity indexes,
  deterministic TOML, path safety, output verification, and coordinated writes.
- `cita-documents`: validated arXiv PDF/source downloads, safe source
  extraction, and atomic caching.
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

All six crates share one version and their APIs are intentionally unstable
throughout 0.x. Releases are automated with
[release-plz](https://release-plz.dev) (`release-plz.toml`,
`.github/workflows/release-plz.yml`):

1. Merge Conventional-Commit PRs to `main`.
2. release-plz opens (or updates) a "release PR" that bumps the shared version
   and updates every crate's `CHANGELOG.md`.
3. Merging that release PR publishes all six crates in dependency order and tags
   them.

After the first release, smoke-test with `cargo install cita --locked`.

As an emergency fallback, the crates can still be published by hand in
dependency order: `cita-core`, then `cita-bibliography` and `cita-documents`,
then `cita-inspire-client`, then `cita-manifest`, and finally `cita`.
