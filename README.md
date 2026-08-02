# bibi

bibi maintains one project's bibliography in `bibi.toml` and renders exact
BibTeX on demand.

```console
$ bibi add 1207.7214
Aad:2012tfa
added 1, overwrote 0

$ bibi import local.bib
MySoftware:2026
added 1, overwrote 0

$ bibi list --format bibtex > references.bib
```

Schema 1 is the only authoritative project state. A `.bib` file is ordinary
derived output: bibi writes it to stdout when asked and never reads it back as
state.

## Model

The domain has five central concepts:

- `RecordState` is complete replaceable state: source, stable identifiers,
  description, and exact BibTeX.
- `Record` adds an immutable local UUID (`RecordId`) to a `RecordState`.
- `Bibliography` is the validated collection and owns all identity indexes and
  mutations.
- `Locator` identifies a record by texkey, provider identity, DOI, arXiv id, or
  a provider-specific opaque value.
- `Provider` resolves a strict locator batch into one complete positional
  `RecordState` per locator.

A record source is either `local` or a managed `(provider, provider_id)` pair.
Local is not a provider.

The texkey is always read from the exact stored BibTeX. It is not separate
state, cannot be overridden or renamed, and may change when a provider returns
new BibTeX during sync. The record UUID does not change.

## Install

```console
cargo install --path crates/bibi
```

Rust 1.88 or newer is required.

## Commands

| Command | Purpose |
| --- | --- |
| `bibi add [LOCATOR]…` | Resolve remote records and admit them |
| `bibi import [FILE]` | Import exact local BibTeX from a file or redirected stdin |
| `bibi remove [LOCATOR]…` | Remove records as one strict batch |
| `bibi show [LOCATOR]` | Emit exactly one record as BibTeX |
| `bibi list` | List as a table, JSON, BibTeX, or selected fields |
| `bibi sync` | Unconditionally resolve complete current state for managed records |
| `bibi check [BIBFILE]` | Compare a derived bibliography byte for byte |
| `bibi init` | Create an empty `bibi.toml` |
| `bibi completions SHELL` | Print shell completions |

There is no `rename` or `fetch` command. Use
`bibi list --fields arxiv-url` as the document-download piping surface.

### Input

`add` and `remove` accept positionals or newline-delimited stdin. `show` accepts
one positional or exactly one nonblank stdin line. Explicit positionals leave
stdin untouched.

`import FILE` reads that path. With no file it reads the complete redirected
stdin stream, which supports pipelines:

```console
generate-bibtex | bibi import
```

Omitting the file at an interactive terminal is a usage error. `-` has no
special meaning; it is an ordinary filename.

### Identity and overwrite

Stable claims are managed provider identity, DOI, and arXiv id. Without
`--overwrite`, any stable collision fails the complete command. With
`--overwrite`:

- zero matched UUIDs inserts a new record;
- one matched UUID replaces its complete state while preserving the UUID;
- matches spanning multiple UUIDs fail.

Texkeys enforce uniqueness but never choose an overwrite target. Duplicate
incoming claims, divergent stable identifiers, duplicate texkeys, multiple
changes targeting one UUID, and repeated removal targets all fail the entire
batch. No partial successor is committed.

`--dry-run` still parses, resolves, and validates the full successor; it only
skips the final publication.

### Sync

Sync builds provider-identity locators from every selected managed record and
uses the same provider `resolve` operation as add. It is unconditional: there
are no revisions, conditional metadata calls, or `--force`.

The returned source identity must equal the requested identity. The UUID is
retained locally, while all `RecordState` fields—including the BibTeX and its
derived texkey—are authoritative replacements. One absence, mapping error,
transport error, payload-join error, or collision aborts every update.

### Rendering and checking

Rendering sorts by the current derived texkey and concatenates exact stored
entry bytes with one blank line between entries and one final newline.

```console
bibi sync
bibi list --format bibtex > references.bib
bibi check references.bib
bibi list --fields key,year,title
bibi list --fields arxiv-url | xargs -n1 curl -O
```

Rendering, listing, showing, and checking are offline.

## Scope and persistence

The target is `./bibi.toml`, or the exact file named by global `-p/--path`.
There is no upward search and no user-level store.

Every mutation constructs a fully validated successor `Bibliography`, compares
the exact generation read with the bytes currently on disk, and atomically
replaces the TOML through a same-directory temporary file. The project manifest
is the only file the CLI writes.

Schema 1 records contain:

- `id` (canonical UUID);
- `source = "local"` or an installed provider name;
- `provider_id` for managed records only;
- optional DOI/arXiv id and description fields;
- exact `bibtex`.

There is no stored `key` or `revision`. Unknown fields and unknown provider
names are rejected.

## Providers

INSPIRE is the only installed provider and the default. Selection is closed:
`--provider` and qualified locators must agree, and failure never falls through
to another implementation.

INSPIRE keeps batching, proactive pacing, retry, structured metadata mapping,
bulk BibTeX retrieval, and verified texkey joining private to its adapter.
Provider BibTeX is never interpreted as metadata; local BibTeX is the original,
so local import projects its description and identifiers from that input.

## Output and exit codes

stdout contains pipeable command results. stderr contains human diagnostics.

| Code | Meaning |
| --- | --- |
| 0 | Complete success |
| 1 | Operational or validation failure |
| 2 | CLI usage error |

`BIBI_INSPIRE_BASE_URL` redirects INSPIRE requests for hermetic tests. No
document-download runtime is configured by the CLI.

## Workspace

The multi-crate split is deliberate:

```text
bibi-bibtex -> bibi-core -> bibi-manifest
                    |             |
                    v             v
              bibi-inspire -> bibi-provider -> bibi-application -> bibi

bibi-documents  (dormant, buildable workspace crate)
```

See [AGENTS.md](AGENTS.md) for contributor contracts.

## License

MIT. See [LICENSE](LICENSE).
