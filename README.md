# bibi

bibi maintains one project's bibliography as a manifest and renders BibTeX from
it on demand.

```console
$ bibi add 1207.7214
@article{ATLAS:2012yve,
    author = "Aad, Georges and others",
    collaboration = "ATLAS",
    title = "{Observation of a new particle in the search for the Standard
              Model Higgs boson with the ATLAS detector at the LHC}",
    eprint = "1207.7214",
    doi = "10.1016/j.physletb.2012.08.020",
    ...
}
added 1, overwrote 0

$ bibi list --format bibtex > references.bib
```

Schema-1 `bibi.toml` is the only project state bibi maintains. A `.bib` file is
output: your shell writes it when you ask for it, bibi never reads it back, and
nothing keeps it silently in step behind your back.

Clone the project and you can reproduce its bibliography from the manifest
alone. There is no user-level library to install first and no database to be
out of sync with.

## The idea

**Bibliographic metadata is owned by providers, not by you.** You choose which
references to cite and what to call them; titles, authors, journals,
identifiers, and the BibTeX bytes are fetched and refreshed rather than
authored. Where no provider holds a work — a software release, a dataset, an
unpublished note — you supply its BibTeX once and bibi stores it verbatim.

Three consequences shape everything else:

- **Stored BibTeX is byte-identical to what bibi was given.** The only thing
  bibi ever rewrites is the citation-key token. Your publisher's journal
  abbreviations and collaboration formatting survive intact.
- **Your citation keys are yours.** A record adopts the provider's texkey when
  it is added and never changes again on its own. No refresh can invalidate a
  `\cite{}` in your document.
- **Rendering is reproducible.** The same manifest and the same options always
  produce the same bytes, with no network and no ambient state.

## Install

```console
cargo install --path crates/bibi
```

Rust 1.88 or newer.

## Commands

| Command | What it does |
| --- | --- |
| `bibi add [locator]…` | Resolve arXiv ids, DOIs, or `<provider>:<id>` and store them |
| `bibi add -f <file>` | Resolve each entry through one provider; use `--provider local` to keep entries as supplied |
| `bibi remove [selector]…` | Delete records, emitting what was deleted |
| `bibi rename <selector> <key>` | Change a local citation key |
| `bibi list` | List records as a table, `bibtex`, or `json`, or as `--fields` columns |
| `bibi show [selector]` | Emit exactly one record as BibTeX |
| `bibi sync` | Refresh what changed upstream |
| `bibi check [bibfile]` | Verify a rendered bibliography, byte for byte |
| `bibi fetch [selector]…` | Download records' PDFs or source archives |
| `bibi init` | Create an empty manifest |
| `bibi completions <shell>` | Print a completion script |

Records are selected by citation key, DOI, arXiv id, or `<provider>:<id>`, in
that order.

When `add`, `fetch`, `remove`, or `show` has no positional input, it reads one
locator or selector per nonblank stdin line. Explicit positionals win and stdin
is left untouched. An empty redirected stream is a successful no-op for the
three batch commands; `show` requires exactly one selector. Omitting input at an
interactive terminal is a usage error.

### Adding

```console
$ bibi add 1207.7214 10.1016/j.physletb.2012.08.020 inspire:1124337
$ bibi add 2401.00001 --key Smith:2024   # when you would rather name it yourself
$ bibi add -f colleague.bib
$ bibi add -f notes.bib --provider local
```

`add -f` **resolves**: each entry's DOI and arXiv id are looked up, and what
comes back replaces the entry's bytes while keeping the key the file used, so
your collaborator's `\cite{}` commands keep working. The whole invocation uses
one provider, INSPIRE by default, with no fallback. Missing, unsupported, and
failed identifiers fail that entry. `--provider local` instead keeps every
entry exactly as supplied under local provenance and performs no network I/O.

Duplicates are detected by DOI, arXiv id, and provider identity, and refused
unless you pass `--overwrite`. A refused duplicate is a skip, not a failure:
`bibi add <locator> && make` proceeds when the reference was already there.

### Listing

```console
$ bibi list
KEY                        AUTHOR                    YEAR  ARXIV       TITLE
Ghoderao:2026lvz           Ghoderao et al.           2026  2607.24734  Gravitational waves from
                                                                       super-Hubble bubbles
MartinBarandiaran:2026lnb  Martin Barandiar… et al.  2026  2607.26021  Weighted Webs:
                                                                       Morphology-Informed Marked
                                                                       Fields
```

The title wraps rather than being cut off, and never takes more than half the
terminal. The header is coloured only when the destination is a terminal;
`NO_COLOR` turns all styling off everywhere, including warnings.

`--format` says how to encode a listing; `--fields` says what to put in it, as
tab-separated columns. An absent value is an empty column rather than a missing
line, so the output stays aligned with the records it describes.

```console
$ bibi list --fields key
$ bibi list --fields key,year,title --year 2024
$ bibi list --fields arxiv-url | xargs -n1 curl -O   # the whole selection
```

That last form remains useful when another downloader should own the transfer.
`fetch` itself also accepts several selectors when bibi should validate and
download the batch.

### Syncing

```console
$ bibi sync
examined 214, unchanged 213, refreshed 1, 1 description change(s)
```

`sync` fetches a narrowed structured record for everything it manages, compares
each provider's change token, and fetches BibTeX only for what actually changed.
An unchanged project issues no BibTeX request at all, and a forced refresh of
three hundred records costs six requests rather than six hundred.

A record is updated as a unit. If its metadata arrives and its BibTeX does not,
nothing about it is written — including its revision, so the next sync tries
again rather than believing it is current.

### Rendering and checking

```console
$ bibi list --format bibtex --year 2024 > paper/references.bib
$ bibi check paper/references.bib --year 2024
```

bibi does not write bibliographies; it renders them to stdout and your shell
decides whether and where that becomes a file. Rendering is always offline, and
never refreshes anything on the way — run `sync` first if you want that:

```console
$ bibi sync && bibi list --format bibtex > references.bib
```

Whether you commit the rendered file is your business — bibi neither tracks it
nor rewrites it — and `check` covers the boundary if you do, exiting nonzero on
drift without repairing anything. It takes the same filters `list` does, so a
filtered view can be verified under the options it was made with.

Shell redirection truncates the destination before bibi runs. If that matters,
render to a sibling and move it into place:

```console
$ bibi list --format bibtex > refs.bib.tmp && mv refs.bib.tmp refs.bib
```

### Documents

```console
$ bibi fetch ATLAS:2012yve            # downloads ./1207.7214.pdf
$ bibi fetch ATLAS:2012yve --source   # downloads ./1207.7214.tar.gz
$ bibi fetch 1207.7214 --url          # any selector works; just the URL
$ bibi fetch ATLAS:2012yve -o higgs.pdf
$ bibi fetch First:2024 Second:2025 -o papers/
$ bibi list --fields key | bibi fetch --url
$ open $(bibi fetch ATLAS:2012yve)    # or compose it yourself
```

`fetch` gets files; it does not manage a collection. With no `--output`, each
download lands in the working directory under arXiv's own name. An existing
directory passed to `-o/--output` receives one or many default-named files. For
one selector, a non-directory path names an exact file; for several selectors
it is rejected before downloading.

The complete batch is planned before network access and downloaded
sequentially. Invalid selectors, records without arXiv ids, and occupied
destinations fail only those items; valid items continue, successful paths or
URLs are printed in input order, and any item failure makes the command exit
nonzero. Selectors that resolve to the same artifact are downloaded and printed
once, with later occurrences reported as successful skips. Destinations are
never overwritten unless `--force` is given.

There is no cache to grow, evict, or reason about, and nothing in the manifest
refers to a downloaded file. arXiv is the only source; a record without an arXiv
identifier says so plainly.

## Scope

A manifest is `bibi.toml` in the directory you are standing in, or the one
`-p/--path` names. That is the whole rule.

**There is no upward search and no user-level manifest.** Running `bibi list` in
a subdirectory targets that subdirectory, not the project above it, and no
command reaches for a library somewhere in your home directory. Inputs like
`add -f`, and `fetch` downloads, resolve against the directory you are in.

`init` exists, but nothing depends on it: `add` creates a manifest where you are
standing. Commands that only read report that there is no project instead.

## Output and exit codes

stdout carries the result in its most pipeable form — BibTeX for record
commands, a path or URL for `fetch`, whatever `--format` asks for from `list`.
stderr carries everything meant for a person.

| Code | Meaning |
| --- | --- |
| 0 | Success, including runs whose only non-successes were skips |
| 1 | An operational error, any item failure, or check drift |
| 2 | A usage error |

## Environment

| Variable | Effect |
| --- | --- |
| `BIBI_INSPIRE_BASE_URL` | Where INSPIRE requests go (for testing) |
| `BIBI_ARXIV_BASE_URL` | Where arXiv downloads come from (for testing) |

Both exist so the test suite can run against a local listener. bibi reads no
configuration file and stores no credential anywhere; INSPIRE needs none.

## Providers

INSPIRE is the default and currently the only remote provider. Provider calls
go through a closed facade: one invocation selects one provider, qualifiers
must agree, and absence or failure never falls through to another
implementation. Local is an explicit ingestion mode for `add -f`, not a remote
resolver, and its records have no provider handle.

Provider choice is more permanent than it looks: because stored BibTeX is
verbatim and never repaired, whichever provider resolves a record determines
that entry's quality in every bibliography you render from it. ADS and a DOI
registry for software and datasets are the intended next two.

## Design

[REDESIGN.md](REDESIGN.md) is the design authority and
[IMPLEMENTATION.md](IMPLEMENTATION.md) the crate-level contract.
[AGENTS.md](AGENTS.md) is the short version for people changing the code.

bibi replaces `cita` as a clean break, with no migration path from any cita
schema.

## License

MIT. See [LICENSE](LICENSE).
