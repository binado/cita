# bibi

bibi maintains a bibliography as a manifest and renders a BibTeX file from it on
demand.

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

$ bibi export
wrote 1 record(s) to /home/you/paper/references.bib
```

Schema-1 `bibi.toml` is the only project state bibi maintains. `references.bib`
is output: it is written when you ask for it, never read back, and never kept
silently in step behind your back.

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
| `bibi add <locator>…` | Resolve arXiv ids, DOIs, or `<provider>:<id>` and store them |
| `bibi add -f <file>` | Resolve each entry in a `.bib`; keep the rest as local records |
| `bibi remove <selector>…` | Delete records, emitting what was deleted |
| `bibi rename <selector> <key>` | Change a local citation key |
| `bibi list` | List records as a table, `keys`, `bibtex`, or `json` |
| `bibi show <selector>` | Emit one record as BibTeX |
| `bibi sync` | Refresh what changed upstream |
| `bibi export` | Render the bibliography |
| `bibi check [bibfile]` | Verify a rendered bibliography, byte for byte |
| `bibi fetch <selector>` | Get the PDF or source package |
| `bibi cache clean` | Evict the global document cache |
| `bibi init` | Create an empty manifest |
| `bibi completions <shell>` | Print a completion script |

Records are selected by citation key, DOI, arXiv id, or `<provider>:<id>`, in
that order.

### Adding

```console
$ bibi add 1207.7214 10.1016/j.physletb.2012.08.020 inspire:1124337
$ bibi add 2401.00001 --key Smith:2024   # when you would rather name it yourself
$ bibi add -f colleague.bib
```

`add -f` **resolves**: each entry's DOI and arXiv id are looked up, and what
comes back replaces the entry's bytes while keeping the key the file used, so
your collaborator's `\cite{}` commands keep working. An entry every provider
reports as absent is kept exactly as written, under a local provider that never
refreshes it. A provider *failure* is never treated as absence — a timeout says
nothing about whether a work exists.

Duplicates are detected by DOI, arXiv id, and provider identity, and refused
unless you pass `--overwrite`. A refused duplicate is a skip, not a failure:
`bibi add <locator> && make` proceeds when the reference was already there.

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

### Exporting and checking

```console
$ bibi export --output paper/references.bib --year 2024
$ bibi check paper/references.bib --year 2024
```

`export` is the only way a bibliography is written, and plain `export` is
offline. `export --provider inspire` syncs that provider first and writes
nothing if the sync reported a failure. Whether you commit the rendered file is
your business — bibi neither tracks it nor rewrites it — and `check` covers the
boundary if you do, exiting nonzero on drift without repairing anything.

### Documents

```console
$ bibi fetch ATLAS:2012yve            # an absolute path to the cached PDF
$ bibi fetch ATLAS:2012yve --source   # the extracted source tree
$ bibi fetch 1207.7214 --url          # any selector works; just the URL
$ open $(bibi fetch ATLAS:2012yve)
```

Documents live in one user-level cache shared by every project, addressed by
arXiv id, so two projects citing one paper store one copy. The cache is derived
and disposable: removing a record evicts nothing, and `bibi cache clean --all`
is how it goes away.

## Scope

A manifest is `bibi.toml` in the directory you are standing in. `-p/--path`
names another one and `-g/--global` selects the user-level manifest, which is an
ordinary project at a fixed path.

**There is no upward search.** Running `bibi list` in a subdirectory targets
that subdirectory, not the project above it. Outputs resolve against the
manifest's directory; inputs like `add -f` resolve against yours.

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
| `BIBI_GLOBAL_MANIFEST` | Where `-g` points |
| `BIBI_CACHE_ROOT` | Where documents are cached |

bibi reads no configuration file, and stores no credential anywhere. INSPIRE
needs none.

## Providers

INSPIRE is the first provider; the local provider exists so that a bibliography
is always complete. Providers differ in capability, not in kind — the local one
is simply a provider whose records never change.

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
