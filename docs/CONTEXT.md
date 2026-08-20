# Domain context

## Library and shelf

A library is a `cita-library.toml` registry that maps stable shelf names to
library-root-relative paths. A shelf is a completely independent cita project
with its own authoritative manifest, generated bibliography, document cache,
identities, and Git commits. The registry routes commands; it does not aggregate
or share bibliographic state.

Shelf names are stable identifiers; the registered path is fixed at
registration time, and registering an existing name under a different path is
rejected. Paths must remain beneath the library root and cannot be equal,
nested, or symlink aliases. Library discovery walks ancestors independently of
nearest-project `cita.toml` discovery. Library-wide sync and generation are
ordered collections of independent shelf mutations, not one crash-atomic
transaction.

## Reference

A `Reference` is Cita's provider-neutral semantic view: entry type, title,
authors, collaborations, display year, and normalized identifiers. It is the
*ingest seam* only. A provider hands one over, an `Entry` is seeded from it once,
and no command consumes a `Reference` afterwards.

## Entry

An entry is what `cita.toml` stores under a local citation key. It is the sole
authority for a reference.

- **Structured fields** — `type`, `title`, `authors`, `collaborations`, `year`,
  `doi`, `arxiv` — are authoritative. They are seeded once at ingest by
  `Entry::from_inspire` or `Entry::from_bibtex`, the only two projection sites in
  the codebase, and are read directly from then on.
- **`tags` and `notes`** are user-owned. A refresh never touches them. Tags are a
  set, so they stay sorted and deduplicated; notes are a sequence in the order
  they were written.
- **`bibtex`** is optional, immutable, opaque cargo: the exact standalone entry
  a provider or an import supplied. It is never derived from the structured
  fields and never parsed to infer them. A reference no provider knows about —
  a book, a thesis, a web page — simply has none.

Projection runs at ingest and nowhere else. Reading, validating, selecting, and
listing all work from stored fields, so no command parses BibTeX to answer a
question about a reference.

## Managed and unmanaged references

Provenance is the presence of a provider sub-table, not a tag field. An entry
carrying `inspire` (`record_id`, `updated`) is *managed*: `cita sync` refreshes
every provider-owned field on it, including the BibTeX blob, by stable record ID.
An entry without one is *unmanaged* and no provider ever touches it. Cita does
not merge provenance or adopt one source as another.

The line between provider-owned and user-owned fields is enforced in code:
`Entry::refresh_from_inspire` carries tags and notes forward across a sync, and
`cita edit` refuses a change to a managed entry's provider-owned fields, or to
`bibtex` on any entry, naming the offending `key.field`.

## Local citation key

The sorted key in `references` is Cita's local identity for citation and Git
review. It may differ from a provider texkey. It is user-owned: refreshing an
entry never changes it, and bibliography generation changes only the raw entry's
key token.

## Provider identity

An identifier names the same work independently of its local key. DOI and arXiv
identifiers are normalized globally. Provider identities are namespaced, for
example `inspire:1124337`. They are read from the entry's own `doi`, `arxiv`, and
`inspire.record_id` fields. Any identity shared by different local keys is a
conflict, whether the entries are managed or not.

## Generated bibliography

`references.bib` is a tracked generated artifact, analogous to a lockfile. Its
bytes are completely derived from the manifest: local-key order, preserved raw
entry fields, one blank line between entries, and a final newline. Drift is an
error; `cita generate` repairs it.

It holds the entries that have BibTeX, not every reference. An entry without a
`bibtex` blob has nothing to render, and its absence is not drift.

## Derived export

The `cita export` output is a derived artifact, not a generated one. It shares
the generated bibliography's layout and ordering but adds resolvable `url`
fields for downstream reference managers. Unlike `references.bib` it is not
tracked, not verified, never read back, and never authoritative — it is written
for other tools to consume and regenerated rather than edited.

## Manifest edit

`cita edit` is a whole-manifest edit in an external editor, and the only way to
create a reference no provider knows about. A rejected buffer is re-opened with
the reasons as `# cita:` comments above the user's own bytes; saving it back
unchanged aborts. Because the saved manifest is re-rendered from parsed data, no
comment ever reaches `cita.toml`. Adding and deleting keys is how provider-less
references are created and removed.

## Schema break

Schema 2 is a hard break from schema 1, which stored opaque BibTeX blobs and had
no structured fields to migrate. Every command rejects a schema-1 manifest
outright. The recovery path — `rm cita.toml && cita init` — rebuilds from
`references.bib`, which carries no `record_id`, so every entry returns unmanaged;
re-running `cita add` for those locators restores management. The library
registry keeps its own schema and is unaffected.
