# Domain context

## Global library and shelf

Cita owns one user-global library rooted at `$CITA_HOME`, or `$HOME/.cita` by
default. `library.toml` contains a sorted set of stable shelf names and records
the fixed `main` default. Shelf locations are deterministic:
`shelves/<name>/shelf.toml`. Commands never discover local project files.

Every data command lazily initializes the library and `main`. An explicit shelf
must already exist; selection never creates. Shelf mutations are independent,
atomically replace one manifest, and hold a per-shelf advisory lock across the
read-modify-write operation. Batch sync and export run in shelf-name order and
continue after failures.

## Source snapshot

A source snapshot is authoritative provider-specific evidence stored under a
local citation key in `shelf.toml`. Every snapshot contains standalone BibTeX
and is tagged by its refresh lifecycle.

- `source = "inspire"` stores authoritative INSPIRE BibTeX, stable record ID,
  update timestamp, and canonical normalized arXiv/DOI values selected from and
  cross-checked against that BibTeX.
- `source = "import"` stores one exact standalone imported entry.

Snapshots project to provider-neutral `Reference` values. INSPIRE's curated
arXiv/DOI values override projected identities and add the namespaced provider
identity. Duplicate normalized identities are rejected within a shelf.

## Local citation key

The sorted manifest key is Cita's local citation identity and may differ from a
provider texkey. Refreshing a source snapshot never changes it. Rendering changes
only the raw BibTeX key token.

## Export

There is no managed `references.bib`. `cita export` deterministically materializes
an untracked, unverified, one-way BibTeX artifact from a selected shelf. Entries
are sorted by local key, separated by one blank line, and end with one newline.
The CLI adds an arXiv PDF URL when an entry has an arXiv identity but no authored
URL.

Relative outputs resolve from the caller's directory. An omitted output becomes
`<shelf>.bib`; exports cannot target the global store. Cita never reads exports
back.

## Documents

PDFs and safely extracted source packages live under the library-wide `files/`
cache and are shared across shelves by normalized, versionless arXiv ID.
Transient `fetch` resolves INSPIRE JSON only unless `--save` stores the complete
record in the selected shelf.
