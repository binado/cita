# cita

cita is a Git-friendly reference manager. It keeps authoritative, readable
reference fields in `cita.toml` — plus your own tags and notes — and
deterministically generates the tracked `references.bib`. INSPIRE is the managed
metadata provider: its records can be refreshed by stable record ID, and a
refresh never touches what you wrote. Generic BibTeX ingestion is available
through `cita import`, which preserves each standalone entry's exact source
bytes, and `cita edit` adds references no provider knows about — a book, a
thesis, a web page.

Requires Rust 1.88 or newer.

```bash
cargo install cita
```

## Quick start

```bash
cita init
cita add https://arxiv.org/abs/1207.7214 doi:10.1016/j.physletb.2012.08.020
cita import local-references.bib
cita edit                      # add tags and notes, or a provider-less reference
cita list --tag reading-list
cita sync
cita fetch 1207.7214
```

Several independent projects can also be registered as shelves in one library:

```bash
cita library init
cita library new paper-one --path papers/paper-one
cita add 1207.7214 --shelf paper-one
cita library list
cita sync --all-shelves
```

Supported locators are bare arXiv IDs; explicit `arxiv:`, `doi:`, or `inspire:`
locators; and canonical `https://arxiv.org`, `https://inspirehep.net`, or
`https://doi.org` URLs. Selectors first match an exact local citation key, then
a provider ID, normalized DOI, or normalized arXiv ID. The same canonical URLs
work as selectors.

## Commands

- `cita init [--path <directory>]` creates an empty schema-2 project in the
  current directory, or in the specified existing directory. Initialization
  runs no external programs. Commands run inside a nested project discover
  its nearest `cita.toml`. If only `references.bib` exists at the chosen
  location, initialization imports every standalone entry. Existing schema-2
  projects are validated; any other schema is explicitly unsupported. The
  target directory must already exist, and invalid existing content is rejected
  without rewriting the managed files.
- `cita import <path|->` atomically imports all standalone entries from a file
  or stdin.
- `cita add [--key K] <locator>...` resolves INSPIRE JSON and authoritative
  BibTeX. `--key` keeps an independent local key and accepts one locator.
- `cita sync` refreshes only INSPIRE-managed entries by stable record ID. It
  leaves unmanaged entries byte-for-byte unchanged, and leaves the `tags` and
  `notes` on a managed entry exactly as you wrote them.
- `cita remove <selector>...` removes a batch atomically.
- `cita list [--sort-by key|title|author|year] [--order asc|desc] [--tag <TAG>]`
  lists stored references. `--tag` is repeatable and narrows: an entry has to
  carry every tag named.
- `cita edit` opens the whole manifest in `$VISUAL`, `$EDITOR`, or `vi`. This is
  where you add tags and notes, and the only way to create a reference no
  provider knows about. A rejected edit is re-opened with the reasons as `#`
  comments above your own text; save it back unchanged to give up.
- `cita generate` repairs a missing or edited `references.bib` from the
  authoritative manifest.
- `cita export [-o/--output <file>]` writes a derived BibTeX file for tools that
  want a resolvable link, such as Zotero. It renders the same entries as
  `references.bib` and adds `url = {https://arxiv.org/pdf/<id>}` to each entry
  with an arXiv ID, leaving entries that already define a `url` untouched. The
  default file is named for the project directory; a relative `--output` is
  relative to the directory where cita was invoked. The export never overwrites
  a managed file — not this project's, and not a `cita.toml`, `references.bib`,
  or `cita-library.toml` belonging to any other project or library — and it
  refuses to run while `references.bib` has drifted.
- `cita fetch [--force | --cache-only | --url] [--source] [--open] [--save]
  <selector>` returns an absolute cached PDF path by default, the arXiv PDF URL
  with `-u/--url`, or an absolute extracted source directory with `--source`.
  `--source` and `--url` are mutually exclusive. `--open` launches the returned
  target with the system default application. Without `--save`, an unmatched
  locator uses INSPIRE JSON only.
- `cita library init [--path <directory>]` creates an idempotent
  `cita-library.toml` registry in an existing directory. A library root cannot
  itself be a cita project.
- `cita library list` (alias `ls`) lists stable shelf names and their
  library-relative paths in deterministic order.
- `cita library new <name> [--path <relative-directory>]` (alias `create`)
  creates and registers an independent shelf. It can create an empty project,
  import an existing standalone `references.bib`, or adopt an existing verified
  cita project. Initialization completes before registration, so a registry
  write failure leaves a usable standalone shelf for a safe retry.
- `-s/--shelf <name>` runs any of `add`, `import`, `remove`, `list`, `edit`,
  `generate`, `export`, `sync`, and `fetch` in that registered shelf
  instead of the project discovered from the current directory. Import and export
  paths remain relative to the directory where the user invoked cita, not to the
  shelf. A shelf export is named for the stable registered shelf name rather
  than the shelf directory, so importing each file into Zotero yields one
  collection per shelf.
- `--all-shelves` runs `generate`, `export`, or `sync` in every shelf in name
  order, continuing after shelf-specific failures, printing one result per
  shelf, and exiting unsuccessfully if any shelf failed. It is mutually
  exclusive with `--shelf`, and with `export --output`, which cannot name a file
  for each shelf. The remaining commands are deliberately excluded: mutations
  stay per-shelf.
- `cita completions <bash|elvish|fish|powershell|zsh>` prints a shell completion
  script to stdout, e.g. `cita completions zsh > ~/.zfunc/_cita`.

Successful `fetch` output is suitable for command substitution; status messages
are written to stderr. For example, choose a specific PDF viewer on macOS with
`open -a Skim "$(cita fetch <selector>)"`.

## Storage rules

`cita.toml` is the sole authority. Each sorted local key holds one entry:

```toml
schema = 2

[references.Higgs2012]
type = "article"
title = "Observation of a new particle in the search for the SM Higgs boson"
authors = ["Georges Aad", "others"]
collaborations = ["ATLAS"]
year = 2012
doi = "10.1016/j.physletb.2012.08.020"
arxiv = "1207.7214"
tags = [
    "atlas",
    "higgs",
]
notes = ["Superseded for the mass measurement by 1503.07589."]
bibtex = """
@article{ATLAS:2012yve,
    title = "{Observation of a new particle}",
    eprint = "1207.7214"
}"""

[references.Higgs2012.inspire]
record_id = 1124337
updated = "2026-06-26T15:56:33.515824+00:00"

[references.Rovelli2004]
type = "book"
title = "Quantum Gravity"
authors = ["Carlo Rovelli"]
year = 2004
tags = ["reading-list"]
```

The structured fields are authoritative. They are seeded once, when the reference
is added or imported, and are never re-derived afterwards. `tags` and `notes` are
yours: no command rewrites them. `bibtex` is the exact entry a provider or an
import supplied — immutable, never generated from the fields above it, and
absent entirely for a reference no provider knows about.

An entry with an `inspire` sub-table is *managed*: `cita sync` refreshes its
bibliographic fields by stable record ID. An entry without one is unmanaged and
is never touched by a provider. `cita edit` enforces the same line, refusing a
change to a managed entry's provider-owned fields, or to any `bibtex`, and naming
the field it refused.

Entries are strictly validated, and duplicate normalized DOI, arXiv, or provider
identities are rejected across every entry.

Schema 2 is a hard break: a schema-1 `cita.toml` is rejected outright and there
is no migration, because it stored opaque BibTeX with no structured fields to
read. To recover, delete `cita.toml` and run `cita init`, which rebuilds from
`references.bib` — then re-run `cita add` for anything you want managed again,
since a BibTeX entry carries no INSPIRE record ID.

`references.bib` holds the entries that have BibTeX, not every reference, and
behaves like a lockfile: entries are sorted by local key,
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
Downloaded PDFs and extracted source packages live under `.cita/files`; if you
track the project in Git, add `/.cita/files/` to your own `.gitignore` so the
cache is not tracked.

A library is only a sorted registry of shelf names and relative paths. Each
shelf has its own `cita.toml`, `references.bib`, `.cita/files` cache, identities,
and optional Git history. There is no aggregate bibliography, shared cache, or
cross-shelf citation-key/identifier uniqueness. Registered paths cannot escape
the library root, overlap or nest, or alias one another through symlinks.

## Workspace

- `cita-core`: locators, neutral `Reference` vocabulary, and provider traits.
- `cita-bibliography`: standalone BibTeX snapshot projection at ingest,
  raw-entry preservation, re-keying, and `biblatex`-based generic rendering.
- `cita-inspire-client`: typed INSPIRE JSON metadata, authoritative BibTeX
  snapshots, and stable-ID refreshes.
- `cita-manifest`: schema-2 entries, shelf and library validation, identity
  indexes, deterministic TOML, path safety, output verification, and coordinated
  writes.
- `cita-documents`: validated arXiv PDF/source downloads, safe source
  extraction, and atomic caching.
- `cita`: CLI wiring, discovery, and selectors.

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
